#!/bin/sh
# Playground for Linux — per-user install (no root).
#
# Copies the two binaries into ~/.local/bin, the icon into
# ~/.local/share/icons, and the menu entry into
# ~/.local/share/applications. Run it from the folder you
# extracted Playground-Linux.zip into:
#
#     ./install.sh
#
# Playground keeps its settings and data in
# ~/.config/playground — uninstalling (delete the files
# named at the end of this script) leaves that untouched;
# delete it too if you want everything gone.
set -e

SRC="$(cd "$(dirname "$0")" && pwd)"
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/256x256/apps"

for f in playground update; do
    if [ ! -f "$SRC/$f" ]; then
        echo "install.sh: '$f' not found beside this script." >&2
        echo "Run install.sh from the extracted Playground-Linux folder." >&2
        exit 1
    fi
done

mkdir -p "$BIN" "$APPS" "$ICONS"

install -m 0755 "$SRC/playground" "$BIN/playground"
install -m 0755 "$SRC/update" "$BIN/update"

if [ -f "$SRC/playground.png" ]; then
    install -m 0644 "$SRC/playground.png" "$ICONS/playground.png"
fi

if [ -f "$SRC/playground.desktop" ]; then
    install -m 0644 "$SRC/playground.desktop" "$APPS/playground.desktop"
    # Point the entry at the installed binary, wherever
    # this user's home actually is.
    sed -i "s|^Exec=.*|Exec=$BIN/playground|" "$APPS/playground.desktop"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APPS" >/dev/null 2>&1 || true
    fi
fi

echo "Playground installed:"
echo "  $BIN/playground"
echo "  $BIN/update"
echo "  $APPS/playground.desktop"
echo
echo "Start it from your applications menu, or run: playground"
echo "Make sure $BIN is on your PATH (most desktops add it at next login)."
echo
echo "To uninstall: rm $BIN/playground $BIN/update $APPS/playground.desktop $ICONS/playground.png"
