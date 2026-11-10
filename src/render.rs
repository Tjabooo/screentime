//! Terminal rendering of the Today and Week screens.

use chrono::NaiveDate;
use serde_json::Value;

/// 3h 42m · 12m · 40s · 0m
pub fn duration(seconds: i64) -> String {
    if seconds < 60 {
        return if seconds > 0 { format!("{seconds}s") } else { "0m".into() };
    }
    let (hours, minutes) = (seconds / 60 / 60, seconds / 60 % 60);
    if hours > 0 { format!("{hours}h {minutes:02}m") } else { format!("{minutes}m") }
}

fn color(category: &str) -> &'static str {
    match category {
        "Social" => "38;5;211",
        "Games" => "38;5;209",
        "Entertainment" => "38;5;141",
        "Web" => "38;5;75",
        "Development" => "38;5;79",
        "Productivity" => "38;5;221",
        "Creativity" => "38;5;114",
        "Education" => "38;5;180",
        "Utilities" => "38;5;248",
        "Other" => "38;5;245",
        _ => "0",
    }
}

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

struct Paint(bool);

impl Paint {
    fn p(&self, text: &str, code: &str) -> String {
        if self.0 { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }
}

fn column(value: i64, top: i64) -> char {
    if value == 0 || top == 0 {
        return ' ';
    }
    BLOCKS[(8.0 * value as f64 / top as f64).round_ties_even().clamp(1.0, 8.0) as usize]
}

fn bar(value: i64, top: i64, width: usize) -> String {
    let filled = if top > 0 { (width as f64 * value as f64 / top as f64).round_ties_even() as usize } else { 0 };
    let n = filled.max(usize::from(value > 0));
    "━".repeat(n) + &" ".repeat(width.saturating_sub(n))
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The key of the biggest value in a {name: seconds} object (first wins on ties), or Other.
fn lead(map: &Value) -> String {
    let mut best: Option<(&String, i64)> = None;
    for (k, v) in map.as_object().into_iter().flatten() {
        if best.is_none_or(|(_, b)| i(v) > b) {
            best = Some((k, i(v)));
        }
    }
    best.map(|(k, _)| k.clone()).unwrap_or_else(|| "Other".into())
}

fn trunc(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn most_used(out: &mut Vec<String>, apps: &[Value], paint: &Paint) {
    if apps.is_empty() {
        return;
    }
    out.push(String::new());
    out.push(format!("  {}", paint.p("MOST USED", "1")));
    let best = i(&apps[0]["seconds"]);
    for (n, a) in apps.iter().take(8).enumerate() {
        let secs = i(&a["seconds"]);
        out.push(format!(
            "  {:>2}  {:<22} {:>7}  {}",
            n + 1,
            trunc(s(&a["name"]), 22),
            duration(secs),
            paint.p(&bar(secs, best, 18), color(s(&a["category"])))
        ));
    }
}

pub fn day(summary: &Value, color_on: bool, label: &str) -> String {
    let paint = Paint(color_on);
    let mut out =
        vec![format!("{}  {}  ·  {label}", paint.p("SCREEN TIME", "1"), paint.p(s(&summary["device"]), "2")), String::new()];
    let word = if label.starts_with("Today") { "today" } else { "total" };
    out.push(format!("  {}  {}", paint.p(&duration(i(&summary["total"])), "1"), paint.p(word, "2")));
    out.push(String::new());
    let hourly = arr(&summary["hourly"]);
    let top = hourly.iter().map(|h| i(&h["seconds"])).max().unwrap_or(0);
    let cols: String = hourly
        .iter()
        .map(|h| paint.p(&column(i(&h["seconds"]), top).to_string().repeat(2), color(&lead(&h["categories"]))))
        .collect();
    out.push(format!("  {cols}"));
    out.push(format!("  {}", paint.p("00          06          12          18        ", "2")));
    let cats: Vec<String> = arr(&summary["categories"])
        .iter()
        .take(4)
        .map(|c| format!("{} {} {}", paint.p("●", color(s(&c["name"]))), s(&c["name"]), duration(i(&c["seconds"]))))
        .collect();
    if !cats.is_empty() {
        out.push(String::new());
        out.push(format!("  {}", cats.join("   ")));
    }
    most_used(&mut out, arr(&summary["apps"]), &paint);
    out.push(String::new());
    let first = summary["first_pickup"].as_str().map(|f| format!("   first at {f}")).unwrap_or_default();
    out.push(format!("  {}  {}{first}", paint.p("PICKUPS", "1"), i(&summary["pickups"])));
    let after = arr(&summary["after_pickup"]);
    if !after.is_empty() {
        let list: Vec<String> = after.iter().take(3).map(|a| format!("{} {}", s(&a["name"]), i(&a["count"]))).collect();
        out.push(format!("  first used after pickup: {}", list.join(", ")));
    }
    let ls = &summary["longest_session"];
    if ls.is_object() {
        let when = format!("{}–{}, mostly {}", s(&ls["start"]), s(&ls["end"]), s(&ls["name"]));
        out.push(format!("  {}  {}  {}", paint.p("LONGEST SESSION", "1"), duration(i(&ls["seconds"])), paint.p(&when, "2")));
    }
    out.join("\n")
}

pub fn week(w: &Value, color_on: bool) -> String {
    let paint = Paint(color_on);
    let date =
        |v: &Value| NaiveDate::parse_from_str(s(v), "%Y-%m-%d").map(|d| d.format("%-d %b").to_string()).unwrap_or_default();
    let mut out = vec![
        format!(
            "{}  {}  ·  {} – {}",
            paint.p("SCREEN TIME", "1"),
            paint.p(s(&w["device"]), "2"),
            date(&w["start"]),
            date(&w["end"])
        ),
        String::new(),
    ];
    let change = match w["change_percent"].as_i64() {
        Some(c) => {
            let arrow = if c > 0 {
                '↑'
            } else if c < 0 {
                '↓'
            } else {
                '='
            };
            paint.p(&format!("   {arrow} {}% from last week", c.abs()), "2")
        }
        None => String::new(),
    };
    out.push(format!("  {}  {}{change}", paint.p(&duration(i(&w["average"])), "1"), paint.p("daily average", "2")));
    out.push(String::new());
    let days = arr(&w["days"]);
    let top = days.iter().map(|d| i(&d["total"])).max().unwrap_or(0);
    for d in days {
        if d["total"].is_null() {
            out.push(format!("  {}  {}", s(&d["weekday"]), paint.p("·", "2")));
            continue;
        }
        let total = i(&d["total"]);
        out.push(format!(
            "  {}  {} {:>7}",
            s(&d["weekday"]),
            paint.p(&bar(total, top, 24), color(&lead(&d["categories"]))),
            duration(total)
        ));
    }
    out.push(String::new());
    out.push(format!("  Total {}", duration(i(&w["total"]))));
    most_used(&mut out, arr(&w["apps"]), &paint);
    out.push(String::new());
    out.push(format!("  {}  {}   {} a day", paint.p("PICKUPS", "1"), i(&w["pickups"]), i(&w["pickups_average"])));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let got: Vec<String> = [0, 40, 600, 13320].into_iter().map(duration).collect();
        assert_eq!(got, ["0m", "40s", "10m", "3h 42m"]);
    }
}
