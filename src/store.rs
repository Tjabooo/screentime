//! SQLite storage: app sessions, pickups and what has been pushed.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY,
    app TEXT NOT NULL,
    title TEXT,
    start REAL NOT NULL,
    end REAL NOT NULL,
    open INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS sessions_end ON sessions(end);
CREATE TABLE IF NOT EXISTS pickups (ts REAL NOT NULL, reason TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS pickups_ts ON pickups(ts);
CREATE TABLE IF NOT EXISTS pushes (day TEXT PRIMARY KEY, sent REAL NOT NULL, final INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
";

pub type Result<T> = std::result::Result<T, String>;

fn err(e: rusqlite::Error) -> String {
    format!("database: {e}")
}

/// One stored app session: (app, title, start, end).
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub app: String,
    pub title: Option<String>,
    pub start: f64,
    pub end: f64,
}

pub struct Store {
    pub db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let db = Connection::open(path).map_err(err)?;
        db.busy_timeout(Duration::from_secs(10)).map_err(err)?;
        // WAL + NORMAL: a crash of the daemon loses nothing, a power cut at most the last beat.
        db.pragma_update(None, "journal_mode", "WAL").map_err(err)?;
        db.pragma_update(None, "synchronous", "NORMAL").map_err(err)?;
        db.execute_batch(SCHEMA).map_err(err)?;
        set_private(path);
        Ok(Store { db })
    }

    pub fn open_readonly(path: &Path) -> Result<Store> {
        if !path.exists() {
            return Err(format!("No Screen Time data yet at {} (is the daemon running?).", path.display()));
        }
        let db =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(err)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(err)?;
        Ok(Store { db })
    }

    // ---- writes (daemon) ----
    pub fn open_session(&self, app: &str, title: Option<&str>, start: f64) -> Result<i64> {
        self.db
            .execute("INSERT INTO sessions (app, title, start, end, open) VALUES (?1, ?2, ?3, ?3, 1)", params![app, title, start])
            .map_err(err)?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn extend_session(&self, id: i64, end: f64) -> Result<()> {
        self.db.execute("UPDATE sessions SET end = MAX(start, ?1) WHERE id = ?2", params![end, id]).map_err(err)?;
        Ok(())
    }

    pub fn close_session(&mut self, id: i64, end: f64, min_length: f64) -> Result<()> {
        let tx = self.db.transaction().map_err(err)?;
        tx.execute("UPDATE sessions SET end = MAX(start, ?1), open = 0 WHERE id = ?2", params![end, id]).map_err(err)?;
        tx.execute("DELETE FROM sessions WHERE id = ?1 AND end - start < ?2", params![id, min_length]).map_err(err)?;
        tx.commit().map_err(err)
    }

    /// Sessions left open by a crash end at their last heartbeat.
    pub fn close_stale(&self) -> Result<()> {
        self.db.execute("UPDATE sessions SET open = 0 WHERE open = 1", []).map_err(err)?;
        Ok(())
    }

    pub fn add_pickup(&self, ts: f64, reason: &str) -> Result<()> {
        self.db.execute("INSERT INTO pickups (ts, reason) VALUES (?1, ?2)", params![ts, reason]).map_err(err)?;
        Ok(())
    }

    pub fn set_meta(&mut self, values: &[(&str, Option<String>)]) -> Result<()> {
        let tx = self.db.transaction().map_err(err)?;
        for (key, value) in values {
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value]).map_err(err)?;
        }
        tx.commit().map_err(err)
    }

    pub fn mark_pushed(&self, day: &str, sent: f64, final_: bool) -> Result<()> {
        self.db
            .execute("INSERT OR REPLACE INTO pushes (day, sent, final) VALUES (?1, ?2, ?3)", params![day, sent, final_ as i64])
            .map_err(err)?;
        Ok(())
    }

    // ---- reads ----
    pub fn meta(&self, key: &str) -> Option<String> {
        self.db
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get::<_, Option<String>>(0))
            .optional()
            .ok()
            .flatten()
            .flatten()
    }

    /// Sessions overlapping [start, end). A session the daemon is still in runs up to now.
    pub fn sessions(&self, start: f64, end: f64, now: f64) -> Result<Vec<Session>> {
        let mut stmt = self
            .db
            .prepare_cached("SELECT app, title, start, end, open FROM sessions WHERE end > ?1 AND start < ?2 ORDER BY start")
            .map_err(err)?;
        let rows = stmt
            .query_map(params![start, end], |r| {
                let stop: f64 = r.get(3)?;
                let open: i64 = r.get(4)?;
                // the daemon's heartbeat is behind by up to 15 s
                let stop = if open != 0 && (0.0..90.0).contains(&(now - stop)) { now } else { stop };
                Ok(Session { app: r.get(0)?, title: r.get(1)?, start: r.get(2)?, end: stop })
            })
            .map_err(err)?;
        rows.collect::<std::result::Result<_, _>>().map_err(err)
    }

    pub fn pickups(&self, start: f64, end: f64) -> Result<Vec<(f64, String)>> {
        let mut stmt =
            self.db.prepare_cached("SELECT ts, reason FROM pickups WHERE ts >= ?1 AND ts < ?2 ORDER BY ts").map_err(err)?;
        let rows = stmt.query_map(params![start, end], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?;
        rows.collect::<std::result::Result<_, _>>().map_err(err)
    }

    /// When tracking began: the first session or pickup (a machine can be locked all day).
    pub fn first_record(&self) -> Option<f64> {
        self.db
            .query_row(
                "SELECT MIN(t) FROM (SELECT MIN(start) AS t FROM sessions UNION ALL SELECT MIN(ts) FROM pickups)",
                [],
                |r| r.get::<_, Option<f64>>(0),
            )
            .ok()
            .flatten()
    }

    pub fn pushed(&self, day: &str) -> Option<(f64, bool)> {
        self.db
            .query_row("SELECT sent, final FROM pushes WHERE day = ?1", [day], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)))
            .optional()
            .ok()
            .flatten()
    }
}

fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
