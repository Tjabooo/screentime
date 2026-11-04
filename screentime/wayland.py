"""A tiny pure-Python Wayland client for one thing: ext-idle-notify-v1.

The compositor tells us when there has been no input for `timeout` seconds ("idled") and when
input comes back ("resumed"). It respects idle inhibitors from apps (a fullscreen video keeps
counting as screen time) but not systemd/logind inhibitors, so a "keep awake" toggle doesn't
make time away from the machine count.
"""
import os
import socket
import struct
from pathlib import Path

DISPLAY_ID, REGISTRY_ID, CALLBACK_ID, SEAT_ID, NOTIFIER_ID, NOTIFICATION_ID = 1, 2, 3, 4, 5, 6


def _string(text):
    data = text.encode() + b'\0'
    return struct.pack('=I', len(data)) + data + b'\0' * (-len(data) % 4)


def message(object_id, opcode, payload=b''):
    return struct.pack('=II', object_id, (8 + len(payload)) << 16 | opcode) + payload


def parse_messages(buffer):
    """Split a byte buffer into (object_id, opcode, payload) messages; returns them and the rest."""
    out = []
    while len(buffer) >= 8:
        object_id, word = struct.unpack_from('=II', buffer)
        size = word >> 16
        if size < 8 or len(buffer) < size:
            break
        out.append((object_id, word & 0xFFFF, buffer[8:size]))
        buffer = buffer[size:]
    return out, buffer


def read_string(payload, offset=0):
    (length,) = struct.unpack_from('=I', payload, offset)
    text = payload[offset + 4:offset + 4 + length - 1].decode('utf-8', 'replace')
    return text, offset + 4 + length + (-length % 4)


def socket_path():
    display = os.environ.get('WAYLAND_DISPLAY') or 'wayland-0'
    if display.startswith('/'):
        return Path(display)
    runtime = os.environ.get('XDG_RUNTIME_DIR') or f'/run/user/{os.getuid()}'
    return Path(runtime) / display


class Unsupported(RuntimeError):
    pass


class IdleWatcher:
    """Connects, binds wl_seat + ext_idle_notifier_v1 and asks for an idle notification.

    After `connect()`, select on `fileno()` and call `read()`: it returns a list of True (idled)
    / False (resumed) transitions."""

    def __init__(self, timeout_seconds, path=None):
        self.timeout_ms = max(1000, int(timeout_seconds * 1000))
        self.path = Path(path) if path else socket_path()
        self.sock = None
        self.buffer = b''

    def fileno(self):
        return self.sock.fileno()

    def _send(self, data):
        self.sock.sendall(data)

    def _recv(self):
        chunk = self.sock.recv(65536)
        if not chunk:
            raise ConnectionError('The Wayland compositor closed the connection.')
        messages, self.buffer = parse_messages(self.buffer + chunk)
        for object_id, opcode, payload in messages:
            if object_id == DISPLAY_ID and opcode == 0:  # wl_display.error(object, code, message)
                text, _ = read_string(payload, 8)
                raise RuntimeError(f'Wayland protocol error: {text}')
        return messages

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(self.path))
        self._send(message(DISPLAY_ID, 1, struct.pack('=I', REGISTRY_ID))  # get_registry
                   + message(DISPLAY_ID, 0, struct.pack('=I', CALLBACK_ID)))  # sync
        globals_, done = {}, False
        while not done:
            for object_id, opcode, payload in self._recv():
                if object_id == REGISTRY_ID and opcode == 0:  # global(name, interface, version)
                    (name,) = struct.unpack_from('=I', payload)
                    interface, offset = read_string(payload, 4)
                    (version,) = struct.unpack_from('=I', payload, offset)
                    globals_.setdefault(interface, (name, version))
                elif object_id == CALLBACK_ID and opcode == 0:
                    done = True
        if 'ext_idle_notifier_v1' not in globals_ or 'wl_seat' not in globals_:
            self.close()
            raise Unsupported('The compositor does not offer ext-idle-notify-v1.')

        def bind(interface, version, new_id):
            name, _ = globals_[interface]
            return message(REGISTRY_ID, 0, struct.pack('=I', name) + _string(interface) + struct.pack('=II', version, new_id))
        self._send(bind('wl_seat', 1, SEAT_ID) + bind('ext_idle_notifier_v1', 1, NOTIFIER_ID)
                   # get_idle_notification(id, timeout ms, seat)
                   + message(NOTIFIER_ID, 1, struct.pack('=III', NOTIFICATION_ID, self.timeout_ms, SEAT_ID)))
        return self

    def read(self):
        return [opcode == 0 for object_id, opcode, _ in self._recv()
                if object_id == NOTIFICATION_ID and opcode in (0, 1)]  # 0 idled, 1 resumed

    def close(self):
        if self.sock:
            self.sock.close()
            self.sock = None
