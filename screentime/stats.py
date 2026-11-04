"""iPhone-style Screen Time numbers from the stored sessions."""
import datetime as dt
import time
from collections import Counter, defaultdict

MERGE_GAP = 5.0  # app sessions this close together are one sitting ("longest session")


def day_start(day):
    return dt.datetime.combine(day, dt.time()).timestamp()


def day_bounds(day):
    return day_start(day), day_start(day + dt.timedelta(days=1))


def _clip(sessions, start, end):
    for app, title, a, b in sessions:
        a, b = max(a, start), min(b, end)
        if b > a:
            yield app, title, a, b


def _hhmm(ts):
    return dt.datetime.fromtimestamp(ts).strftime('%H:%M') if ts is not None else None


def _offset_minutes(ts):
    return round(dt.datetime.fromtimestamp(ts).astimezone().utcoffset().total_seconds() / 60)


def daemon_state(store, now):
    beat = float(store.meta('last_beat', 0) or 0)
    return {'running': 0 <= now - beat < 60, 'state': store.meta('state'), 'app': store.meta('app'), 'last_beat': beat or None}


def day_summary(store, config, day, now=None):
    """Everything the Today screen shows, for one calendar day."""
    now = time.time() if now is None else now
    start, end = day_bounds(day)
    sessions = list(_clip(store.sessions(start, end, now), start, min(end, now)))
    pickups = store.pickups(start, end)
    hours = [dt.datetime.combine(day, dt.time(h)).timestamp() for h in range(24)] + [end]

    apps, hourly, hourly_cat = Counter(), [0.0] * 24, [defaultdict(float) for _ in range(24)]
    for app, _, a, b in sessions:
        apps[app] += b - a
        category = config.category(app)
        for h in range(24):
            overlap = min(b, hours[h + 1]) - max(a, hours[h])
            if overlap > 0:
                hourly[h] += overlap
                hourly_cat[h][category] += overlap
    categories = Counter()
    for app, seconds in apps.items():
        categories[config.category(app)] += seconds

    # A sitting: app sessions back to back (focus changes don't end it, being away does).
    sittings = []
    for app, _, a, b in sessions:
        if sittings and a - sittings[-1]['end'] <= MERGE_GAP:
            sittings[-1]['end'] = max(sittings[-1]['end'], b)
            sittings[-1]['apps'][app] += b - a
        else:
            sittings.append({'start': a, 'end': b, 'apps': Counter({app: b - a})})
    longest = max(sittings, key=lambda s: s['end'] - s['start'], default=None)

    pickups_hourly = [0] * 24
    for ts, _ in pickups:
        pickups_hourly[dt.datetime.fromtimestamp(ts).hour] += 1
    after = Counter()
    for ts, _ in pickups:
        first = next((app for app, _, a, _ in sessions if a >= ts - 1), None)
        if first is not None:
            after[first] += 1

    def app_entry(app, seconds):
        return {'app': app, 'name': config.name(app), 'category': config.category(app), 'seconds': round(seconds)}

    total = sum(apps.values())
    return {
        'version': 1,
        'device': config.device_name,
        'date': day.isoformat(),
        'generated': round(now),
        'utc_offset': _offset_minutes(min(now, end - 1)),
        'complete': now >= end,
        'total': round(total),
        'apps': [app_entry(app, s) for app, s in apps.most_common()],
        'categories': [{'name': c, 'seconds': round(s)} for c, s in categories.most_common()],
        'hourly': [{'hour': h, 'seconds': round(hourly[h]), 'categories': {c: round(s) for c, s in sorted(hourly_cat[h].items(), key=lambda i: -i[1])}}
                   for h in range(24)],
        'pickups': len(pickups),
        'pickups_hourly': pickups_hourly,
        'first_pickup': _hhmm(pickups[0][0]) if pickups else None,
        'first_pickup_ts': round(pickups[0][0]) if pickups else None,
        'after_pickup': [{'app': app, 'name': config.name(app), 'count': n} for app, n in after.most_common(10)],
        'longest_session': None if not longest else {
            'seconds': round(longest['end'] - longest['start']),
            'start': _hhmm(longest['start']), 'end': _hhmm(longest['end']),
            'app': longest['apps'].most_common(1)[0][0], 'name': config.name(longest['apps'].most_common(1)[0][0]),
        },
        'sittings': len(sittings),
        'titles': bool(config['track_titles']),
    }


def week_start(day, config):
    offset = (day.weekday() + 1) % 7 if config['week_start'] == 'sunday' else day.weekday()
    return day - dt.timedelta(days=offset)


def _tracked_days(days, today, first_day):
    """Days that count towards an average: already started, and not before tracking began."""
    return [d for d in days if d <= today and (first_day is None or d >= first_day)]


def week_summary(store, config, day, now=None):
    """The week containing `day`: daily average vs last week, per-day bars and the most used apps."""
    now = time.time() if now is None else now
    today = dt.date.fromtimestamp(now)
    first = store.first_record()
    first_day = dt.date.fromtimestamp(first) if first else None
    start = week_start(day, config)
    days = [start + dt.timedelta(days=i) for i in range(7)]
    summaries = {d: day_summary(store, config, d, now) for d in days if d <= today}

    apps, categories, pickups = Counter(), Counter(), 0
    for s in summaries.values():
        for a in s['apps']:
            apps[a['app']] += a['seconds']
        for c in s['categories']:
            categories[c['name']] += c['seconds']
        pickups += s['pickups']
    counted = _tracked_days(days, today, first_day)
    total = sum(s['total'] for s in summaries.values())
    average = total / len(counted) if counted else 0

    last_days = [d - dt.timedelta(days=7) for d in days]
    last_counted = _tracked_days(last_days, today, first_day)
    last_total = 0
    for d in last_counted:
        a, b = day_bounds(d)
        last_total += sum(y - x for _, _, x, y in _clip(store.sessions(a, b, now), a, min(b, now)))
    last_average = last_total / len(last_counted) if last_counted else None
    change = round(100 * (average - last_average) / last_average) if last_average else None

    longest = max((dict(s['longest_session'], date=d.isoformat()) for d, s in summaries.items() if s['longest_session']),
                  key=lambda s: s['seconds'], default=None)
    return {
        'version': 1,
        'device': config.device_name,
        'start': start.isoformat(),
        'end': days[-1].isoformat(),
        'generated': round(now),
        'total': round(total),
        'average': round(average),
        'last_week_average': None if last_average is None else round(last_average),
        'change_percent': change,
        'days': [{'date': d.isoformat(), 'weekday': d.strftime('%a'),
                  'total': summaries[d]['total'] if d in summaries else None,
                  'pickups': summaries[d]['pickups'] if d in summaries else None,
                  'categories': {c['name']: c['seconds'] for c in summaries[d]['categories']} if d in summaries else {}}
                 for d in days],
        'apps': [{'app': a, 'name': config.name(a), 'category': config.category(a), 'seconds': s} for a, s in apps.most_common()],
        'categories': [{'name': c, 'seconds': s} for c, s in categories.most_common()],
        'pickups': pickups,
        'pickups_average': round(pickups / len(counted)) if counted else 0,
        'longest_session': longest,
    }
