#!/bin/sh
# Removes Stress from ~/.local, and the system helpers it installed (asks for
# the sudo password then). Pass --purge to also delete the settings.
set -eu

cd "$(dirname "$0")"

PREFIX="${PREFIX:-$HOME/.local}"
APP_ID=io.github.stresstest.Stress
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/stresstest"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/stresstest"

rm -fv "$PREFIX/bin/stresstest" \
       "$PREFIX/share/applications/$APP_ID.desktop" \
       "$PREFIX/share/icons/hicolor/scalable/apps/$APP_ID.svg" \
       "$PREFIX/share/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"

rm -rf "$CACHE_DIR"

if [ -e /etc/udev/rules.d/60-stresstest-rapl.rules ]; then
    echo "==> Removing processor power access (asks for your sudo password)"
    sudo sh data/remove-rapl.sh
fi

gtk4-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true

if [ "${1:-}" = "--purge" ]; then
    rm -rfv "$CONFIG_DIR"
    echo "Stress and all its settings have been removed."
else
    echo "Stress has been removed. Settings are kept in $CONFIG_DIR"
    echo "(run with --purge to delete them too)."
fi
