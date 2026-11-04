"""Configuration: ~/.config/screentime/config.toml merged over built-in defaults."""
import copy
import fnmatch
import os
import socket
import tomllib
from pathlib import Path

# iOS-style categories for common Linux desktop apps. Keys are glob patterns matched against the
# lower-cased window class; the first match wins, and patterns from your config are tried first.
DEFAULT_CATEGORIES = {
    # Social
    'discord': 'Social', 'vesktop': 'Social', 'webcord': 'Social', 'signal': 'Social',
    'org.telegram.desktop': 'Social', 'telegram*': 'Social', 'element': 'Social', 'slack': 'Social',
    'whatsapp*': 'Social', 'thunderbird': 'Productivity', 'teams*': 'Social',
    # Games
    'steam_app_*': 'Games', 'cs2': 'Games', 'gamescope': 'Games', 'org.vinegarhq.sober': 'Games',
    'minecraft*': 'Games', 'net.lutris.lutris': 'Games', 'lutris': 'Games', 'gta*.exe': 'Games',
    'heroic': 'Games', 'retroarch': 'Games', 'osu!': 'Games', '*.exe': 'Games',
    'steam': 'Entertainment',
    # Entertainment
    'mpv': 'Entertainment', 'vlc': 'Entertainment', 'spotify': 'Entertainment', 'celluloid': 'Entertainment',
    'io.github.celluloid_player.celluloid': 'Entertainment', 'jellyfin*': 'Entertainment', 'stremio': 'Entertainment',
    # Web
    'firefox': 'Web', 'org.mozilla.firefox': 'Web', 'librewolf': 'Web', 'zen*': 'Web', 'chromium': 'Web',
    'google-chrome': 'Web', 'brave-browser': 'Web', 'vivaldi*': 'Web', 'qutebrowser': 'Web',
    # Development
    'code': 'Development', 'code-oss': 'Development', 'vscodium': 'Development', 'cursor': 'Development',
    'jetbrains-*': 'Development', 'zed': 'Development', 'dev.zed.zed': 'Development', 'neovide': 'Development',
    'alacritty': 'Development', 'kitty': 'Development', 'foot': 'Development', 'footclient': 'Development',
    'org.wezfurlong.wezterm': 'Development', 'com.mitchellh.ghostty': 'Development', 'wezterm': 'Development',
    # Productivity
    'obsidian': 'Productivity', 'libreoffice*': 'Productivity', 'org.pwmt.zathura': 'Productivity',
    'org.gnome.evince': 'Productivity', 'okular': 'Productivity', 'anki': 'Productivity',
    # Creativity
    'gimp*': 'Creativity', 'inkscape': 'Creativity', 'org.inkscape.inkscape': 'Creativity', 'krita': 'Creativity',
    'blender': 'Creativity', 'com.obsproject.studio': 'Creativity', 'obs': 'Creativity', 'kdenlive': 'Creativity',
    'org.kde.kdenlive': 'Creativity', 'darktable': 'Creativity', 'audacity': 'Creativity',
    # Utilities
    'thunar': 'Utilities', 'org.gnome.nautilus': 'Utilities', 'pcmanfm*': 'Utilities', 'pavucontrol': 'Utilities',
    'org.pulseaudio.pavucontrol': 'Utilities', 'nm-connection-editor': 'Utilities', 'blueman-manager': 'Utilities',
    'rofi': 'Utilities', 'wofi': 'Utilities', 'org.gnome.calculator': 'Utilities', 'qalculate*': 'Utilities',
    'desktop': 'Utilities',
}

# Friendlier names for classes that aren't readable as they are.
DEFAULT_NAMES = {
    'org.mozilla.firefox': 'Firefox', 'firefox': 'Firefox', 'code': 'VS Code', 'code-oss': 'VS Code',
    'cs2': 'Counter-Strike 2', 'org.vinegarhq.sober': 'Roblox', 'com.obsproject.studio': 'OBS Studio',
    'org.telegram.desktop': 'Telegram', 'org.wezfurlong.wezterm': 'WezTerm', 'com.mitchellh.ghostty': 'Ghostty',
    'net.lutris.lutris': 'Lutris', 'org.pwmt.zathura': 'Zathura', 'dev.zed.zed': 'Zed', 'desktop': 'Desktop',
    'google-chrome': 'Chrome', 'brave-browser': 'Brave', 'org.gnome.nautilus': 'Files',
    'org.pulseaudio.pavucontrol': 'Volume Control', 'pavucontrol': 'Volume Control',
}

DEFAULTS = {
    'device': '',                       # name sent with pushes; empty = this machine's hostname
    'database': '',                     # empty = $XDG_DATA_HOME/screentime/screentime.db
    'idle_timeout': 180,                # seconds without input before time stops counting
    'trim_idle': True,                  # also take those idle seconds back off the session
    'track_titles': False,              # store window titles too (off: only the app class)
    'poll_interval': 5,                 # seconds between lock / screen-off / suspend checks
    'min_session': 1.0,                 # drop focus flashes shorter than this many seconds
    'lockers': ['hyprlock', 'swaylock', 'gtklock', 'waylock', 'swaylock-effect'],
    'week_start': 'monday',             # or 'sunday'
    'categories': {},
    'names': {},
    'ignore': [],                       # classes (globs) never counted, e.g. a screensaver
    'push': {
        'url': '',                      # POST each day's summary here (empty = off)
        'token': '',                    # sent as "Authorization: Bearer <token>"
        'token_file': '',               # or read the token from this file
        'interval': 600,                # seconds between pushes of today's running summary
        'catch_up_days': 14,            # how far back unsent days are retried after being offline
        'timeout': 10,
    },
}

CATEGORIES = ['Social', 'Games', 'Entertainment', 'Web', 'Development', 'Productivity', 'Creativity',
              'Education', 'Utilities', 'Other']


def config_path():
    if os.environ.get('SCREENTIME_CONFIG'):
        return Path(os.environ['SCREENTIME_CONFIG']).expanduser()
    base = os.environ.get('XDG_CONFIG_HOME') or Path.home() / '.config'
    return Path(base) / 'screentime' / 'config.toml'


def data_dir():
    base = os.environ.get('XDG_DATA_HOME') or Path.home() / '.local' / 'share'
    return Path(base) / 'screentime'


def _merge(base, extra):
    for key, value in extra.items():
        if isinstance(value, dict) and isinstance(base.get(key), dict):
            _merge(base[key], value)
        else:
            base[key] = value
    return base


class Config(dict):
    __getattr__ = dict.__getitem__

    @property
    def device_name(self):
        return self['device'] or socket.gethostname()

    @property
    def database_path(self):
        return Path(self['database']).expanduser() if self['database'] else data_dir() / 'screentime.db'

    @property
    def push_token(self):
        push = self['push']
        if os.environ.get('SCREENTIME_TOKEN'):
            return os.environ['SCREENTIME_TOKEN']
        if push.get('token_file'):
            try:
                return Path(push['token_file']).expanduser().read_text().strip()
            except OSError:
                return ''
        return push.get('token') or ''

    def category(self, app):
        key = (app or '').lower()
        for table in (self['categories'], DEFAULT_CATEGORIES):
            for pattern, category in table.items():
                if fnmatch.fnmatchcase(key, pattern.lower()):
                    return category
        return 'Other'

    def name(self, app):
        key = (app or '').lower()
        for table in (self['names'], DEFAULT_NAMES):
            for pattern, name in table.items():
                if fnmatch.fnmatchcase(key, pattern.lower()):
                    return name
        return pretty_name(app)

    def ignored(self, app):
        key = (app or '').lower()
        return any(fnmatch.fnmatchcase(key, p.lower()) for p in self['ignore'])


def pretty_name(app):
    """'org.gnome.Nautilus' -> 'Nautilus', 'steam_app_730' -> 'Steam game 730', 'firefox' -> 'Firefox'."""
    if not app:
        return 'Desktop'
    if app.lower().startswith('steam_app_'):
        return 'Steam game ' + app[10:]
    name = app.rsplit('.', 1)[-1] if '.' in app and not app.lower().endswith('.exe') else app
    name = name.removesuffix('-bin').replace('_', ' ').replace('-', ' ').strip() or app
    return name[0].upper() + name[1:]


def load(path=None):
    config = copy.deepcopy(DEFAULTS)
    path = Path(path) if path else config_path()
    if path.is_file():
        with open(path, 'rb') as handle:
            _merge(config, tomllib.load(handle))
    for key in ('idle_timeout', 'poll_interval', 'min_session'):
        config[key] = float(config[key])
    return Config(config)
