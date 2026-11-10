//! The state machine: which app has focus, and whether the person is actually there.
//!
//! Time counts only while the screen is on, unlocked and not idle. Coming back from a locked,
//! blanked or suspended screen is a pickup (like waking an iPhone); coming back from a short idle
//! with the screen still on is not.

use crate::config::Config;
use crate::store::Store;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Away {
    Start,
    Suspended,
    Locked,
    Blanked,
}

impl Away {
    fn reason(self) -> &'static str {
        match self {
            Away::Start => "login",
            Away::Suspended => "wake",
            Away::Locked => "unlock",
            Away::Blanked => "screen-on",
        }
    }
}

struct Open {
    id: i64,
    app: String,
    title: Option<String>,
    start: f64,
}

pub struct Tracker<'a> {
    pub store: &'a mut Store,
    config: &'a Config,
    app: Option<String>,
    title: Option<String>,
    idle: bool,
    locked: bool,
    blanked: bool,
    away: Option<Away>,
    session: Option<Open>,
}

fn check<T>(result: Result<T, String>) -> Option<T> {
    result.map_err(|e| crate::log(&e)).ok()
}

impl<'a> Tracker<'a> {
    /// A fresh login counts as a pickup; a quick daemon restart doesn't.
    pub fn new(store: &'a mut Store, config: &'a Config, restarted: bool) -> Self {
        Tracker {
            store,
            config,
            app: None,
            title: None,
            idle: false,
            locked: false,
            blanked: false,
            away: if restarted { None } else { Some(Away::Start) },
            session: None,
        }
    }

    fn active(&self) -> bool {
        !(self.idle || self.locked || self.blanked)
    }

    pub fn state(&self) -> &'static str {
        if self.locked {
            "locked"
        } else if self.blanked {
            "screen-off"
        } else if self.idle {
            "idle"
        } else {
            "active"
        }
    }

    fn label(&self) -> Option<(String, Option<String>)> {
        let app = self.app.clone().unwrap_or_else(|| "desktop".into());
        if self.config.ignored(&app) {
            return None;
        }
        Some((app, if self.config.track_titles { self.title.clone() } else { None }))
    }

    fn close(&mut self, end: f64) {
        if let Some(open) = self.session.take() {
            check(self.store.close_session(open.id, end, self.config.min_session));
        }
    }

    fn write_meta(&mut self, state: &str, now: f64) {
        let app = self.session.as_ref().map(|s| s.app.clone());
        check(self.store.set_meta(&[("state", Some(state.into())), ("app", app), ("last_beat", Some(now.to_string()))]));
    }

    fn update(&mut self, now: f64, end_at: Option<f64>) {
        if !self.active() {
            self.close(end_at.unwrap_or(now));
        } else {
            let label = self.label();
            if let Some(open) = &self.session
                && label.as_ref().is_none_or(|(a, t)| (a, t) != (&open.app, &open.title))
            {
                self.close(now);
            }
            if let Some((app, title)) = label
                && self.session.is_none()
                && let Some(id) = check(self.store.open_session(&app, title.as_deref(), now))
            {
                self.session = Some(Open { id, app, title, start: now });
            }
            if let Some(away) = self.away.take() {
                check(self.store.add_pickup(now, away.reason()));
            }
        }
        self.write_meta(self.state(), now);
    }

    // ---- inputs ----
    pub fn focus(&mut self, app: &str, title: &str, now: f64) {
        self.app = (!app.is_empty()).then(|| app.to_string());
        self.title = (!title.is_empty()).then(|| title.to_string());
        self.update(now, None);
    }

    pub fn set_idle(&mut self, idle: bool, now: f64) {
        if idle == self.idle {
            return;
        }
        self.idle = idle;
        let mut end_at = None;
        if idle
            && self.config.trim_idle
            && let Some(open) = &self.session
        {
            // Nobody touched anything for idle_timeout seconds: that time wasn't use either.
            end_at = Some(open.start.max(now - self.config.idle_timeout));
        }
        self.update(now, end_at);
    }

    pub fn set_locked(&mut self, locked: bool, now: f64) {
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        if locked {
            self.away = Some(Away::Locked);
        }
        self.update(now, None);
    }

    pub fn set_blanked(&mut self, blanked: bool, now: f64) {
        if blanked == self.blanked {
            return;
        }
        self.blanked = blanked;
        if blanked && self.away.is_none() {
            self.away = Some(Away::Blanked);
        }
        self.update(now, None);
    }

    /// The machine slept from `since` until `now`: nothing in between counts.
    pub fn suspended(&mut self, since: f64, now: f64) {
        self.close(since);
        self.away = Some(Away::Suspended);
        self.update(now, None);
    }

    pub fn heartbeat(&mut self, now: f64) {
        if let Some(open) = &self.session {
            check(self.store.extend_session(open.id, now));
        }
        self.write_meta(self.state(), now);
    }

    pub fn stop(&mut self, now: f64) {
        self.close(now);
        self.write_meta("stopped", now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    fn rows(store: &Store) -> Vec<(String, f64, f64)> {
        let mut stmt = store.db.prepare("SELECT app, start, end FROM sessions ORDER BY start").unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    }

    fn pickups(store: &Store) -> Vec<String> {
        store.pickups(0.0, 1e12).unwrap().into_iter().map(|(_, r)| r).collect()
    }

    fn r(app: &str, a: f64, b: f64) -> (String, f64, f64) {
        (app.into(), a, b)
    }

    #[test]
    fn focus_idle_lock_and_pickups() {
        let (_dir, config, mut store) = setup();
        let t0 = at(9, 0, day());
        let mut t = Tracker::new(&mut store, &config, false);
        t.focus("kitty", "vim", t0);
        t.focus("firefox", "news", t0 + 100.0);
        t.set_idle(true, t0 + 400.0); // idle_timeout 180: the last 180 s weren't use
        t.set_idle(false, t0 + 500.0); // short idle with the screen on: not a pickup
        t.set_locked(true, t0 + 600.0);
        t.set_locked(false, t0 + 900.0);
        t.stop(t0 + 1000.0);
        assert_eq!(
            rows(&store),
            vec![
                r("kitty", t0, t0 + 100.0),
                r("firefox", t0 + 100.0, t0 + 220.0),
                r("firefox", t0 + 500.0, t0 + 600.0),
                r("firefox", t0 + 900.0, t0 + 1000.0)
            ]
        );
        assert_eq!(pickups(&store), ["login", "unlock"]);
        assert_eq!(store.meta("state").as_deref(), Some("stopped"));
    }

    #[test]
    fn titles_only_when_enabled_and_flashes_dropped() {
        let (_dir, config, mut store) = setup();
        let t0 = at(9, 0, day());
        let mut t = Tracker::new(&mut store, &config, true);
        t.focus("kitty", "secret project", t0);
        t.focus("org.vinegarhq.Sober", "Roblox", t0 + 10.0); // 0.3 s focus flash
        t.focus("kitty", "secret project", t0 + 10.3);
        t.stop(t0 + 20.0);
        let titles: Vec<Option<String>> = store
            .db
            .prepare("SELECT title FROM sessions")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(titles, [None, None]);
        assert!(pickups(&store).is_empty()); // a quick daemon restart is not a pickup
    }

    #[test]
    fn suspend_blank_and_ignore() {
        let (_dir, mut config, mut store) = setup();
        config.ignore = vec!["screensaver".into()];
        let t0 = at(9, 0, day());
        let mut t = Tracker::new(&mut store, &config, false);
        t.focus("kitty", "", t0);
        t.suspended(t0 + 50.0, t0 + 3600.0); // slept, woke straight to an unlocked desktop
        t.set_blanked(true, t0 + 3700.0);
        t.set_blanked(false, t0 + 3800.0);
        t.focus("screensaver", "", t0 + 3810.0);
        t.stop(t0 + 3900.0);
        assert_eq!(
            rows(&store),
            vec![r("kitty", t0, t0 + 50.0), r("kitty", t0 + 3600.0, t0 + 3700.0), r("kitty", t0 + 3800.0, t0 + 3810.0)]
        );
        assert_eq!(pickups(&store), ["login", "wake", "screen-on"]);
    }

    #[test]
    fn wake_into_lock_screen_is_one_pickup() {
        let (_dir, config, mut store) = setup();
        let t0 = at(9, 0, day());
        let mut t = Tracker::new(&mut store, &config, true);
        t.focus("kitty", "", t0);
        t.set_locked(true, t0 + 10.0);
        t.suspended(t0 + 20.0, t0 + 100.0);
        t.set_blanked(true, t0 + 110.0);
        t.set_blanked(false, t0 + 120.0);
        t.set_locked(false, t0 + 130.0);
        assert_eq!(pickups(&store), ["wake"]);
    }
}
