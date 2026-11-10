//! iPhone-style Screen Time numbers from the stored sessions.

use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveTime, TimeZone, Timelike};
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::store::{Session, Store};

/// App sessions this close together are one sitting ("longest session").
const MERGE_GAP: f64 = 5.0;

/// Python's round(): halves go to the even neighbour.
pub fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Local wall-clock time on `day` as a Unix timestamp (DST gaps resolve forward, overlaps to the first).
pub fn local_ts(day: NaiveDate, hour: u32, minute: u32) -> f64 {
    let naive = day.and_time(NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default());
    let dt = match Local.from_local_datetime(&naive) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t,
        LocalResult::None => {
            Local.from_local_datetime(&(naive + Duration::hours(1))).earliest().unwrap_or_else(|| Local.from_utc_datetime(&naive))
        }
    };
    dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_nanos()) / 1e9
}

pub fn local(ts: f64) -> DateTime<Local> {
    let secs = ts.floor();
    let nanos = ((ts - secs) * 1e9) as u32;
    Local.timestamp_opt(secs as i64, nanos.min(999_999_999)).earliest().unwrap_or_else(Local::now)
}

pub fn date_of(ts: f64) -> NaiveDate {
    local(ts).date_naive()
}

pub fn day_start(day: NaiveDate) -> f64 {
    local_ts(day, 0, 0)
}

pub fn day_bounds(day: NaiveDate) -> (f64, f64) {
    (day_start(day), day_start(day + Duration::days(1)))
}

fn clip(sessions: Vec<Session>, start: f64, end: f64) -> Vec<Session> {
    sessions
        .into_iter()
        .filter_map(|mut s| {
            s.start = s.start.max(start);
            s.end = s.end.min(end);
            (s.end > s.start).then_some(s)
        })
        .collect()
}

fn hhmm(ts: f64) -> String {
    local(ts).format("%H:%M").to_string()
}

fn offset_minutes(ts: f64) -> i64 {
    round(f64::from(local(ts).offset().local_minus_utc()) / 60.0)
}

/// A Counter that remembers insertion order, so `most_common` ties come out like Python's.
#[derive(Default)]
pub struct Tally<T: Copy + Into<f64>>(pub Vec<(String, T)>);

impl<T: Copy + Into<f64> + std::ops::AddAssign> Tally<T> {
    pub fn add(&mut self, key: &str, value: T) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v += value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    pub fn most_common(&self) -> Vec<(String, T)> {
        let mut out = self.0.clone();
        out.sort_by(|a, b| b.1.into().total_cmp(&a.1.into())); // stable: ties keep first-seen order
        out
    }

    pub fn top(&self) -> Option<String> {
        self.most_common().into_iter().next().map(|(k, _)| k)
    }
}

pub fn daemon_state(store: &Store, now: f64) -> Value {
    let beat: f64 = store.meta("last_beat").and_then(|b| b.parse().ok()).unwrap_or(0.0);
    json!({
        "running": (0.0..60.0).contains(&(now - beat)),
        "state": store.meta("state"),
        "app": store.meta("app"),
        "last_beat": if beat != 0.0 { json!(beat) } else { Value::Null },
    })
}

struct Sitting {
    start: f64,
    end: f64,
    apps: Tally<f64>,
}

/// Everything the Today screen shows, for one calendar day.
pub fn day_summary(store: &Store, config: &Config, day: NaiveDate, now: f64) -> Result<Value, String> {
    let (start, end) = day_bounds(day);
    let sessions = clip(store.sessions(start, end, now)?, start, end.min(now));
    let pickups = store.pickups(start, end)?;
    let hours: Vec<f64> = (0..24).map(|h| local_ts(day, h, 0)).chain([end]).collect();

    let mut apps = Tally::<f64>::default();
    let mut hourly = [0.0f64; 24];
    let mut hourly_cat: Vec<Tally<f64>> = (0..24).map(|_| Tally::default()).collect();
    for s in &sessions {
        apps.add(&s.app, s.end - s.start);
        let category = config.category(&s.app);
        for h in 0..24 {
            let overlap = s.end.min(hours[h + 1]) - s.start.max(hours[h]);
            if overlap > 0.0 {
                hourly[h] += overlap;
                hourly_cat[h].add(&category, overlap);
            }
        }
    }
    let mut categories = Tally::<f64>::default();
    for (app, seconds) in &apps.0 {
        categories.add(&config.category(app), *seconds);
    }

    // A sitting: app sessions back to back (focus changes don't end it, being away does).
    let mut sittings: Vec<Sitting> = Vec::new();
    for s in &sessions {
        match sittings.last_mut() {
            Some(last) if s.start - last.end <= MERGE_GAP => {
                last.end = last.end.max(s.end);
                last.apps.add(&s.app, s.end - s.start);
            }
            _ => {
                let mut apps = Tally::default();
                apps.add(&s.app, s.end - s.start);
                sittings.push(Sitting { start: s.start, end: s.end, apps });
            }
        }
    }
    // max() keeps the first of equal maxima, like Python's max(key=...)
    let longest = sittings.iter().fold(None::<&Sitting>, |best, s| match best {
        Some(b) if s.end - s.start <= b.end - b.start => Some(b),
        _ => Some(s),
    });

    let mut pickups_hourly = [0u32; 24];
    let mut after = Tally::<u32>::default();
    for (ts, _) in &pickups {
        pickups_hourly[local(*ts).hour() as usize] += 1;
        if let Some(first) = sessions.iter().find(|s| s.start >= ts - 1.0) {
            after.add(&first.app, 1);
        }
    }

    let app_entry = |app: &str, seconds: f64| json!({"app": app, "name": config.name(app), "category": config.category(app), "seconds": round(seconds)});
    let total: f64 = apps.0.iter().map(|(_, s)| s).sum();
    let hourly: Vec<Value> = (0..24)
        .map(|h| {
            let cats: Map<String, Value> = hourly_cat[h].most_common().into_iter().map(|(c, s)| (c, json!(round(s)))).collect();
            json!({"hour": h, "seconds": round(hourly[h]), "categories": cats})
        })
        .collect();
    Ok(json!({
        "version": 1,
        "device": config.device_name(),
        "date": day.to_string(),
        "generated": round(now),
        "utc_offset": offset_minutes(now.min(end - 1.0)),
        "complete": now >= end,
        "total": round(total),
        "apps": apps.most_common().into_iter().map(|(a, s)| app_entry(&a, s)).collect::<Vec<_>>(),
        "categories": categories.most_common().into_iter().map(|(c, s)| json!({"name": c, "seconds": round(s)})).collect::<Vec<_>>(),
        "hourly": hourly,
        "pickups": pickups.len(),
        "pickups_hourly": pickups_hourly,
        "first_pickup": pickups.first().map(|p| hhmm(p.0)),
        "first_pickup_ts": pickups.first().map(|p| round(p.0)),
        "after_pickup": after.most_common().into_iter().take(10)
            .map(|(a, n)| json!({"app": a, "name": config.name(&a), "count": n})).collect::<Vec<_>>(),
        "longest_session": longest.map(|l| {
            let app = l.apps.top().unwrap_or_default();
            let name = config.name(&app);
            json!({"seconds": round(l.end - l.start), "start": hhmm(l.start), "end": hhmm(l.end), "app": app, "name": name})
        }),
        "sittings": sittings.len(),
        "titles": config.track_titles,
    }))
}

pub fn week_start(day: NaiveDate, config: &Config) -> NaiveDate {
    let wd = i64::from(day.weekday().num_days_from_monday());
    let offset = if config.week_start == "sunday" { (wd + 1) % 7 } else { wd };
    day - Duration::days(offset)
}

/// Days that count towards an average: already started, and not before tracking began.
fn tracked_days(days: &[NaiveDate], today: NaiveDate, first_day: Option<NaiveDate>) -> Vec<NaiveDate> {
    days.iter().copied().filter(|d| *d <= today && first_day.is_none_or(|f| *d >= f)).collect()
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

/// The week containing `day`: daily average vs last week, per-day bars and the most used apps.
pub fn week_summary(store: &Store, config: &Config, day: NaiveDate, now: f64) -> Result<Value, String> {
    let today = date_of(now);
    let first_day = store.first_record().map(date_of);
    let start = week_start(day, config);
    let days: Vec<NaiveDate> = (0..7).map(|i| start + Duration::days(i)).collect();
    let mut summaries = Vec::new();
    for d in days.iter().filter(|d| **d <= today) {
        summaries.push((*d, day_summary(store, config, *d, now)?));
    }
    let summary = |d: NaiveDate| summaries.iter().find(|(x, _)| *x == d).map(|(_, s)| s);

    let (mut apps, mut categories, mut pickups) = (Tally::<f64>::default(), Tally::<f64>::default(), 0i64);
    for (_, s) in &summaries {
        for a in s["apps"].as_array().into_iter().flatten() {
            apps.add(a["app"].as_str().unwrap_or(""), int(&a["seconds"]) as f64);
        }
        for c in s["categories"].as_array().into_iter().flatten() {
            categories.add(c["name"].as_str().unwrap_or(""), int(&c["seconds"]) as f64);
        }
        pickups += int(&s["pickups"]);
    }
    let counted = tracked_days(&days, today, first_day);
    let total: i64 = summaries.iter().map(|(_, s)| int(&s["total"])).sum();
    let average = if counted.is_empty() { 0.0 } else { total as f64 / counted.len() as f64 };

    let last_days: Vec<NaiveDate> = days.iter().map(|d| *d - Duration::days(7)).collect();
    let last_counted = tracked_days(&last_days, today, first_day);
    let mut last_total = 0.0;
    for d in &last_counted {
        let (a, b) = day_bounds(*d);
        last_total += clip(store.sessions(a, b, now)?, a, b.min(now)).iter().map(|s| s.end - s.start).sum::<f64>();
    }
    let last_average = (!last_counted.is_empty()).then(|| last_total / last_counted.len() as f64);
    let change = last_average.filter(|l| *l != 0.0).map(|l| round(100.0 * (average - l) / l));

    let mut longest: Option<Value> = None;
    for (d, s) in &summaries {
        let ls = &s["longest_session"];
        if ls.is_object() && longest.as_ref().is_none_or(|b| int(&ls["seconds"]) > int(&b["seconds"])) {
            let mut entry = ls.clone();
            entry["date"] = json!(d.to_string());
            longest = Some(entry);
        }
    }
    let day_rows: Vec<Value> = days
        .iter()
        .map(|d| {
            let s = summary(*d);
            let cats: Map<String, Value> = s
                .and_then(|s| s["categories"].as_array())
                .into_iter()
                .flatten()
                .map(|c| (c["name"].as_str().unwrap_or("").to_string(), c["seconds"].clone()))
                .collect();
            json!({
                "date": d.to_string(),
                "weekday": d.format("%a").to_string(),
                "total": s.map(|s| s["total"].clone()),
                "pickups": s.map(|s| s["pickups"].clone()),
                "categories": cats,
            })
        })
        .collect();
    Ok(json!({
        "version": 1,
        "device": config.device_name(),
        "start": start.to_string(),
        "end": days[6].to_string(),
        "generated": round(now),
        "total": total,
        "average": round(average),
        "last_week_average": last_average.map(round),
        "change_percent": change,
        "days": day_rows,
        "apps": apps.most_common().into_iter()
            .map(|(a, s)| json!({"app": a, "name": config.name(&a), "category": config.category(&a), "seconds": s as i64})).collect::<Vec<_>>(),
        "categories": categories.most_common().into_iter().map(|(c, s)| json!({"name": c, "seconds": s as i64})).collect::<Vec<_>>(),
        "pickups": pickups,
        "pickups_average": if counted.is_empty() { 0 } else { round(pickups as f64 / counted.len() as f64) },
        "longest_session": longest,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn day_summary_numbers() {
        let (_dir, config, mut store) = setup();
        let d = day();
        let next = d + Duration::days(1);
        session(&mut store, "kitty", at(8, 50, d), at(9, 20, d)); // crosses an hour
        session(&mut store, "firefox", at(9, 20, d), at(9, 30, d)); // same sitting
        session(&mut store, "discord", at(14, 0, d), at(14, 5, d));
        session(&mut store, "kitty", at(23, 50, d), at(0, 10, next)); // crosses midnight
        for ts in [at(8, 50, d), at(14, 0, d), at(23, 50, d)] {
            store.add_pickup(ts, "unlock").unwrap();
        }
        let s = day_summary(&store, &config, d, at(12, 0, next)).unwrap();
        assert_eq!(s["total"], json!(30 * 60 + 10 * 60 + 5 * 60 + 10 * 60));
        assert_eq!(s["apps"][0], json!({"app": "kitty", "name": "Kitty", "category": "Development", "seconds": 2400}));
        assert_eq!(s["categories"][0], json!({"name": "Development", "seconds": 2400}));
        assert_eq!(s["hourly"][8]["seconds"], json!(600));
        assert_eq!(s["hourly"][9], json!({"hour": 9, "seconds": 1800, "categories": {"Development": 1200, "Web": 600}}));
        assert_eq!(s["hourly"][23]["seconds"], json!(600));
        assert_eq!((s["pickups"].clone(), s["first_pickup"].clone()), (json!(3), json!("08:50")));
        assert_eq!(s["pickups_hourly"][14], json!(1));
        assert_eq!(s["after_pickup"][0], json!({"app": "kitty", "name": "Kitty", "count": 2}));
        assert_eq!(
            s["longest_session"],
            json!({"seconds": 2400, "start": "08:50", "end": "09:30", "app": "kitty", "name": "Kitty"})
        );
        assert_eq!(s["sittings"], json!(3));
        assert_eq!(s["complete"], json!(true));
        assert!(crate::render::day(&s, false, "Today").contains("MOST USED"));
    }

    #[test]
    fn open_session_counts_up_to_now() {
        let (_dir, config, store) = setup();
        let d = day();
        store.open_session("kitty", None, at(10, 0, d)).unwrap();
        let s = day_summary(&store, &config, d, at(10, 0, d) + 30.0).unwrap();
        assert_eq!(s["total"], json!(30));
        assert_eq!(s["complete"], json!(false));
        let stale = day_summary(&store, &config, d, at(10, 0, d) + 300.0).unwrap();
        assert_eq!(stale["total"], json!(0));
    }

    #[test]
    fn week_average_and_change() {
        let (_dir, config, mut store) = setup();
        let d = day();
        let last_monday = d - Duration::days(9);
        for i in 0..7 {
            let x = last_monday + Duration::days(i);
            session(&mut store, "kitty", at(10, 0, x), at(11, 0, x));
        }
        for i in 0..3 {
            let x = d - Duration::days(2 - i);
            session(&mut store, "firefox", at(10, 0, x), at(11, 30, x));
        }
        let w = week_summary(&store, &config, d, at(23, 0, d)).unwrap();
        assert_eq!((w["start"].clone(), w["end"].clone()), (json!("2026-11-02"), json!("2026-11-08")));
        assert_eq!(w["average"], json!(5400));
        assert_eq!(w["last_week_average"], json!(3600));
        assert_eq!(w["change_percent"], json!(50));
        let totals: Vec<Value> = w["days"].as_array().unwrap().iter().map(|x| x["total"].clone()).collect();
        assert_eq!(totals, [json!(5400), json!(5400), json!(5400), Value::Null, Value::Null, Value::Null, Value::Null]);
        assert_eq!(w["apps"][0]["app"], json!("firefox"));
        assert!(crate::render::week(&w, false).contains("from last week"));
    }

    #[test]
    fn week_counts_only_tracked_days() {
        let (_dir, mut config, mut store) = setup();
        let d = day();
        session(&mut store, "kitty", at(10, 0, d), at(12, 0, d)); // first day ever is today (Wednesday)
        let w = week_summary(&store, &config, d, at(23, 0, d)).unwrap();
        assert_eq!(w["average"], json!(7200)); // not divided by 3
        assert_eq!(w["change_percent"], Value::Null);
        config.week_start = "sunday".into();
        assert_eq!(week_summary(&store, &config, d, at(23, 0, d)).unwrap()["start"], json!("2026-11-01"));
    }
}
