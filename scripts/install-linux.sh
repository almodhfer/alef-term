#!/bin/sh
# Install Alef for the current user on Linux: the executable in
# ~/.local/bin, and a desktop entry and icon so it appears in the launcher.
# Run it again to update. PREFIX and XDG_DATA_HOME choose other locations.
#
# From a source checkout this builds first. In a release download, where the
# executable and icons sit beside this script, it installs those.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -x "$HERE/alef-term" ]; then
  EXE="$HERE/alef-term"
  icon() { cp "$HERE/icons/$2.png" "$1"; }
else
  cd "$HERE/.."
  cargo build --release
  cargo build --release --example icon
  EXE=target/release/alef-term
  icon() { ./target/release/examples/icon "$1" "$2" >/dev/null; }
fi

BIN="${PREFIX:-$HOME/.local}/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
ICONS="$DATA/icons/hicolor"
mkdir -p "$BIN" "$DATA/applications"

# Replace rather than overwrite, so running windows keep their executable.
install -m 755 "$EXE" "$BIN/alef-term.new"
mv -f "$BIN/alef-term.new" "$BIN/alef-term"

for s in 48 128 256 512; do
  mkdir -p "$ICONS/${s}x${s}/apps"
  icon "$ICONS/${s}x${s}/apps/alef-term.png" $s
done

# StartupWMClass matches the window class the app sets, so the launcher and
# the dock show the icon for its windows.
cat > "$DATA/applications/alef-term.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Alef
Name[ar]=ألف
GenericName=Terminal
Comment=An Arabic-first terminal
Exec=$BIN/alef-term
Icon=alef-term
Terminal=false
Categories=System;TerminalEmulator;
Keywords=shell;terminal;console;arabic;rtl;
StartupWMClass=alef-term
DESKTOP

command -v update-desktop-database >/dev/null && update-desktop-database -q "$DATA/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$ICONS" || true

echo "installed $BIN/alef-term"
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) echo "note: $BIN is not on PATH; the launcher entry works regardless" ;;
esac
