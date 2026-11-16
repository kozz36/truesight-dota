#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/128x128/apps"

BIN_EXEC="${1:-}"
if [[ -z "$BIN_EXEC" ]]; then
  if command -v truesight-dota >/dev/null 2>&1; then
    BIN_EXEC="$(command -v truesight-dota)"
  elif [[ -x "$HOME/.local/bin/truesight-dota" ]]; then
    BIN_EXEC="$HOME/.local/bin/truesight-dota"
  else
    BIN_EXEC="truesight-dota"
  fi
fi

ICON_SRC=""
if [[ -f "$SCRIPT_DIR/truesight-dota.png" ]]; then
  ICON_SRC="$SCRIPT_DIR/truesight-dota.png"
elif [[ -f "$SCRIPT_DIR/../icons/truesight-dota.png" ]]; then
  ICON_SRC="$SCRIPT_DIR/../icons/truesight-dota.png"
else
  echo "Gem icon truesight-dota.png not found near $SCRIPT_DIR" >&2
  exit 1
fi

DESKTOP_SRC="$SCRIPT_DIR/truesight-dota.desktop"
if [[ ! -f "$DESKTOP_SRC" ]]; then
  echo "Desktop entry truesight-dota.desktop not found near $SCRIPT_DIR" >&2
  exit 1
fi

mkdir -p "$APP_DIR" "$ICON_DIR"
install -m 644 "$ICON_SRC" "$ICON_DIR/truesight-dota.png"
sed "s|^Exec=.*|Exec=$BIN_EXEC|" "$DESKTOP_SRC" > "$APP_DIR/truesight-dota.desktop"
chmod 644 "$APP_DIR/truesight-dota.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$DATA_HOME/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Installed $APP_DIR/truesight-dota.desktop and $ICON_DIR/truesight-dota.png (Exec=$BIN_EXEC)"
