import datetime as dt
import http.server
import json
import os
import struct
import tempfile
import threading
import unittest
from pathlib import Path

from screentime import config as config_module, hypr, wayland
from screentime.cli import duration, main, render_day, render_week, Paint
from screentime.push import due_days, push_due
from screentime.stats import day_bounds, day_summary, week_summary
from screentime.store import Store
from screentime.tracker import Tracker

DAY = dt.date(2026, 11, 4)  # a Wednesday
T0 = dt.datetime.combine(DAY, dt.time(9)).timestamp()


def at(hour, minute=0, day=DAY):
    return dt.datetime.combine(day, dt.time(hour, minute)).timestamp()


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.config = config_module.load(Path(self.tmp.name) / 'missing.toml')
        self.config['database'] = str(Path(self.tmp.name) / 'st.db')
        self.config['device'] = 'testbox'
        self.store = Store(self.config.database_path)

    def tearDown(self):
        self.store.close()
        self.tmp.cleanup()

    def session(self, app, start, end):
        sid = self.store.open_session(app, None, start)
        self.store.close_session(sid, end)


class ConfigTests(Base):
    def test_categories_names_and_overrides(self):
        self.assertEqual(self.config.category('Alacritty'), 'Development')
        self.assertEqual(self.config.category('steam_app_730'), 'Games')
        self.assertEqual(self.config.category('something-new'), 'Other')
        self.assertEqual(self.config.name('org.mozilla.firefox'), 'Firefox')
        self.assertEqual(self.config.name('org.gnome.TextEditor'), 'TextEditor')
        self.assertEqual(self.config.name('steam_app_730'), 'Steam game 730')
        path = Path(self.tmp.name) / 'c.toml'
        path.write_text('idle_timeout = 60\n[categories]\n"alacritty" = "Productivity"\n[names]\n"alacritty" = "Terminal"\n[push]\nurl = "http://x"\n')
        config = config_module.load(path)
        self.assertEqual(config['idle_timeout'], 60.0)
        self.assertEqual(config.category('Alacritty'), 'Productivity')
        self.assertEqual(config.name('Alacritty'), 'Terminal')
        self.assertEqual(config['push']['interval'], 600)  # nested defaults survive a partial table

    def test_token_file_and_env(self):
        token = Path(self.tmp.name) / 'token'
        token.write_text('secret\n')
        self.config['push']['token_file'] = str(token)
        self.assertEqual(self.config.push_token, 'secret')
        os.environ['SCREENTIME_TOKEN'] = 'from-env'
        try:
            self.assertEqual(self.config.push_token, 'from-env')
        finally:
            del os.environ['SCREENTIME_TOKEN']


class TrackerTests(Base):
    def rows(self):
        return [(r['app'], r['start'], r['end']) for r in self.store.db.execute('SELECT * FROM sessions ORDER BY start')]

    def pickups(self):
        return [r for _, r in self.store.pickups(0, 1e12)]

    def test_focus_idle_lock_and_pickups(self):
        t = Tracker(self.store, self.config, T0)
        t.focus('kitty', 'vim', T0)
        t.focus('firefox', 'news', T0 + 100)
        t.set_idle(True, T0 + 400)  # idle_timeout 180: the last 180 s weren't use
        t.set_idle(False, T0 + 500)  # short idle with the screen on: not a pickup
        t.set_locked(True, T0 + 600)
        t.set_locked(False, T0 + 900)
        t.stop(T0 + 1000)
        self.assertEqual(self.rows(), [('kitty', T0, T0 + 100), ('firefox', T0 + 100, T0 + 220),
                                       ('firefox', T0 + 500, T0 + 600), ('firefox', T0 + 900, T0 + 1000)])
        self.assertEqual(self.pickups(), ['login', 'unlock'])
        self.assertEqual(self.store.meta('state'), 'stopped')

    def test_titles_only_when_enabled_and_flashes_dropped(self):
        t = Tracker(self.store, self.config, T0, restarted=True)
        t.focus('kitty', 'secret project', T0)
        t.focus('org.vinegarhq.Sober', 'Roblox', T0 + 10)  # 0.3 s focus flash
        t.focus('kitty', 'secret project', T0 + 10.3)
        t.stop(T0 + 20)
        titles = [r[0] for r in self.store.db.execute('SELECT title FROM sessions')]
        self.assertEqual(titles, [None, None])
        self.assertEqual(self.pickups(), [])  # a quick daemon restart is not a pickup

    def test_suspend_blank_and_ignore(self):
        self.config['ignore'] = ['screensaver']
        t = Tracker(self.store, self.config, T0)
        t.focus('kitty', '', T0)
        t.suspended(T0 + 50, T0 + 3600)  # slept, woke straight to an unlocked desktop
        t.set_blanked(True, T0 + 3700)
        t.set_blanked(False, T0 + 3800)
        t.focus('screensaver', '', T0 + 3810)
        t.stop(T0 + 3900)
        self.assertEqual(self.rows(), [('kitty', T0, T0 + 50), ('kitty', T0 + 3600, T0 + 3700), ('kitty', T0 + 3800, T0 + 3810)])
        self.assertEqual(self.pickups(), ['login', 'wake', 'screen-on'])

    def test_wake_into_lock_screen_is_one_pickup(self):
        t = Tracker(self.store, self.config, T0, restarted=True)
        t.focus('kitty', '', T0)
        t.set_locked(True, T0 + 10)
        t.suspended(T0 + 20, T0 + 100)
        t.set_blanked(True, T0 + 110)
        t.set_blanked(False, T0 + 120)
        t.set_locked(False, T0 + 130)
        self.assertEqual(self.pickups(), ['wake'])


class StatsTests(Base):
    def test_day_summary(self):
        self.session('kitty', at(8, 50), at(9, 20))  # crosses an hour
        self.session('firefox', at(9, 20, ), at(9, 30))  # same sitting
        self.session('discord', at(14), at(14, 5))
        self.session('kitty', at(23, 50), at(0, 10, DAY + dt.timedelta(days=1)))  # crosses midnight
        for ts in (at(8, 50), at(14), at(23, 50)):
            self.store.add_pickup(ts, 'unlock')
        s = day_summary(self.store, self.config, DAY, now=at(12, 0, DAY + dt.timedelta(days=1)))
        self.assertEqual(s['total'], 30 * 60 + 10 * 60 + 5 * 60 + 10 * 60)
        self.assertEqual(s['apps'][0], {'app': 'kitty', 'name': 'Kitty', 'category': 'Development', 'seconds': 2400})
        self.assertEqual(s['categories'][0], {'name': 'Development', 'seconds': 2400})
        self.assertEqual(s['hourly'][8]['seconds'], 600)
        self.assertEqual(s['hourly'][9], {'hour': 9, 'seconds': 1800, 'categories': {'Development': 1200, 'Web': 600}})
        self.assertEqual(s['hourly'][23]['seconds'], 600)
        self.assertEqual((s['pickups'], s['first_pickup']), (3, '08:50'))
        self.assertEqual(s['pickups_hourly'][14], 1)
        self.assertEqual(s['after_pickup'][0], {'app': 'kitty', 'name': 'Kitty', 'count': 2})
        self.assertEqual(s['longest_session'], {'seconds': 2400, 'start': '08:50', 'end': '09:30', 'app': 'kitty', 'name': 'Kitty'})
        self.assertEqual(s['sittings'], 3)
        self.assertTrue(s['complete'])
        self.assertIn('MOST USED', render_day(s, Paint(False), 'Today'))

    def test_open_session_counts_up_to_now(self):
        self.store.open_session('kitty', None, at(10))
        s = day_summary(self.store, self.config, DAY, now=at(10) + 30)
        self.assertEqual(s['total'], 30)
        self.assertFalse(s['complete'])
        stale = day_summary(self.store, self.config, DAY, now=at(11))  # daemon died: ends at last write
        self.assertEqual(stale['total'], 0)

    def test_week_average_and_change(self):
        last_monday = DAY - dt.timedelta(days=9)
        for i in range(7):  # last week: 1 h a day
            d = last_monday + dt.timedelta(days=i)
            self.session('kitty', at(10, 0, d), at(11, 0, d))
        for i in range(3):  # this week so far (Mon–Wed): 1.5 h a day
            d = DAY - dt.timedelta(days=2 - i)
            self.session('firefox', at(10, 0, d), at(11, 30, d))
        w = week_summary(self.store, self.config, DAY, now=at(23, 0))
        self.assertEqual((w['start'], w['end']), ('2026-11-02', '2026-11-08'))
        self.assertEqual(w['average'], 5400)
        self.assertEqual(w['last_week_average'], 3600)
        self.assertEqual(w['change_percent'], 50)
        self.assertEqual([d['total'] for d in w['days']], [5400, 5400, 5400, None, None, None, None])
        self.assertEqual(w['apps'][0]['app'], 'firefox')
        self.assertIn('from last week', render_week(w, Paint(False)))

    def test_week_counts_only_tracked_days(self):
        self.session('kitty', at(10), at(12))  # first day ever is today (Wednesday)
        w = week_summary(self.store, self.config, DAY, now=at(23))
        self.assertEqual(w['average'], 7200)  # not divided by 3
        self.assertIsNone(w['change_percent'])
        self.config['week_start'] = 'sunday'
        self.assertEqual(week_summary(self.store, self.config, DAY, now=at(23))['start'], '2026-11-01')


class PushTests(Base):
    def test_queue_catches_up_and_marks_final_days(self):
        received = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                received.append((self.headers['Authorization'], body))
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b'{}')

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        yesterday = DAY - dt.timedelta(days=1)
        self.session('kitty', at(10, 0, yesterday), at(11, 0, yesterday))
        self.session('kitty', at(10), at(10, 30))
        self.config['push'].update(url='http://127.0.0.1:1/unreachable', token='tok', timeout=2)
        self.assertEqual(push_due(self.store, self.config, now=at(12), log=lambda m: None), 0)  # offline
        self.assertEqual([d.isoformat() for d in due_days(self.store, self.config, at(12))], ['2026-11-03', '2026-11-04'])
        self.config['push']['url'] = f'http://127.0.0.1:{server.server_port}/api/screentime'
        self.assertEqual(push_due(self.store, self.config, now=at(12)), 2)
        self.assertEqual(received[0][0], 'Bearer tok')
        self.assertEqual([(b['date'], b['final'], b['summary']['total']) for _, b in received],
                         [('2026-11-03', True, 3600), ('2026-11-04', False, 1800)])
        self.assertEqual(received[0][1]['device'], 'testbox')
        # Yesterday is done; today keeps going out until a push after midnight.
        self.assertEqual([d.isoformat() for d in due_days(self.store, self.config, at(13))], ['2026-11-04'])


    def test_a_day_with_only_a_pickup_is_still_sent(self):
        self.store.add_pickup(at(9), 'login')  # logged in, then locked all day
        self.assertEqual([d.isoformat() for d in due_days(self.store, self.config, at(12))], ['2026-11-04'])


class ProtocolTests(unittest.TestCase):
    def test_wayland_wire_format(self):
        payload = struct.pack('=I', 7) + wayland._string('wl_seat') + struct.pack('=I', 9)
        data = wayland.message(2, 0, payload) + wayland.message(6, 1)
        messages, rest = wayland.parse_messages(data + b'\x01\x00')
        self.assertEqual(rest, b'\x01\x00')
        self.assertEqual([(m[0], m[1]) for m in messages], [(2, 0), (6, 1)])
        self.assertEqual(wayland.read_string(messages[0][2], 4), ('wl_seat', 16))
        self.assertEqual(len(wayland._string('abc')) % 4, 0)

    def test_hyprland_events(self):
        self.assertEqual(hypr.parse_event('activewindow>>kitty,vim: a, b'), ('activewindow', 'kitty,vim: a, b'))
        self.assertEqual(hypr.split_window('kitty,vim: a, b'), ('kitty', 'vim: a, b'))
        self.assertEqual(hypr.split_window(','), ('', ''))


class CliTests(Base):
    def test_duration(self):
        self.assertEqual([duration(s) for s in (0, 40, 600, 13320)], ['0m', '40s', '10m', '3h 42m'])

    def test_today_json(self):
        path = Path(self.tmp.name) / 'c.toml'
        path.write_text(f'database = "{self.config.database_path}"\ndevice = "testbox"\n')
        self.session('kitty', 1, 2)
        import contextlib
        import io
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['--config', str(path), 'today', '--json']), 0)
        data = json.loads(out.getvalue())
        self.assertEqual(data['device'], 'testbox')
        self.assertEqual(len(data['hourly']), 24)
        self.assertIn('running', data['daemon'])


if __name__ == '__main__':
    unittest.main()
