#!/bin/sh
# Install screentime for this user: the binary in ~/.local/share/screentime/bin (linked from
# ~/.local/bin), and a systemd user service that starts with the graphical session.
#   ./install.sh                                  build with cargo, install or update, (re)start the service
#   SCREENTIME_BINARY=/path/to/screentime ./install.sh   install a binary built elsewhere
#   ./install.sh --uninstall
set -eu
here=$(cd "$(dirname "$0")" && pwd)
share="$HOME/.local/share/screentime"
bin="$HOME/.local/bin/screentime"
unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/screentime.service"
conf="${XDG_CONFIG_HOME:-$HOME/.config}/screentime/config.toml"

if [ "${1:-}" = --uninstall ]; then
  systemctl --user disable --now screentime.service 2>/dev/null || true
  rm -rf "$share/bin" "$share/lib" "$bin" "$unit"
  systemctl --user daemon-reload
  echo "Removed screentime (your data in $share and config were kept)."
  exit 0
fi

if [ -n "${SCREENTIME_BINARY:-}" ]; then
  built=$SCREENTIME_BINARY
else
  command -v cargo >/dev/null || { echo "cargo not found: install Rust (Arch: pacman -S rust) or pass SCREENTIME_BINARY=" >&2; exit 1; }
  cargo build --release --locked --manifest-path "$here/Cargo.toml"
  built="$here/target/release/screentime"
fi
"$built" --version >/dev/null

mkdir -p "$share/bin" "$(dirname "$bin")" "$(dirname "$unit")" "$(dirname "$conf")"
rm -rf "$share/lib"                                   # the Python 0.1 code (the database next to it stays)
install -m 755 "$built" "$share/bin/.screentime.new"  # atomic: a running daemon keeps its old binary
mv -f "$share/bin/.screentime.new" "$share/bin/screentime"
rm -f "$bin"
ln -s "$share/bin/screentime" "$bin"
cp "$here/systemd/screentime.service" "$unit"
[ -e "$conf" ] || cp "$here/config.example.toml" "$conf"

systemctl --user daemon-reload
systemctl --user enable screentime.service >/dev/null 2>&1
systemctl --user restart screentime.service
echo "Installed: $bin -> $share/bin/screentime (service: systemctl --user status screentime). Try: screentime today"
