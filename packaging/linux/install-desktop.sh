#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/128x128/apps"

resolve_exec_target() {
  local candidate="${1:-}"
  if [[ -n "$candidate" ]]; then
    if [[ "$candidate" == */* ]]; then
      local dir_part base_part
      dir_part="$(dirname -- "$candidate")"
      base_part="$(basename -- "$candidate")"
      if [[ -d "$dir_part" ]]; then
        printf '%s/%s' "$(cd -- "$dir_part" && pwd -P)" "$base_part"
      else
        printf '%s' "$candidate"
      fi
    else
      printf '%s' "$candidate"
    fi
    return 0
  fi

  # Prefer the binary bundled beside this script inside an extracted release tarball.
  if [[ -x "$SCRIPT_DIR/truesight-dota" ]]; then
    printf '%s/truesight-dota' "$SCRIPT_DIR"
  elif command -v truesight-dota >/dev/null 2>&1; then
    command -v truesight-dota
  elif [[ -x "$HOME/.local/bin/truesight-dota" ]]; then
    printf '%s/.local/bin/truesight-dota' "$HOME"
  else
    printf '%s' "truesight-dota"
  fi
}

# Serialize Exec= across both Desktop Entry layers:
# 1) Exec argument quoting/escaping (\, ", `, $, and %% for literal %),
# 2) Desktop Entry `string` value escaping (\ -> \\).
format_desktop_exec() {
  local raw="$1"
  if [[ "$raw" == *$'\n'* || "$raw" == *$'\r'* ]]; then
    echo "Unsupported newline/carriage-return in Exec target path" >&2
    return 1
  fi

  local exec_encoded="$raw"
  local reserved_re='[[:space:]"'\''<>~|&;$*?#()`\\]'
  if [[ "$raw" == */* || "$raw" =~ $reserved_re ]]; then
    exec_encoded="${exec_encoded//\\/\\\\}"
    exec_encoded="${exec_encoded//\"/\\\"}"
    exec_encoded="${exec_encoded//\`/\\\`}"
    exec_encoded="${exec_encoded//\$/\\\$}"
    exec_encoded="${exec_encoded//%/%%}"
    exec_encoded="\"$exec_encoded\""
  else
    exec_encoded="${exec_encoded//%/%%}"
  fi

  # Layer 2: escape backslashes for the .desktop string parser before writing Exec=.
  printf '%s' "${exec_encoded//\\/\\\\}"
}

BIN_EXEC="$(resolve_exec_target "${1:-}")"
EXEC_VALUE="$(format_desktop_exec "$BIN_EXEC")"

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

{
  while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ "$line" == Exec=* ]]; then
      printf 'Exec=%s\n' "$EXEC_VALUE"
    else
      printf '%s\n' "$line"
    fi
  done < "$DESKTOP_SRC"
} > "$APP_DIR/truesight-dota.desktop"
chmod 644 "$APP_DIR/truesight-dota.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$DATA_HOME/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Installed $APP_DIR/truesight-dota.desktop and $ICON_DIR/truesight-dota.png (Exec=$EXEC_VALUE)"
