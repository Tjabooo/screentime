//! Optional push: POST each day's summary to a URL (for a dashboard such as Lilin).
//!
//! The queue is the database itself: a day stays "unsent" until a push succeeds after the day has
//! ended, so a machine that was asleep or away from home catches up on the next successful push.

use std::time::Duration;

use chrono::{Duration as Days, NaiveDate};
use serde_json::json;

use crate::config::Config;
use crate::stats::{date_of, day_bounds, day_summary};
use crate::store::Store;

/// Days to send, oldest first: unfinished pushes within `catch_up_days`, plus today.
pub fn due_days(store: &Store, config: &Config, now: f64) -> Vec<NaiveDate> {
    let today = date_of(now);
    let Some(first) = store.first_record() else { return Vec::new() };
    let oldest = date_of(first).max(today - Days::days(config.push.catch_up_days));
    let mut days = Vec::new();
    let mut day = oldest;
    while day <= today {
        if !store.pushed(&day.to_string()).is_some_and(|(_, final_)| final_) {
            days.push(day);
        }
        day += Days::days(1);
    }
    days
}

enum SendError {
    Refused(u16),
    Failed(String),
}

fn send(url: &str, token: &str, body: &serde_json::Value, timeout: f64) -> Result<(), SendError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs_f64(timeout.max(0.1))))
        .user_agent(concat!("screentime/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut req = agent.post(url).header("Content-Type", "application/json");
    if !token.is_empty() {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    match req.send(serde_json::to_vec(body).unwrap_or_default()) {
        Ok(_) => Ok(()),
        Err(ureq::Error::StatusCode(code)) => Err(SendError::Refused(code)),
        Err(e) => Err(SendError::Failed(e.to_string())),
    }
}

/// Send everything that is due. Stops at the first failure (offline): the rest waits.
pub fn push_due(store: &Store, config: &Config, now: f64, log: &dyn Fn(&str)) -> Result<usize, String> {
    let url = &config.push.url;
    if url.is_empty() {
        return Ok(0);
    }
    let token = config.push_token();
    let today = date_of(now);
    let mut sent = 0;
    for day in due_days(store, config, now) {
        let summary = day_summary(store, config, day, now)?;
        if summary["total"] == 0 && summary["pickups"] == 0 && day != today {
            store.mark_pushed(&day.to_string(), now, true)?; // nothing happened that day
            continue;
        }
        let body =
            json!({"device": config.device_name(), "date": day.to_string(), "final": summary["complete"], "summary": summary});
        match send(url, &token, &body, config.push.timeout) {
            Ok(()) => {}
            Err(SendError::Refused(code)) => {
                log(&format!("push of {day} refused: HTTP {code}"));
                return Ok(sent);
            }
            Err(SendError::Failed(why)) => {
                log(&format!("push of {day} failed ({why}); will retry"));
                return Ok(sent);
            }
        }
        let (_, end) = day_bounds(day);
        store.mark_pushed(&day.to_string(), now, now >= end)?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A one-thread HTTP server that records (Authorization, JSON body) per POST.
    type Received = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    fn server() -> (u16, Received) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let rec = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut auth, mut length) = (String::new(), 0usize);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    } else if lower.starts_with("authorization:") {
                        auth = line[14..].trim().to_string();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                rec.lock().unwrap().push((auth, serde_json::from_slice(&body).unwrap()));
                let mut s = stream;
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            }
        });
        (port, received)
    }

    #[test]
    fn queue_catches_up_and_marks_final_days() {
        let (_dir, mut config, mut store) = setup();
        let d = day();
        let yesterday = d - Days::days(1);
        session(&mut store, "kitty", at(10, 0, yesterday), at(11, 0, yesterday));
        session(&mut store, "kitty", at(10, 0, d), at(10, 30, d));
        config.push.url = "http://127.0.0.1:1/unreachable".into();
        config.push.token = "tok".into();
        config.push.timeout = 2.0;
        assert_eq!(push_due(&store, &config, at(12, 0, d), &|_| {}).unwrap(), 0); // offline
        let iso = |v: Vec<NaiveDate>| v.into_iter().map(|d| d.to_string()).collect::<Vec<_>>();
        assert_eq!(iso(due_days(&store, &config, at(12, 0, d))), ["2026-11-03", "2026-11-04"]);
        let (port, received) = server();
        config.push.url = format!("http://127.0.0.1:{port}/api/screentime");
        assert_eq!(push_due(&store, &config, at(12, 0, d), &|m| eprintln!("{m}")).unwrap(), 2);
        let got = received.lock().unwrap().clone();
        assert_eq!(got[0].0, "Bearer tok");
        let rows: Vec<_> =
            got.iter().map(|(_, b)| (b["date"].clone(), b["final"].clone(), b["summary"]["total"].clone())).collect();
        assert_eq!(rows, [(json!("2026-11-03"), json!(true), json!(3600)), (json!("2026-11-04"), json!(false), json!(1800))]);
        assert_eq!(got[0].1["device"], json!("testbox"));
        // Yesterday is done; today keeps going out until a push after midnight.
        assert_eq!(iso(due_days(&store, &config, at(13, 0, d))), ["2026-11-04"]);
    }

    #[test]
    fn a_day_with_only_a_pickup_is_still_sent() {
        let (_dir, config, store) = setup();
        store.add_pickup(at(9, 0, day()), "login").unwrap(); // logged in, then locked all day
        assert_eq!(due_days(&store, &config, at(12, 0, day())), [day()]);
    }
}
