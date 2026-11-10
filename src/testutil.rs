//! Shared test fixtures: a temp database, a config with device "testbox", local-time helpers.

use std::path::PathBuf;

use chrono::NaiveDate;

use crate::config::Config;
use crate::store::Store;

pub struct TempDir(pub PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 2026-11-04, a Wednesday.
pub fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
}

pub fn at(hour: u32, minute: u32, day: NaiveDate) -> f64 {
    crate::stats::local_ts(day, hour, minute)
}

pub fn setup() -> (TempDir, Config, Store) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("screentime-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let config = Config { database: dir.join("st.db").display().to_string(), device: "testbox".into(), ..Config::default() };
    let store = Store::open(&config.database_path()).unwrap();
    (TempDir(dir), config, store)
}

pub fn session(store: &mut Store, app: &str, start: f64, end: f64) {
    let id = store.open_session(app, None, start).unwrap();
    store.close_session(id, end, 0.0).unwrap();
}
