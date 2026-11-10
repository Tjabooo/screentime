//! The tracking daemon: Hyprland focus events + Wayland idle + lock/screen-off/suspend checks.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::hypr::{self, EventStream};
use crate::log;
use crate::push::push_due;
use crate::store::Store;
use crate::tracker::Tracker;
use crate::wayland::IdleWatcher;

/// Seconds between writes of the running session's end.
const HEARTBEAT: f64 = 15.0;
const RECONNECT: Duration = Duration::from_secs(5);

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn clock(id: libc::clockid_t) -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: ts is a valid timespec to write into.
    unsafe { libc::clock_gettime(id, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

/// Grows by the time spent suspended: BOOTTIME counts sleep, MONOTONIC doesn't.
fn suspend_offset() -> f64 {
    clock(libc::CLOCK_BOOTTIME) - clock(libc::CLOCK_MONOTONIC)
}

/// Is a screen locker (hyprlock, swaylock…) running as this user? Remembers the PID it found, so
/// while locked only that one /proc entry is read instead of the whole process list.
struct LockerProbe {
    names: Vec<String>,
    uid: u32,
    known: Option<PathBuf>,
}

impl LockerProbe {
    fn new(names: &[String]) -> Self {
        // SAFETY: getuid never fails.
        LockerProbe { names: names.to_vec(), uid: unsafe { libc::getuid() }, known: None }
    }

    fn matches(&self, dir: &std::path::Path) -> bool {
        let Ok(meta) = fs::metadata(dir) else { return false };
        if meta.uid() != self.uid {
            return false;
        }
        fs::read_to_string(dir.join("comm")).is_ok_and(|c| self.names.iter().any(|n| n == c.trim_end()))
    }

    fn running(&mut self) -> bool {
        if let Some(dir) = &self.known {
            if self.matches(dir) {
                return true;
            }
            self.known = None;
        }
        let Ok(entries) = fs::read_dir("/proc") else { return false };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.as_encoded_bytes().iter().all(u8::is_ascii_digit) {
                continue;
            }
            let dir = entry.path();
            if self.matches(&dir) {
                self.known = Some(dir);
                return true;
            }
        }
        false
    }
}

/// Runs pushes on a thread so a slow network never holds up tracking.
struct Pusher {
    config: Arc<Config>,
    interval: Duration,
    next: Instant,
    thread: Option<JoinHandle<()>>,
}

impl Pusher {
    fn new(config: Arc<Config>) -> Self {
        let interval = Duration::from_secs_f64(config.push.interval.max(1.0));
        Pusher { config, interval, next: Instant::now() + Duration::from_secs(20), thread: None }
    }

    fn soon(&mut self) {
        self.next = self.next.min(Instant::now() + Duration::from_secs(15));
    }

    fn tick(&mut self) {
        if self.config.push.url.is_empty() || Instant::now() < self.next || self.thread.as_ref().is_some_and(|t| !t.is_finished())
        {
            return;
        }
        self.next = Instant::now() + self.interval;
        let config = self.config.clone();
        self.thread = Some(thread::spawn(move || {
            let result = Store::open(&config.database_path()).and_then(|store| push_due(&store, &config, now(), &|m| log(m)));
            match result {
                Ok(0) => {}
                Ok(n) => log(&format!("pushed {n} day(s)")),
                Err(e) => log(&format!("push error: {e}")), // never let a push take the daemon down
            }
        }));
    }
}

fn poll(fds: &[i32], timeout: f64) -> Vec<bool> {
    let mut pfds: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: pfds is a valid array of pollfd structs of the given length.
    let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, (timeout * 1000.0) as i32) };
    if n <= 0 {
        return vec![false; fds.len()]; // timeout or EINTR (a signal): the caller checks the stop flag
    }
    pfds.iter().map(|p| p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0).collect()
}

struct Conn {
    instance: PathBuf,
    events: EventStream,
    idle: Option<IdleWatcher>,
}

fn connect(config: &Config, tracker: &mut Tracker) -> std::io::Result<Conn> {
    let instance = hypr::instance_dir()?;
    let events = EventStream::connect(&instance)?;
    let idle = match IdleWatcher::connect(config.idle_timeout) {
        Ok(w) => Some(w),
        Err(e) => {
            log(&format!("no idle detection ({e}); only lock, screen-off and suspend stop the clock"));
            None
        }
    };
    let (app, title) = hypr::active_window(&instance)?;
    tracker.focus(&app, &title, now());
    Ok(Conn { instance, events, idle })
}

enum Lost {
    Stop,
    Error(String),
}

pub fn run(config: Config) -> Result<i32, String> {
    let config = Arc::new(config);
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, stop.clone()).map_err(|e| e.to_string())?;
    }
    let mut store = Store::open(&config.database_path())?;
    store.close_stale()?;
    let start = now();
    let last: f64 = store.meta("last_beat").and_then(|b| b.parse().ok()).unwrap_or(0.0);
    let restarted = (0.0..120.0).contains(&(start - last));
    log(&format!(
        "tracking into {} (idle after {:.0} s{}{})",
        config.database_path().display(),
        config.idle_timeout,
        if config.track_titles { ", titles on" } else { "" },
        if config.push.url.is_empty() { "" } else { ", pushing" }
    ));
    let mut tracker = Tracker::new(&mut store, &config, restarted);
    let mut pusher = Pusher::new(config.clone());
    let mut locker = LockerProbe::new(&config.lockers);
    let mut offset = suspend_offset();
    let mut last_wall = now();
    let mut last_beat = 0.0;
    let mut last_poll = 0.0;

    while !stop.load(Ordering::Relaxed) {
        let mut conn = match connect(&config, &mut tracker) {
            Ok(c) => c,
            Err(e) => {
                log(&format!("waiting for Hyprland: {e}"));
                tracker.set_blanked(true, now()); // no compositor, nothing on screen
                thread::sleep(RECONNECT);
                continue;
            }
        };
        tracker.set_blanked(false, now());
        let lost = 'outer: loop {
            if stop.load(Ordering::Relaxed) {
                break Lost::Stop;
            }
            let mut fds = vec![conn.events.fd()];
            if let Some(idle) = &conn.idle {
                fds.push(idle.fd());
            }
            let ready = poll(&fds, config.poll_interval);
            let t = now();
            let slept = suspend_offset() - offset;
            if slept > 3.0 {
                offset += slept;
                log(&format!("resumed after {slept:.0} s asleep"));
                tracker.suspended(last_wall, t);
                pusher.soon();
            }
            last_wall = t;
            if ready[0] {
                let events = match conn.events.read_events() {
                    Ok(e) => e,
                    Err(e) => break Lost::Error(e.to_string()),
                };
                for (name, data) in events {
                    match name.as_str() {
                        "activewindow" => {
                            let (app, title) = hypr::split_window(&data);
                            tracker.focus(app, title, t);
                        }
                        "workspace" | "focusedmon" | "closewindow" => match hypr::active_window(&conn.instance) {
                            Ok((app, title)) => tracker.focus(&app, &title, t),
                            Err(e) => break 'outer Lost::Error(e.to_string()),
                        },
                        _ => {}
                    }
                }
            }
            if ready.get(1).copied().unwrap_or(false)
                && let Some(idle) = conn.idle.as_mut()
            {
                match idle.read() {
                    Ok(changes) => changes.into_iter().for_each(|idled| tracker.set_idle(idled, t)),
                    Err(e) => break Lost::Error(e.to_string()),
                }
            }
            // Lock / screen-off checks at most every poll_interval, not on every Hyprland event
            // (title changes alone can fire many times a second).
            if t - last_poll >= config.poll_interval || t < last_poll {
                last_poll = t;
                if let Ok(off) = hypr::screens_off(&conn.instance) {
                    tracker.set_blanked(off, t);
                }
                tracker.set_locked(locker.running(), t);
            }
            if t - last_beat >= HEARTBEAT {
                tracker.heartbeat(t);
                last_beat = t;
            }
            pusher.tick();
        };
        match lost {
            Lost::Stop => break,
            Lost::Error(e) => {
                log(&format!("lost connection ({e}); reconnecting"));
                tracker.set_idle(false, now());
                drop(conn);
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
    tracker.stop(now());
    log("stopped");
    Ok(0)
}
