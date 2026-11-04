"""Optional push: POST each day's summary to a URL (for a dashboard such as Marquee).

The queue is the database itself: a day stays "unsent" until a push succeeds after the day has
ended, so a machine that was asleep or away from home catches up on the next successful push.
"""
import datetime as dt
import json
import time
import urllib.error
import urllib.request

from . import __version__
from .stats import day_bounds, day_summary


def due_days(store, config, now):
    """Days to send, oldest first: unfinished pushes within `catch_up_days`, plus today."""
    today = dt.date.fromtimestamp(now)
    first = store.first_record()
    if first is None:
        return []
    oldest = max(dt.date.fromtimestamp(first), today - dt.timedelta(days=int(config['push']['catch_up_days'])))
    days = []
    day = oldest
    while day <= today:
        pushed = store.pushed(day.isoformat())
        if not (pushed and pushed[1]):
            days.append(day)
        day += dt.timedelta(days=1)
    return days


def send(url, token, payload, timeout=10):
    body = json.dumps(payload, separators=(',', ':')).encode()
    headers = {'Content-Type': 'application/json', 'User-Agent': f'screentime/{__version__}'}
    if token:
        headers['Authorization'] = f'Bearer {token}'
    request = urllib.request.Request(url, data=body, headers=headers, method='POST')
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.status


def push_due(store, config, now=None, log=print):
    """Send everything that is due. Stops at the first failure (offline): the rest waits."""
    now = time.time() if now is None else now
    url = config['push']['url']
    if not url:
        return 0
    sent = 0
    for day in due_days(store, config, now):
        summary = day_summary(store, config, day, now)
        if summary['total'] == 0 and summary['pickups'] == 0 and day != dt.date.fromtimestamp(now):
            store.mark_pushed(day.isoformat(), now, True)  # nothing happened that day
            continue
        try:
            send(url, config.push_token, {'device': config.device_name, 'date': day.isoformat(),
                                          'final': summary['complete'], 'summary': summary},
                 config['push']['timeout'])
        except urllib.error.HTTPError as error:
            log(f'push of {day} refused: HTTP {error.code}')
            return sent
        except (urllib.error.URLError, OSError, TimeoutError) as error:
            log(f'push of {day} failed ({getattr(error, "reason", error)}); will retry')
            return sent
        _, end = day_bounds(day)
        store.mark_pushed(day.isoformat(), now, now >= end)
        sent += 1
    return sent
