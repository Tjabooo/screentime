//! Hyprland IPC: the focused window (socket2 events) and monitor power state (request socket).

use std::env;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

pub fn runtime_dir() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        // SAFETY: getuid never fails.
        _ => PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })),
    }
}

/// This session's Hyprland instance. The environment can be stale after Hyprland restarts
/// (a user service keeps the old value), so fall back to the newest live instance.
pub fn instance_dir() -> io::Result<PathBuf> {
    let base = runtime_dir().join("hypr");
    if let Ok(sig) = env::var("HYPRLAND_INSTANCE_SIGNATURE")
        && !sig.is_empty()
        && base.join(&sig).join(".socket2.sock").exists()
    {
        return Ok(base.join(sig));
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let sock = entry.path().join(".socket2.sock");
            if let Ok(m) = sock.metadata().and_then(|m| m.modified())
                && best.as_ref().is_none_or(|(t, _)| m > *t)
            {
                best = Some((m, entry.path()));
            }
        }
    }
    best.map(|(_, p)| p)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Hyprland is not running (no socket in $XDG_RUNTIME_DIR/hypr)."))
}

/// Send one hyprctl-style request ('j/activewindow') and return the decoded JSON.
pub fn request(command: &str, instance: &Path) -> io::Result<Value> {
    let mut sock = UnixStream::connect(instance.join(".socket.sock"))?;
    sock.set_read_timeout(Some(Duration::from_secs(3)))?;
    sock.set_write_timeout(Some(Duration::from_secs(3)))?;
    sock.write_all(command.as_bytes())?;
    let mut out = Vec::new();
    sock.read_to_end(&mut out)?;
    if out.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&out).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn active_window(instance: &Path) -> io::Result<(String, String)> {
    let w = request("j/activewindow", instance)?;
    let s = |k: &str| w.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok((s("class"), s("title")))
}

/// True when every monitor is powered off (DPMS / wlr output power).
pub fn screens_off(instance: &Path) -> io::Result<bool> {
    let monitors = request("j/monitors all", instance)?;
    let states: Vec<bool> = monitors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| !m.get("disabled").and_then(Value::as_bool).unwrap_or(false))
        .map(|m| m.get("dpmsStatus").and_then(Value::as_bool).unwrap_or(true))
        .collect();
    Ok(!states.is_empty() && !states.iter().any(|&on| on))
}

/// 'activewindow>>kitty,~/src' -> ('activewindow', 'kitty,~/src').
pub fn parse_event(line: &str) -> (&str, &str) {
    line.split_once(">>").unwrap_or((line, ""))
}

/// activewindow data is 'class,title'; the class never contains a comma, titles may.
pub fn split_window(data: &str) -> (&str, &str) {
    data.split_once(',').unwrap_or((data, ""))
}

/// Line reader over socket2.
pub struct EventStream {
    sock: UnixStream,
    buffer: Vec<u8>,
}

impl EventStream {
    pub fn connect(instance: &Path) -> io::Result<Self> {
        Ok(EventStream { sock: UnixStream::connect(instance.join(".socket2.sock"))?, buffer: Vec::new() })
    }

    pub fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    pub fn read_events(&mut self) -> io::Result<Vec<(String, String)>> {
        let mut chunk = [0u8; 65536];
        let n = self.sock.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "Hyprland closed the event socket."));
        }
        self.buffer.extend_from_slice(&chunk[..n]);
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            if !line.is_empty() {
                let (name, data) = parse_event(&line);
                out.push((name.to_string(), data.to_string()));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprland_events() {
        assert_eq!(parse_event("activewindow>>kitty,vim: a, b"), ("activewindow", "kitty,vim: a, b"));
        assert_eq!(split_window("kitty,vim: a, b"), ("kitty", "vim: a, b"));
        assert_eq!(split_window(","), ("", ""));
    }
}
