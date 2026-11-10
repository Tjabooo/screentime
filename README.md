# screentime

[![CI](https://github.com/Tjabooo/screentime/actions/workflows/ci.yml/badge.svg)](https://github.com/Tjabooo/screentime/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/Tjabooo/screentime?include_prereleases&sort=semver)](https://github.com/Tjabooo/screentime/releases) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

iPhone-style Screen Time for Hyprland: a tiny daemon that tracks which app you're using, stops the clock when you're idle, locked, the screen is off or the machine sleeps, and gives you today's and this week's numbers.

```
SCREEN TIME  desktop  ·  Today, Wed 4 Nov

  4h 20m  today

                ▂▂▅▅▃▃▁▁▆▆▃▃▂▂▁▁▃▃▄▄██▆▆▃▃▂▂▅▅▇▇
  00          06          12          18

  ● Development 1h 52m   ● Web 1h 04m   ● Social 40m   ● Games 21m

  MOST USED
   1  Alacritty               1h 43m  ━━━━━━━━━━━━━━━━━━
   2  Firefox                 1h 04m  ━━━━━━━━━━━
   3  Discord                    40m  ━━━━━━━
   4  Counter-Strike 2           21m  ━━━━

  PICKUPS  23   first at 08:12
  first used after pickup: Discord 7, Firefox 5, Alacritty 4
  LONGEST SESSION  1h 32m  13:05–14:37, mostly Alacritty
```

## Features

- **Tracks the focused app** from Hyprland's event socket (`activewindow`, `workspace`), with friendly names and iOS-style categories.
- **Only counts real use.** The clock stops when you're idle (Wayland `ext-idle-notify-v1`, so a fullscreen video still counts but a "keep awake" toggle doesn't), when a screen locker runs, when every monitor is off (DPMS), and while the machine is suspended.
- **iOS numbers:** today's total, per-app and per-category time, hourly bars, pickups (wakes, unlocks, screen-on), first pickup, "first used after pickup", longest session, and the week's daily average with the change from last week.
- **Private by default:** stores the window class only. Window titles are saved only if you turn them on.
- **`--json` output** for bars, widgets, dashboards and scripts.
- **Optional push** of each day's summary to a URL with a token, queued in the database while you're offline, so a laptop catches up after sleep or once it's back home.
- **Small and cheap:** one ~5 MB Rust binary, SQLite storage (built in), a systemd user service. About 7 MB of memory and next to no CPU while running.

## Install

```sh
git clone https://github.com/Tjabooo/screentime
cd screentime
./install.sh
```

This builds the binary (`cargo build --release`), installs it to `~/.local/share/screentime/bin` and links the `screentime` command into `~/.local/bin`, writes a commented config to `~/.config/screentime/config.toml` (if you don't have one), and enables `screentime.service` for your graphical session. Run `./install.sh` again to update and `./install.sh --uninstall` to remove it (your data stays).

To install a binary built elsewhere (same CPU architecture), run `SCREENTIME_BINARY=/path/to/screentime ./install.sh`. Upgrading from the Python version (0.1) keeps your database and config; the installer removes the old Python code.

Requirements: Hyprland, systemd, and Rust 1.85+ to build (Arch: `pacman -S rust`). Your session should export `WAYLAND_DISPLAY` and `HYPRLAND_INSTANCE_SIGNATURE` to the systemd user manager (most Hyprland setups do; if not, add `exec-once = systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE`).

## Usage

```sh
screentime today                 # today, as above
screentime today --date yesterday
screentime week                  # this week: daily average vs last week, per-day bars, most used
screentime week --date 2026-10-28
screentime today --json          # everything as JSON (also: week --json)
screentime push                  # send unsent days to the push URL now
screentime config                # where the config and database live
systemctl --user status screentime
```

`today --json` gives `total`, `apps`, `categories`, `hourly` (24 entries with per-category seconds), `pickups`, `pickups_hourly`, `first_pickup`, `after_pickup`, `longest_session` and `daemon` (whether it's running and what it sees right now). `week --json` gives `average`, `last_week_average`, `change_percent`, `days`, `apps`, `categories`, `pickups` and `pickups_average`.

## Configuration

`~/.config/screentime/config.toml` (or `$SCREENTIME_CONFIG`). Every key is optional; see [`config.example.toml`](config.example.toml).

| Key | Default | |
| --- | --- | --- |
| `idle_timeout` | `180` | Seconds without input before the clock stops. |
| `trim_idle` | `true` | Also take those idle seconds back off the session. |
| `track_titles` | `false` | Store window titles as well as the app class. |
| `lockers` | `hyprlock`, `swaylock`, … | Processes that mean "locked". |
| `ignore` | `[]` | Window classes (globs) that never count. |
| `week_start` | `monday` | Or `sunday`. |
| `[categories]` | built-in list | `"glob" = "Category"`, checked before the built-in map. |
| `[names]` | built-in list | `"glob" = "Display name"`. |
| `[push] url` | off | POST each day's summary here. |
| `[push] token` / `token_file` | | Sent as `Authorization: Bearer …` (or set `$SCREENTIME_TOKEN`). |
| `[push] interval` | `600` | Seconds between pushes of today's running summary. |
| `[push] catch_up_days` | `14` | How far back unsent days are retried. |

Categories: Social, Games, Entertainment, Web, Development, Productivity, Creativity, Education, Utilities and Other.

### Push format

```http
POST <url>
Authorization: Bearer <token>
Content-Type: application/json

{"device": "desktop", "date": "2026-11-04", "final": false, "summary": { …same as `today --json`… }}
```

Today's summary is re-sent every `interval` seconds. A day counts as sent once a push succeeds after that day has ended; until then it stays queued in the database and goes out on the next successful push (after a resume, or whenever the URL answers again). Any 2xx answer is success.

## How it works

- **Focus:** a connection to Hyprland's `.socket2.sock` delivers `activewindow>>class,title`; the request socket answers `j/activewindow` and `j/monitors` (for DPMS).
- **Idle:** a tiny built-in Wayland client (no libwayland) binds `wl_seat` and `ext_idle_notifier_v1` and asks for an idle notification after `idle_timeout`. `idled` stops the clock (and, with `trim_idle`, ends the session when input stopped); `resumed` starts it again.
- **Lock, screen-off, suspend:** at most every `poll_interval` seconds (not on every Hyprland event) it looks for a locker process in `/proc` (while locked, only the locker's own entry) and asks Hyprland whether any monitor is on. A jump between `CLOCK_BOOTTIME` and `CLOCK_MONOTONIC` means the machine was suspended; that time is cut out.
- **Pickups:** coming back from locked, screen-off, suspended or a fresh login is a pickup. Coming back from a short idle with the screen still on isn't.
- **Storage:** sessions (`app`, optional `title`, `start`, `end`) and pickups in SQLite (WAL) at `~/.local/share/screentime/screentime.db`. The running session is written every 15 seconds, so a crash loses at most that.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

[MIT](LICENSE)
