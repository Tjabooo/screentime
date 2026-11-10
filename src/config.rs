//! Configuration: ~/.config/screentime/config.toml merged over built-in defaults.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// iOS-style categories for common Linux desktop apps. Patterns are globs matched against the
/// lower-cased window class; the first match wins, and patterns from your config are tried first.
pub const DEFAULT_CATEGORIES: &[(&str, &str)] = &[
    // Social
    ("discord", "Social"),
    ("vesktop", "Social"),
    ("webcord", "Social"),
    ("signal", "Social"),
    ("org.telegram.desktop", "Social"),
    ("telegram*", "Social"),
    ("element", "Social"),
    ("slack", "Social"),
    ("whatsapp*", "Social"),
    ("thunderbird", "Productivity"),
    ("teams*", "Social"),
    // Games
    ("steam_app_*", "Games"),
    ("cs2", "Games"),
    ("gamescope", "Games"),
    ("org.vinegarhq.sober", "Games"),
    ("minecraft*", "Games"),
    ("net.lutris.lutris", "Games"),
    ("lutris", "Games"),
    ("gta*.exe", "Games"),
    ("heroic", "Games"),
    ("retroarch", "Games"),
    ("osu!", "Games"),
    ("*.exe", "Games"),
    ("steam", "Entertainment"),
    // Entertainment
    ("mpv", "Entertainment"),
    ("vlc", "Entertainment"),
    ("spotify", "Entertainment"),
    ("celluloid", "Entertainment"),
    ("io.github.celluloid_player.celluloid", "Entertainment"),
    ("jellyfin*", "Entertainment"),
    ("stremio", "Entertainment"),
    // Web
    ("firefox", "Web"),
    ("org.mozilla.firefox", "Web"),
    ("librewolf", "Web"),
    ("zen*", "Web"),
    ("chromium", "Web"),
    ("google-chrome", "Web"),
    ("brave-browser", "Web"),
    ("vivaldi*", "Web"),
    ("qutebrowser", "Web"),
    // Development
    ("code", "Development"),
    ("code-oss", "Development"),
    ("vscodium", "Development"),
    ("cursor", "Development"),
    ("jetbrains-*", "Development"),
    ("zed", "Development"),
    ("dev.zed.zed", "Development"),
    ("neovide", "Development"),
    ("alacritty", "Development"),
    ("kitty", "Development"),
    ("foot", "Development"),
    ("footclient", "Development"),
    ("org.wezfurlong.wezterm", "Development"),
    ("com.mitchellh.ghostty", "Development"),
    ("wezterm", "Development"),
    // Productivity
    ("obsidian", "Productivity"),
    ("libreoffice*", "Productivity"),
    ("org.pwmt.zathura", "Productivity"),
    ("org.gnome.evince", "Productivity"),
    ("okular", "Productivity"),
    ("anki", "Productivity"),
    // Creativity
    ("gimp*", "Creativity"),
    ("inkscape", "Creativity"),
    ("org.inkscape.inkscape", "Creativity"),
    ("krita", "Creativity"),
    ("blender", "Creativity"),
    ("com.obsproject.studio", "Creativity"),
    ("obs", "Creativity"),
    ("kdenlive", "Creativity"),
    ("org.kde.kdenlive", "Creativity"),
    ("darktable", "Creativity"),
    ("audacity", "Creativity"),
    // Utilities
    ("thunar", "Utilities"),
    ("org.gnome.nautilus", "Utilities"),
    ("pcmanfm*", "Utilities"),
    ("pavucontrol", "Utilities"),
    ("org.pulseaudio.pavucontrol", "Utilities"),
    ("nm-connection-editor", "Utilities"),
    ("blueman-manager", "Utilities"),
    ("rofi", "Utilities"),
    ("wofi", "Utilities"),
    ("org.gnome.calculator", "Utilities"),
    ("qalculate*", "Utilities"),
    ("desktop", "Utilities"),
];

/// Friendlier names for classes that aren't readable as they are.
pub const DEFAULT_NAMES: &[(&str, &str)] = &[
    ("org.mozilla.firefox", "Firefox"),
    ("firefox", "Firefox"),
    ("code", "VS Code"),
    ("code-oss", "VS Code"),
    ("cs2", "Counter-Strike 2"),
    ("org.vinegarhq.sober", "Roblox"),
    ("com.obsproject.studio", "OBS Studio"),
    ("org.telegram.desktop", "Telegram"),
    ("org.wezfurlong.wezterm", "WezTerm"),
    ("com.mitchellh.ghostty", "Ghostty"),
    ("net.lutris.lutris", "Lutris"),
    ("org.pwmt.zathura", "Zathura"),
    ("dev.zed.zed", "Zed"),
    ("desktop", "Desktop"),
    ("google-chrome", "Chrome"),
    ("brave-browser", "Brave"),
    ("org.gnome.nautilus", "Files"),
    ("org.pulseaudio.pavucontrol", "Volume Control"),
    ("pavucontrol", "Volume Control"),
];

#[derive(Clone, Debug)]
pub struct Push {
    /// POST each day's summary here (empty = off).
    pub url: String,
    /// Sent as "Authorization: Bearer <token>".
    pub token: String,
    /// Or read the token from this file.
    pub token_file: String,
    /// Seconds between pushes of today's running summary.
    pub interval: f64,
    /// How far back unsent days are retried after being offline.
    pub catch_up_days: i64,
    pub timeout: f64,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub device: String,
    pub database: String,
    pub idle_timeout: f64,
    pub trim_idle: bool,
    pub track_titles: bool,
    pub poll_interval: f64,
    pub min_session: f64,
    pub lockers: Vec<String>,
    pub week_start: String,
    pub categories: Vec<(String, String)>,
    pub names: Vec<(String, String)>,
    pub ignore: Vec<String>,
    pub push: Push,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            device: String::new(),
            database: String::new(),
            idle_timeout: 180.0,
            trim_idle: true,
            track_titles: false,
            poll_interval: 5.0,
            min_session: 1.0,
            lockers: ["hyprlock", "swaylock", "gtklock", "waylock", "swaylock-effect"].map(String::from).to_vec(),
            week_start: "monday".into(),
            categories: Vec::new(),
            names: Vec::new(),
            ignore: Vec::new(),
            push: Push {
                url: String::new(),
                token: String::new(),
                token_file: String::new(),
                interval: 600.0,
                catch_up_days: 14,
                timeout: 10.0,
            },
        }
    }
}

pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &[&str]) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback.iter().fold(home(), |p, part| p.join(part)),
    }
}

pub fn config_path() -> PathBuf {
    match env::var_os("SCREENTIME_CONFIG") {
        Some(p) if !p.is_empty() => expand(&p.to_string_lossy()),
        _ => xdg("XDG_CONFIG_HOME", &[".config"]).join("screentime").join("config.toml"),
    }
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", &[".local", "share"]).join("screentime")
}

/// `~/x` -> `$HOME/x`.
pub fn expand(path: &str) -> PathBuf {
    if path == "~" {
        home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(path)
    }
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into buf.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    if !ok {
        return "localhost".into();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

impl Config {
    pub fn device_name(&self) -> String {
        if self.device.is_empty() { hostname() } else { self.device.clone() }
    }

    pub fn database_path(&self) -> PathBuf {
        if self.database.is_empty() { data_dir().join("screentime.db") } else { expand(&self.database) }
    }

    pub fn push_token(&self) -> String {
        if let Ok(t) = env::var("SCREENTIME_TOKEN")
            && !t.is_empty()
        {
            return t;
        }
        if !self.push.token_file.is_empty() {
            return fs::read_to_string(expand(&self.push.token_file)).map(|t| t.trim().to_string()).unwrap_or_default();
        }
        self.push.token.clone()
    }

    fn lookup<'a>(&'a self, user: &'a [(String, String)], builtin: &'a [(&'a str, &'a str)], app: &str) -> Option<&'a str> {
        let key = app.to_lowercase();
        user.iter()
            .map(|(p, v)| (p.as_str(), v.as_str()))
            .chain(builtin.iter().copied())
            .find(|(pattern, _)| fnmatch(&key, &pattern.to_lowercase()))
            .map(|(_, v)| v)
    }

    pub fn category(&self, app: &str) -> String {
        self.lookup(&self.categories, DEFAULT_CATEGORIES, app).unwrap_or("Other").to_string()
    }

    pub fn name(&self, app: &str) -> String {
        self.lookup(&self.names, DEFAULT_NAMES, app).map(String::from).unwrap_or_else(|| pretty_name(app))
    }

    pub fn ignored(&self, app: &str) -> bool {
        let key = app.to_lowercase();
        self.ignore.iter().any(|p| fnmatch(&key, &p.to_lowercase()))
    }
}

/// 'org.gnome.Nautilus' -> 'Nautilus', 'steam_app_730' -> 'Steam game 730', 'firefox' -> 'Firefox'.
pub fn pretty_name(app: &str) -> String {
    if app.is_empty() {
        return "Desktop".into();
    }
    if app.to_lowercase().starts_with("steam_app_") {
        return format!("Steam game {}", &app[10..]);
    }
    let base =
        if app.contains('.') && !app.to_lowercase().ends_with(".exe") { app.rsplit('.').next().unwrap_or(app) } else { app };
    let name = base.strip_suffix("-bin").unwrap_or(base).replace(['_', '-'], " ");
    let name = name.trim();
    let name = if name.is_empty() { app } else { name };
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Shell-style glob match (`*`, `?`, `[abc]`, `[!a-z]`), like Python's fnmatchcase.
pub fn fnmatch(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ti));
                    pi += 1;
                    continue;
                }
                '?' => {
                    ti += 1;
                    pi += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next)) = class(&p, pi, t[ti]) {
                        if matched {
                            ti += 1;
                            pi = next;
                            continue;
                        }
                    } else if t[ti] == '[' {
                        ti += 1;
                        pi += 1;
                        continue;
                    }
                }
                c if c == t[ti] => {
                    ti += 1;
                    pi += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, st)) => {
                pi = sp + 1;
                ti = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// A `[...]` class at p[start]: (does c match, index after the class), or None if unterminated.
fn class(p: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < p.len() && p[i] == '!';
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() && (first || p[i] != ']') {
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            matched |= p[i] <= c && c <= p[i + 2];
            i += 3;
        } else {
            matched |= p[i] == c;
            i += 1;
        }
    }
    if i >= p.len() {
        return None;
    }
    Some((matched != negate, i + 1))
}

fn num(value: &toml::Value) -> Option<f64> {
    match value {
        toml::Value::Integer(i) => Some(*i as f64),
        toml::Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn string(value: &toml::Value) -> Option<String> {
    value.as_str().map(String::from)
}

fn strings(value: &toml::Value) -> Option<Vec<String>> {
    value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
}

fn pairs(value: &toml::Value) -> Option<Vec<(String, String)>> {
    value.as_table().map(|t| t.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect())
}

/// Load the config file (missing = defaults). Unknown keys are ignored.
pub fn load(path: Option<&Path>) -> Result<Config, String> {
    let path = path.map(PathBuf::from).unwrap_or_else(config_path);
    let mut c = Config::default();
    if !path.is_file() {
        return Ok(c);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let table: toml::Table = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    let bad = |key: &str| format!("{}: {key} has the wrong type", path.display());
    macro_rules! set {
        ($field:expr, $key:literal, $conv:expr) => {
            if let Some(v) = table.get($key) {
                $field = $conv(v).ok_or_else(|| bad($key))?;
            }
        };
    }
    set!(c.device, "device", string);
    set!(c.database, "database", string);
    set!(c.idle_timeout, "idle_timeout", num);
    set!(c.trim_idle, "trim_idle", toml::Value::as_bool);
    set!(c.track_titles, "track_titles", toml::Value::as_bool);
    set!(c.poll_interval, "poll_interval", num);
    set!(c.min_session, "min_session", num);
    set!(c.lockers, "lockers", strings);
    set!(c.week_start, "week_start", string);
    set!(c.categories, "categories", pairs);
    set!(c.names, "names", pairs);
    set!(c.ignore, "ignore", strings);
    if let Some(push) = table.get("push") {
        let push = push.as_table().ok_or_else(|| bad("push"))?;
        let p = &mut c.push;
        let get = |k: &str| push.get(k);
        if let Some(v) = get("url") {
            p.url = string(v).ok_or_else(|| bad("push.url"))?;
        }
        if let Some(v) = get("token") {
            p.token = string(v).ok_or_else(|| bad("push.token"))?;
        }
        if let Some(v) = get("token_file") {
            p.token_file = string(v).ok_or_else(|| bad("push.token_file"))?;
        }
        if let Some(v) = get("interval") {
            p.interval = num(v).ok_or_else(|| bad("push.interval"))?;
        }
        if let Some(v) = get("catch_up_days") {
            p.catch_up_days = num(v).ok_or_else(|| bad("push.catch_up_days"))? as i64;
        }
        if let Some(v) = get("timeout") {
            p.timeout = num(v).ok_or_else(|| bad("push.timeout"))?;
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_names_and_globs() {
        let c = Config::default();
        assert_eq!(c.category("Alacritty"), "Development");
        assert_eq!(c.category("steam_app_730"), "Games");
        assert_eq!(c.category("GTA5.exe"), "Games");
        assert_eq!(c.category("something-new"), "Other");
        assert_eq!(c.name("org.mozilla.firefox"), "Firefox");
        assert_eq!(c.name("org.gnome.TextEditor"), "TextEditor");
        assert_eq!(c.name("steam_app_730"), "Steam game 730");
        assert_eq!(c.name("signal-desktop-bin"), "Signal desktop");
        assert_eq!(c.name(""), "Desktop");
        assert!(fnmatch("abc", "a[a-c]c") && !fnmatch("abc", "a[!b]c") && fnmatch("a[", "a["));
        assert!(fnmatch("", "*") && !fnmatch("x", "") && fnmatch("xyz", "*z") && !fnmatch("xyz", "*y"));
    }

    #[test]
    fn file_overrides_and_nested_defaults() {
        let dir = std::env::temp_dir().join(format!("st-conf-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.toml");
        fs::write(&path, "idle_timeout = 60\n[categories]\nalacritty = \"Productivity\"\n[names]\nalacritty = \"Terminal\"\n[push]\nurl = \"http://x\"\ntoken_file = \"").unwrap();
        assert!(load(Some(&path)).is_err());
        let token = dir.join("token");
        fs::write(&token, "secret\n").unwrap();
        fs::write(&path, format!("idle_timeout = 60\n[categories]\nalacritty = \"Productivity\"\n[names]\nalacritty = \"Terminal\"\n[push]\nurl = \"http://x\"\ntoken_file = \"{}\"\n", token.display())).unwrap();
        let c = load(Some(&path)).unwrap();
        assert_eq!(c.idle_timeout, 60.0);
        assert_eq!(c.category("Alacritty"), "Productivity");
        assert_eq!(c.name("Alacritty"), "Terminal");
        assert_eq!(c.push.interval, 600.0); // nested defaults survive a partial table
        assert_eq!(c.push_token(), "secret");
        fs::remove_dir_all(&dir).ok();
    }
}
