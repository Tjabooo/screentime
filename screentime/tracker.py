"""The state machine: which app has focus, and whether the person is actually there.

Time counts only while the screen is on, unlocked and not idle. Coming back from a locked,
blanked or suspended screen is a pickup (like waking an iPhone); coming back from a short idle
with the screen still on is not.
"""

AWAY_REASONS = ('suspended', 'locked', 'blanked')


class Tracker:
    def __init__(self, store, config, now, restarted=False):
        self.store, self.config = store, config
        self.app, self.title = None, None
        self.idle = self.locked = self.blanked = False
        # A fresh login counts as a pickup; a quick daemon restart doesn't.
        self.away = None if restarted else 'start'
        self.session = None  # (id, app, title, start)
        self.last_beat = now

    @property
    def active(self):
        return not (self.idle or self.locked or self.blanked)

    @property
    def state(self):
        if self.locked:
            return 'locked'
        if self.blanked:
            return 'screen-off'
        if self.idle:
            return 'idle'
        return 'active'

    def _label(self):
        app = self.app or 'desktop'
        if self.config.ignored(app):
            return None, None
        return app, (self.title if self.config['track_titles'] else None)

    def _close(self, end):
        if self.session:
            self.store.close_session(self.session[0], end, self.config['min_session'])
            self.session = None

    def update(self, now, end_at=None):
        if not self.active:
            self._close(end_at if end_at is not None else now)
        else:
            app, title = self._label()
            if self.session and (self.session[1], self.session[2]) != (app, title):
                self._close(now)
            if app is not None and not self.session:
                self.session = (self.store.open_session(app, title, now), app, title, now)
            if self.away:
                self.store.add_pickup(now, {'start': 'login', 'suspended': 'wake', 'locked': 'unlock',
                                            'blanked': 'screen-on'}.get(self.away, self.away))
            self.away = None
        self.store.set_meta(state=self.state, app=self.session[1] if self.session else None, last_beat=now)
        self.last_beat = now

    # ---- inputs ----
    def focus(self, app, title, now):
        self.app, self.title = app or None, title or None
        self.update(now)

    def set_idle(self, idle, now):
        if idle == self.idle:
            return
        self.idle = idle
        end_at = None
        if idle and self.config['trim_idle'] and self.session:
            # Nobody touched anything for idle_timeout seconds: that time wasn't use either.
            end_at = max(self.session[3], now - self.config['idle_timeout'])
        self.update(now, end_at)

    def set_locked(self, locked, now):
        if locked == self.locked:
            return
        self.locked = locked
        if locked:
            self.away = 'locked'
        self.update(now)

    def set_blanked(self, blanked, now):
        if blanked == self.blanked:
            return
        self.blanked = blanked
        if blanked and self.away is None:
            self.away = 'blanked'
        self.update(now)

    def suspended(self, since, now):
        """The machine slept from `since` until `now`: nothing in between counts."""
        self._close(since)
        self.away = 'suspended'
        self.update(now)

    def heartbeat(self, now):
        if self.session:
            self.store.extend_session(self.session[0], now)
        self.store.set_meta(state=self.state, app=self.session[1] if self.session else None, last_beat=now)
        self.last_beat = now

    def stop(self, now):
        self._close(now)
        self.store.set_meta(state='stopped', app=None, last_beat=now)
