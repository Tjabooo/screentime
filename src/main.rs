//! screentime run | today | week | push | config

mod config;
mod daemon;
mod hypr;
mod push;
mod render;
mod stats;
mod store;
#[cfg(test)]
mod testutil;
mod tracker;
mod wayland;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use chrono::{Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};

use crate::store::Store;

pub fn log(message: &str) {
    eprintln!("screentime: {message}");
}

#[derive(Parser)]
#[command(name = "screentime", version, about = "iPhone-style Screen Time for Hyprland.")]
struct Cli {
    /// config file (default: ~/.config/screentime/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// run the tracking daemon (normally the systemd user service)
    Run,
    /// one day: total, apps, categories, hourly bars, pickups
    Today(View),
    /// the week: daily average vs last week, per-day bars, most used
    Week(View),
    /// send unsent days to the configured push URL now
    Push,
    /// print where the config and database are
    Config,
}

#[derive(clap::Args)]
struct View {
    /// a day in ISO form, "yesterday" (default: today)
    #[arg(long)]
    date: Option<String>,
    /// machine-readable output
    #[arg(long)]
    json: bool,
}

fn parse_day(text: Option<&str>) -> Result<NaiveDate, String> {
    let today = Local::now().date_naive();
    match text {
        None | Some("") | Some("today") => Ok(today),
        Some("yesterday") => Ok(today - Duration::days(1)),
        Some(t) => NaiveDate::parse_from_str(t, "%Y-%m-%d").map_err(|_| format!("Invalid isoformat string: '{t}'")),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<i32, String> {
    let config = config::load(cli.config.as_deref())?;
    let view = match cli.command {
        Command::Run => return daemon::run(config),
        Command::Config => {
            let path = cli.config.clone().unwrap_or_else(config::config_path);
            println!("config    {}", path.display());
            println!("database  {}", config.database_path().display());
            println!("device    {}", config.device_name());
            println!("push      {}", if config.push.url.is_empty() { "off" } else { &config.push.url });
            return Ok(0);
        }
        Command::Push => {
            if config.push.url.is_empty() {
                eprintln!("Push is off: set [push] url in the config.");
                return Ok(1);
            }
            let store = Store::open(&config.database_path())?;
            let sent = push::push_due(&store, &config, daemon::now(), &|m| eprintln!("{m}"))?;
            println!("pushed {sent} day(s)");
            return Ok(0);
        }
        Command::Today(v) => (false, v),
        Command::Week(v) => (true, v),
    };
    let (is_week, view) = view;
    let store = Store::open_readonly(&config.database_path())?;
    let now = daemon::now();
    let day = parse_day(view.date.as_deref())?;
    let mut data =
        if is_week { stats::week_summary(&store, &config, day, now)? } else { stats::day_summary(&store, &config, day, now)? };
    data["daemon"] = stats::daemon_state(&store, now);
    if view.json {
        println!("{data}");
        return Ok(0);
    }
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if is_week {
        println!("{}", render::week(&data, color));
    } else {
        let today = Local::now().date_naive();
        let label =
            if day == today { format!("Today, {}", day.format("%a %-d %b")) } else { day.format("%A %-d %B").to_string() };
        println!("{}", render::day(&data, color, &label));
    }
    if data["daemon"]["running"] != true {
        let hint = "\n  The daemon isn’t running: systemctl --user start screentime";
        println!("{}", if color { format!("\x1b[2m{hint}\x1b[0m") } else { hint.to_string() });
    }
    Ok(0)
}
