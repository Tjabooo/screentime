"""Hyprland IPC: the focused window (socket2 events) and monitor power state (request socket)."""
import json
import os
import socket
from pathlib import Path


def runtime_dir():
    return Path(os.environ.get('XDG_RUNTIME_DIR') or f'/run/user/{os.getuid()}')


def instance_dir():
    """This session's Hyprland instance. The environment can be stale after Hyprland restarts
    (a user service keeps the old value), so fall back to the newest live instance."""
    base = runtime_dir() / 'hypr'
    signature = os.environ.get('HYPRLAND_INSTANCE_SIGNATURE')
    if signature and (base / signature / '.socket2.sock').exists():
        return base / signature
    candidates = [p for p in base.glob('*') if (p / '.socket2.sock').exists()] if base.is_dir() else []
    if not candidates:
        raise FileNotFoundError('Hyprland is not running (no socket in $XDG_RUNTIME_DIR/hypr).')
    return max(candidates, key=lambda p: (p / '.socket2.sock').stat().st_mtime)


def request(command, instance=None, timeout=3.0):
    """Send one hyprctl-style request ('j/activewindow') and return the decoded JSON."""
    path = (instance or instance_dir()) / '.socket.sock'
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(timeout)
        sock.connect(str(path))
        sock.sendall(command.encode())
        chunks = []
        while chunk := sock.recv(65536):
            chunks.append(chunk)
    return json.loads(b''.join(chunks) or b'null')


def active_window(instance=None):
    window = request('j/activewindow', instance) or {}
    return (window.get('class') or '', window.get('title') or '') if isinstance(window, dict) else ('', '')


def screens_off(instance=None):
    """True when every monitor is powered off (DPMS / wlr output power)."""
    monitors = request('j/monitors all', instance) or []
    states = [m.get('dpmsStatus', True) for m in monitors if not m.get('disabled')]
    return bool(states) and not any(states)


def parse_event(line):
    """'activewindow>>kitty,~/src' -> ('activewindow', 'kitty,~/src')."""
    name, _, data = line.partition('>>')
    return name, data


def split_window(data):
    """activewindow data is 'class,title'; the class never contains a comma, titles may."""
    cls, _, title = data.partition(',')
    return cls, title


class EventStream:
    """Line reader over socket2. `fileno()` makes it usable with selectors."""

    def __init__(self, instance=None):
        self.instance = instance or instance_dir()
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(self.instance / '.socket2.sock'))
        self.buffer = b''

    def fileno(self):
        return self.sock.fileno()

    def read_events(self):
        chunk = self.sock.recv(65536)
        if not chunk:
            raise ConnectionError('Hyprland closed the event socket.')
        self.buffer += chunk
        *lines, self.buffer = self.buffer.split(b'\n')
        return [parse_event(line.decode('utf-8', 'replace')) for line in lines if line]

    def close(self):
        self.sock.close()
