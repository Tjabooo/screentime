# Changelog

All notable changes to this project are listed here ([Keep a Changelog](https://keepachangelog.com), [SemVer](https://semver.org)).

## [Unreleased]

## [0.2.0] - 2026-11-10

- Rewritten in Rust: one static-ish binary, same commands, config, database and `--json` output
  (checked byte for byte against 0.1 on real data).
- Much lower CPU: lock and screen-off checks ran on every Hyprland event (window title changes can fire
  many times a second), each scanning all of `/proc`; they now run at most every `poll_interval`, and
  while locked only the locker's own `/proc` entry is read.
- SQLite runs with `synchronous=NORMAL` (WAL), so focus changes no longer fsync.
- `install.sh` builds with cargo (or takes `SCREENTIME_BINARY=`) and removes the Python 0.1 code.

## [0.1.0] - 2026-11-04

- First release.
