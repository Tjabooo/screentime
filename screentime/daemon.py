"""The tracking daemon: Hyprland focus events + Wayland idle + lock/screen-off/suspend checks."""
import os
import selectors
import signal
import sys
import threading
import time
from pathlib import Path

from . import hypr, wayland
from .push import push_due
from .store import Store
from .tracker import Tracker

HEARTBEAT = 15  # seconds between writes of the running session's end
RECONNECT = 5


def log(*parts):
    print('screentime:', *parts, file=sys.stderr, flush=True)


def locker_running(names):
    """Is a screen locker (hyprlock, swaylock…) running as this user?"""
    uid = os.getuid()
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if entry.stat().st_uid != uid:
                continue
            if (entry / 'comm').read_text().strip() in names:
                return True
        except OSError:
            continue
    return False


def suspend_offset():
    """Grows by the time spent suspended: BOOTTIME counts sleep, MONOTONIC doesn't."""
    return time.clock_gettime(time.CLOCK_BOOTTIME) - time.clock_gettime(time.CLOCK_MONOTONIC)


class Pusher:
    """Runs pushes on a thread so a slow network never holds up tracking."""

    def __init__(self, config):
        self.config = config
        self.interval = float(config['push']['interval'])
        self.next = time.monotonic() + 20
        self.thread = None

    def soon(self, delay=15):
        self.next = min(self.next, time.monotonic() + delay)

    def tick(self):
        if not self.config['push']['url'] or time.monotonic() < self.next or (self.thread and self.thread.is_alive()):
            return
        self.next = time.monotonic() + self.interval
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        store = Store(self.config.database_path)
        try:
            sent = push_due(store, self.config, log=log)
            if sent:
                log(f'pushed {sent} day(s)')
        except Exception as error:  # never let a push take the daemon down
            log('push error:', error)
        finally:
            store.close()


class Daemon:
    def __init__(self, config):
        self.config = config
        self.store = Store(config.database_path)
        self.store.close_stale()
        now = time.time()
        last = float(self.store.meta('last_beat', 0) or 0)
        self.tracker = Tracker(self.store, config, now, restarted=0 <= now - last < 120)
        self.pusher = Pusher(config)
        self.running = True
        self.events = self.idle = None

    def stop(self, *_):
        self.running = False

    def connect(self):
        instance = hypr.instance_dir()
        self.events = hypr.EventStream(instance)
        self.instance = instance
        try:
            self.idle = wayland.IdleWatcher(self.config['idle_timeout']).connect()
        except (wayland.Unsupported, OSError, RuntimeError) as error:
            self.idle = None
            log(f'no idle detection ({error}); only lock, screen-off and suspend stop the clock')
        app, title = hypr.active_window(instance)
        self.tracker.focus(app, title, time.time())

    def disconnect(self):
        for conn in (self.events, self.idle):
            if conn:
                try:
                    conn.close()
                except OSError:
                    pass
        self.events = self.idle = None

    def poll_state(self, now):
        try:
            self.tracker.set_blanked(hypr.screens_off(self.instance), now)
        except (OSError, ValueError):
            pass
        self.tracker.set_locked(locker_running(set(self.config['lockers'])), now)

    def run(self):
        signal.signal(signal.SIGTERM, self.stop)
        signal.signal(signal.SIGINT, self.stop)
        log(f'tracking into {self.config.database_path} (idle after {self.config["idle_timeout"]:.0f} s'
            f'{", titles on" if self.config["track_titles"] else ""}{", pushing" if self.config["push"]["url"] else ""})')
        offset = suspend_offset()
        last_wall = time.time()
        last_beat = 0.0
        while self.running:
            if not self.events:
                try:
                    self.connect()
                except (OSError, ValueError) as error:
                    log(f'waiting for Hyprland: {error}')
                    self.tracker.set_blanked(True, time.time())  # no compositor, nothing on screen
                    time.sleep(RECONNECT)
                    continue
                self.tracker.set_blanked(False, time.time())
            selector = selectors.DefaultSelector()
            selector.register(self.events, selectors.EVENT_READ, 'hypr')
            if self.idle:
                selector.register(self.idle, selectors.EVENT_READ, 'idle')
            try:
                while self.running:
                    ready = selector.select(self.config['poll_interval'])
                    now = time.time()
                    slept = suspend_offset() - offset
                    if slept > 3:
                        offset += slept
                        log(f'resumed after {slept:.0f} s asleep')
                        self.tracker.suspended(last_wall, now)
                        self.pusher.soon()
                    last_wall = now
                    for key, _ in ready:
                        if key.data == 'hypr':
                            for name, data in self.events.read_events():
                                if name == 'activewindow':
                                    self.tracker.focus(*hypr.split_window(data), now)
                                elif name in ('workspace', 'focusedmon', 'closewindow'):
                                    self.tracker.focus(*hypr.active_window(self.instance), now)
                        else:
                            for idled in self.idle.read():
                                self.tracker.set_idle(idled, now)
                    self.poll_state(now)
                    if now - last_beat >= HEARTBEAT:
                        self.tracker.heartbeat(now)
                        last_beat = now
                    self.pusher.tick()
            except (OSError, ConnectionError, RuntimeError, ValueError) as error:
                log(f'lost connection ({error}); reconnecting')
                self.tracker.set_idle(False, time.time())
                self.disconnect()
                time.sleep(1)
            finally:
                selector.close()
        self.tracker.stop(time.time())
        self.disconnect()
        log('stopped')
        return 0
