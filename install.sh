#!/bin/sh
# Install screentime for this user: code in ~/.local/share/screentime, the `screentime` command in
# ~/.local/bin, and a systemd user service that starts with the graphical session.
#   ./install.sh            install or update, then (re)start the service
#   ./install.sh --uninstall
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lib="$HOME/.local/share/screentime/lib"
bin="$HOME/.local/bin/screentime"
unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/screentime.service"
conf="${XDG_CONFIG_HOME:-$HOME/.config}/screentime/config.toml"

if [ "${1:-}" = --uninstall ]; then
  systemctl --user disable --now screentime.service 2>/dev/null || true
  rm -rf "$lib" "$bin" "$unit"
  systemctl --user daemon-reload
  echo "Removed screentime (your data in ~/.local/share/screentime and config were kept)."
  exit 0
fi

python=${PYTHON:-}
[ -n "$python" ] || { [ -x /usr/bin/python3 ] && python=/usr/bin/python3; } || python=$(command -v python3)
"$python" -c 'import sys; sys.exit(sys.version_info < (3, 11))' || { echo "screentime needs Python 3.11 or newer." >&2; exit 1; }

mkdir -p "$lib" "$(dirname "$bin")" "$(dirname "$unit")" "$(dirname "$conf")"
rm -rf "$lib/screentime"
cp -r "$here/screentime" "$lib/"
find "$lib" -name __pycache__ -prune -exec rm -rf {} +
cat > "$bin" <<SH
#!/bin/sh
PYTHONPATH="$lib\${PYTHONPATH:+:\$PYTHONPATH}" exec "$python" -m screentime "\$@"
SH
chmod +x "$bin"
cp "$here/systemd/screentime.service" "$unit"
[ -e "$conf" ] || cp "$here/config.example.toml" "$conf"

systemctl --user daemon-reload
systemctl --user enable screentime.service >/dev/null 2>&1
systemctl --user restart screentime.service
echo "Installed: $bin (service: systemctl --user status screentime). Try: screentime today"
