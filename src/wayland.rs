//! A tiny Wayland client for one thing: ext-idle-notify-v1.
//!
//! The compositor tells us when there has been no input for `timeout` seconds ("idled") and when
//! input comes back ("resumed"). It respects idle inhibitors from apps (a fullscreen video keeps
//! counting as screen time) but not systemd/logind inhibitors, so a "keep awake" toggle doesn't
//! make time away from the machine count.

use std::env;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const DISPLAY_ID: u32 = 1;
const REGISTRY_ID: u32 = 2;
const CALLBACK_ID: u32 = 3;
const SEAT_ID: u32 = 4;
const NOTIFIER_ID: u32 = 5;
const NOTIFICATION_ID: u32 = 6;

#[derive(Debug)]
pub enum Error {
    Unsupported(String),
    Io(io::Error),
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Unsupported(s) | Error::Protocol(s) => f.write_str(s),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub fn string(text: &str) -> Vec<u8> {
    let mut data = text.as_bytes().to_vec();
    data.push(0);
    let mut out = (data.len() as u32).to_ne_bytes().to_vec();
    let pad = (4 - data.len() % 4) % 4;
    out.extend(data);
    out.extend(std::iter::repeat_n(0, pad));
    out
}

pub fn message(object_id: u32, opcode: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = object_id.to_ne_bytes().to_vec();
    out.extend((((8 + payload.len() as u32) << 16) | u32::from(opcode)).to_ne_bytes());
    out.extend(payload);
    out
}

fn u32_at(buf: &[u8], offset: usize) -> Option<u32> {
    buf.get(offset..offset + 4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
}

pub type Message = (u32, u16, Vec<u8>);

/// Split a byte buffer into (object_id, opcode, payload) messages; returns them and the rest.
pub fn parse_messages(mut buffer: &[u8]) -> (Vec<Message>, Vec<u8>) {
    let mut out = Vec::new();
    while buffer.len() >= 8 {
        let object_id = u32_at(buffer, 0).unwrap_or(0);
        let word = u32_at(buffer, 4).unwrap_or(0);
        let size = (word >> 16) as usize;
        if size < 8 || buffer.len() < size {
            break;
        }
        out.push((object_id, (word & 0xFFFF) as u16, buffer[8..size].to_vec()));
        buffer = &buffer[size..];
    }
    (out, buffer.to_vec())
}

pub fn read_string(payload: &[u8], offset: usize) -> (String, usize) {
    let len = u32_at(payload, offset).unwrap_or(0) as usize;
    let start = offset + 4;
    let end = (start + len.saturating_sub(1)).min(payload.len());
    let text = String::from_utf8_lossy(payload.get(start..end).unwrap_or_default()).into_owned();
    (text, start + len + (4 - len % 4) % 4)
}

fn socket_path() -> PathBuf {
    let display = env::var("WAYLAND_DISPLAY").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "wayland-0".into());
    if display.starts_with('/') {
        return PathBuf::from(display);
    }
    crate::hypr::runtime_dir().join(display)
}

/// Binds wl_seat + ext_idle_notifier_v1 and asks for an idle notification. Poll `fd()` and call
/// `read()`: it returns a list of true (idled) / false (resumed) transitions.
pub struct IdleWatcher {
    sock: UnixStream,
    buffer: Vec<u8>,
}

impl IdleWatcher {
    pub fn connect(timeout_seconds: f64) -> Result<Self, Error> {
        let timeout_ms = ((timeout_seconds * 1000.0) as u32).max(1000);
        let mut w = IdleWatcher { sock: UnixStream::connect(socket_path())?, buffer: Vec::new() };
        let mut hello = message(DISPLAY_ID, 1, &REGISTRY_ID.to_ne_bytes()); // get_registry
        hello.extend(message(DISPLAY_ID, 0, &CALLBACK_ID.to_ne_bytes())); // sync
        w.sock.write_all(&hello)?;
        let mut globals: Vec<(String, u32)> = Vec::new();
        let mut done = false;
        while !done {
            for (object_id, opcode, payload) in w.recv()? {
                if object_id == REGISTRY_ID && opcode == 0 {
                    // global(name, interface, version)
                    let name = u32_at(&payload, 0).unwrap_or(0);
                    let (interface, _) = read_string(&payload, 4);
                    if !globals.iter().any(|(i, _)| *i == interface) {
                        globals.push((interface, name));
                    }
                } else if object_id == CALLBACK_ID && opcode == 0 {
                    done = true;
                }
            }
        }
        let find = |iface: &str| globals.iter().find(|(i, _)| i == iface).map(|(_, n)| *n);
        let (Some(seat), Some(notifier)) = (find("wl_seat"), find("ext_idle_notifier_v1")) else {
            return Err(Error::Unsupported("The compositor does not offer ext-idle-notify-v1.".into()));
        };
        let bind = |iface: &str, global: u32, new_id: u32| {
            let mut p = global.to_ne_bytes().to_vec();
            p.extend(string(iface));
            p.extend(1u32.to_ne_bytes()); // version
            p.extend(new_id.to_ne_bytes());
            message(REGISTRY_ID, 0, &p)
        };
        let mut req = bind("wl_seat", seat, SEAT_ID);
        req.extend(bind("ext_idle_notifier_v1", notifier, NOTIFIER_ID));
        // get_idle_notification(id, timeout ms, seat)
        let mut p = NOTIFICATION_ID.to_ne_bytes().to_vec();
        p.extend(timeout_ms.to_ne_bytes());
        p.extend(SEAT_ID.to_ne_bytes());
        req.extend(message(NOTIFIER_ID, 1, &p));
        w.sock.write_all(&req)?;
        Ok(w)
    }

    pub fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    fn recv(&mut self) -> Result<Vec<Message>, Error> {
        let mut chunk = [0u8; 65536];
        let n = self.sock.read(&mut chunk)?;
        if n == 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "The Wayland compositor closed the connection.",
            )));
        }
        self.buffer.extend_from_slice(&chunk[..n]);
        let (messages, rest) = parse_messages(&self.buffer);
        self.buffer = rest;
        for (object_id, opcode, payload) in &messages {
            if *object_id == DISPLAY_ID && *opcode == 0 {
                // wl_display.error(object, code, message)
                let (text, _) = read_string(payload, 8);
                return Err(Error::Protocol(format!("Wayland protocol error: {text}")));
            }
        }
        Ok(messages)
    }

    pub fn read(&mut self) -> Result<Vec<bool>, Error> {
        Ok(self
            .recv()?
            .into_iter()
            .filter(|(id, op, _)| *id == NOTIFICATION_ID && *op <= 1)
            .map(|(_, op, _)| op == 0) // 0 idled, 1 resumed
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        let mut payload = 7u32.to_ne_bytes().to_vec();
        payload.extend(string("wl_seat"));
        payload.extend(9u32.to_ne_bytes());
        let mut buf = message(2, 0, &payload);
        buf.extend(message(6, 1, &[]));
        buf.extend([1, 0]);
        let (messages, rest) = parse_messages(&buf);
        assert_eq!(rest, [1, 0]);
        assert_eq!(messages.iter().map(|m| (m.0, m.1)).collect::<Vec<_>>(), [(2, 0), (6, 1)]);
        assert_eq!(read_string(&messages[0].2, 4), ("wl_seat".to_string(), 16));
        assert_eq!(string("abc").len() % 4, 0);
    }
}
