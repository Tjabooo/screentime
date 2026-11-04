"""screentime run | today | week | push | config"""
import argparse
import datetime as dt
import json
import os
import sys
import time

from . import __version__
from . import config as config_module
from .stats import day_summary, week_summary, daemon_state
from .store import Store


def duration(seconds):
    """3h 42m · 12m · 40s · 0m"""
    seconds = int(seconds or 0)
    if seconds < 60:
        return f'{seconds}s' if seconds else '0m'
    hours, minutes = divmod(seconds // 60, 60)
    return f'{hours}h {minutes:02d}m' if hours else f'{minutes}m'


class Paint:
    def __init__(self, enabled):
        self.enabled = enabled

    def __call__(self, text, code):
        return f'\033[{code}m{text}\033[0m' if self.enabled else text


CATEGORY_COLORS = {'Social': '38;5;211', 'Games': '38;5;209', 'Entertainment': '38;5;141', 'Web': '38;5;75',
                   'Development': '38;5;79', 'Productivity': '38;5;221', 'Creativity': '38;5;114',
                   'Education': '38;5;180', 'Utilities': '38;5;248', 'Other': '38;5;245'}
BLOCKS = ' ▁▂▃▄▅▆▇█'


def column(value, top):
    if not value:
        return ' '
    return BLOCKS[max(1, min(8, round(8 * value / top)))] if top else ' '


def bar(value, top, width=18):
    filled = round(width * value / top) if top else 0
    return '━' * max(1 if value else 0, filled) + ' ' * (width - max(1 if value else 0, filled))


def render_day(s, paint, label):
    out = [f"{paint('SCREEN TIME', '1')}  {paint(s['device'], '2')}  ·  {label}", '']
    out.append(f"  {paint(duration(s['total']), '1')}  {paint('today' if label.startswith('Today') else 'total', '2')}")
    out.append('')
    top = max((h['seconds'] for h in s['hourly']), default=0)
    lead = lambda h: max(h['categories'], key=h['categories'].get) if h['categories'] else 'Other'
    cols = ''.join(paint(column(h['seconds'], top) * 2, CATEGORY_COLORS.get(lead(h), '0')) for h in s['hourly'])
    out.append('  ' + cols)
    out.append('  ' + paint('00          06          12          18        ', '2'))
    cats = '   '.join(f"{paint('●', CATEGORY_COLORS.get(c['name'], '0'))} {c['name']} {duration(c['seconds'])}" for c in s['categories'][:4])
    if cats:
        out += ['', '  ' + cats]
    if s['apps']:
        out += ['', '  ' + paint('MOST USED', '1')]
        best = s['apps'][0]['seconds']
        for i, a in enumerate(s['apps'][:8], 1):
            out.append(f"  {i:>2}  {a['name'][:22]:<22} {duration(a['seconds']):>7}  {paint(bar(a['seconds'], best), CATEGORY_COLORS.get(a['category'], '0'))}")
    out += ['', f"  {paint('PICKUPS', '1')}  {s['pickups']}" + (f"   first at {s['first_pickup']}" if s['first_pickup'] else '')]
    if s['after_pickup']:
        out.append('  first used after pickup: ' + ', '.join(f"{a['name']} {a['count']}" for a in s['after_pickup'][:3]))
    ls = s['longest_session']
    if ls:
        when = f"{ls['start']}–{ls['end']}, mostly {ls['name']}"
        out.append(f"  {paint('LONGEST SESSION', '1')}  {duration(ls['seconds'])}  {paint(when, '2')}")
    return '\n'.join(out)


def render_week(w, paint):
    start, end = dt.date.fromisoformat(w['start']), dt.date.fromisoformat(w['end'])
    out = [f"{paint('SCREEN TIME', '1')}  {paint(w['device'], '2')}  ·  {start:%-d %b} – {end:%-d %b}", '']
    change = ''
    if w['change_percent'] is not None:
        arrow = '↑' if w['change_percent'] > 0 else '↓' if w['change_percent'] < 0 else '='
        change = paint(f"   {arrow} {abs(w['change_percent'])}% from last week", '2')
    out.append(f"  {paint(duration(w['average']), '1')}  {paint('daily average', '2')}{change}")
    out.append('')
    top = max((d['total'] or 0 for d in w['days']), default=0)
    for d in w['days']:
        if d['total'] is None:
            out.append(f"  {d['weekday']}  {paint('·', '2')}")
            continue
        lead = max(d['categories'], key=d['categories'].get) if d['categories'] else 'Other'
        out.append(f"  {d['weekday']}  {paint(bar(d['total'], top, 24), CATEGORY_COLORS.get(lead, '0'))} {duration(d['total']):>7}")
    out += ['', f"  Total {duration(w['total'])}"]
    if w['apps']:
        out += ['', '  ' + paint('MOST USED', '1')]
        best = w['apps'][0]['seconds']
        for i, a in enumerate(w['apps'][:8], 1):
            out.append(f"  {i:>2}  {a['name'][:22]:<22} {duration(a['seconds']):>7}  {paint(bar(a['seconds'], best), CATEGORY_COLORS.get(a['category'], '0'))}")
    out += ['', f"  {paint('PICKUPS', '1')}  {w['pickups']}   {w['pickups_average']} a day"]
    return '\n'.join(out)


def parse_day(text):
    if not text or text == 'today':
        return dt.date.today()
    if text == 'yesterday':
        return dt.date.today() - dt.timedelta(days=1)
    return dt.date.fromisoformat(text)


def main(argv=None):
    parser = argparse.ArgumentParser(prog='screentime', description='iPhone-style Screen Time for Hyprland.')
    parser.add_argument('--version', action='version', version=f'screentime {__version__}')
    parser.add_argument('--config', help='config file (default: ~/.config/screentime/config.toml)')
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('run', help='run the tracking daemon (normally the systemd user service)')
    for name, text in (('today', 'one day: total, apps, categories, hourly bars, pickups'),
                       ('week', 'the week: daily average vs last week, per-day bars, most used')):
        p = sub.add_parser(name, help=text)
        p.add_argument('--date', help='a day in ISO form, "yesterday" (default: today)')
        p.add_argument('--json', action='store_true', help='machine-readable output')
    sub.add_parser('push', help='send unsent days to the configured push URL now')
    sub.add_parser('config', help='print where the config and database are')
    args = parser.parse_args(argv)
    config = config_module.load(args.config)

    if args.command == 'run':
        from .daemon import Daemon
        return Daemon(config).run()
    if args.command == 'config':
        print(f'config    {config_module.config_path() if not args.config else args.config}')
        print(f'database  {config.database_path}')
        print(f'device    {config.device_name}')
        print(f"push      {config['push']['url'] or 'off'}")
        return 0
    if args.command == 'push':
        from .push import push_due
        if not config['push']['url']:
            print('Push is off: set [push] url in the config.', file=sys.stderr)
            return 1
        store = Store(config.database_path)
        print(f'pushed {push_due(store, config, log=lambda m: print(m, file=sys.stderr))} day(s)')
        store.close()
        return 0

    try:
        store = Store(config.database_path, readonly=True)
    except FileNotFoundError as error:
        print(error, file=sys.stderr)
        return 1
    now = time.time()
    day = parse_day(args.date)
    if args.command == 'today':
        data = day_summary(store, config, day, now)
    else:
        data = week_summary(store, config, day, now)
    data['daemon'] = daemon_state(store, now)
    store.close()
    if args.json:
        json.dump(data, sys.stdout, ensure_ascii=False)
        sys.stdout.write('\n')
        return 0
    paint = Paint(sys.stdout.isatty() and not os.environ.get('NO_COLOR'))
    if args.command == 'today':
        label = 'Today' if day == dt.date.today() else f'{day:%A %-d %B}'
        print(render_day(data, paint, f'{label}, {day:%a %-d %b}' if label == 'Today' else label))
    else:
        print(render_week(data, paint))
    if not data['daemon']['running']:
        print(paint('\n  The daemon isn’t running: systemctl --user start screentime', '2'))
    return 0
