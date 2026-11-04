"""SQLite storage: app sessions, pickups and what has been pushed."""
import sqlite3
import time
from pathlib import Path

SCHEMA = """
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
"""


class Store:
    def __init__(self, path, readonly=False):
        self.path = Path(path)
        if readonly:
            if not self.path.exists():
                raise FileNotFoundError(f'No Screen Time data yet at {self.path} (is the daemon running?).')
            self.db = sqlite3.connect(f'file:{self.path}?mode=ro', uri=True, timeout=5)
        else:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.db = sqlite3.connect(self.path, timeout=10)
            self.db.execute('PRAGMA journal_mode=WAL')
            self.db.executescript(SCHEMA)
            self.path.chmod(0o600)
        self.db.row_factory = sqlite3.Row

    def close(self):
        self.db.close()

    # ---- writes (daemon) ----
    def open_session(self, app, title, start):
        with self.db:
            return self.db.execute('INSERT INTO sessions (app, title, start, end, open) VALUES (?, ?, ?, ?, 1)',
                                   (app, title, start, start)).lastrowid

    def extend_session(self, session_id, end):
        with self.db:
            self.db.execute('UPDATE sessions SET end = MAX(start, ?) WHERE id = ?', (end, session_id))

    def close_session(self, session_id, end, min_length=0.0):
        with self.db:
            self.db.execute('UPDATE sessions SET end = MAX(start, ?), open = 0 WHERE id = ?', (end, session_id))
            self.db.execute('DELETE FROM sessions WHERE id = ? AND end - start < ?', (session_id, min_length))

    def close_stale(self):
        """Sessions left open by a crash end at their last heartbeat."""
        with self.db:
            self.db.execute('UPDATE sessions SET open = 0 WHERE open = 1')

    def add_pickup(self, ts, reason):
        with self.db:
            self.db.execute('INSERT INTO pickups (ts, reason) VALUES (?, ?)', (ts, reason))

    def set_meta(self, **values):
        with self.db:
            self.db.executemany('INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)',
                                [(k, None if v is None else str(v)) for k, v in values.items()])

    def mark_pushed(self, day, sent, final):
        with self.db:
            self.db.execute('INSERT OR REPLACE INTO pushes (day, sent, final) VALUES (?, ?, ?)', (day, sent, int(final)))

    # ---- reads ----
    def meta(self, key, default=None):
        row = self.db.execute('SELECT value FROM meta WHERE key = ?', (key,)).fetchone()
        return default if row is None or row[0] is None else row[0]

    def sessions(self, start, end, now=None):
        """Sessions overlapping [start, end). A session the daemon is still in runs up to now."""
        now = time.time() if now is None else now
        rows = self.db.execute('SELECT app, title, start, end, open FROM sessions WHERE end > ? AND start < ? ORDER BY start',
                               (start, end)).fetchall()
        out = []
        for row in rows:
            stop = row['end']
            if row['open'] and 0 <= now - stop < 90:  # the daemon's heartbeat is behind by up to 15 s
                stop = now
            out.append((row['app'], row['title'], row['start'], stop))
        return out

    def pickups(self, start, end):
        return [(r['ts'], r['reason']) for r in self.db.execute(
            'SELECT ts, reason FROM pickups WHERE ts >= ? AND ts < ? ORDER BY ts', (start, end))]

    def first_record(self):
        row = self.db.execute('SELECT MIN(start) FROM sessions').fetchone()
        return row[0]

    def pushed(self, day):
        row = self.db.execute('SELECT sent, final FROM pushes WHERE day = ?', (day,)).fetchone()
        return (row['sent'], bool(row['final'])) if row else None
