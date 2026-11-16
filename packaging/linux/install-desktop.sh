#!/usr/bin/env bash
set -euo pipefail

script_parent="."
if [[ "${BASH_SOURCE[0]}" == */* ]]; then
  script_parent="${BASH_SOURCE[0]%/*}"
  [[ -z "$script_parent" ]] && script_parent="/"
fi
script_dir_raw=$(cd -- "$script_parent" && pwd -P && printf x)
SCRIPT_DIR="${script_dir_raw%$'\n'x}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/128x128/apps"

reject_cr_lf() {
  local val="$1"
  if [[ "$val" == *$'\n'* || "$val" == *$'\r'* ]]; then
    echo "Unsupported newline/carriage-return in Exec target path" >&2
    return 1
  fi
}

# Resolve target path into BIN_EXEC without $(...) command substitution stripping trailing newlines.
resolve_exec_target() {
  local candidate="${1:-}"
  if [[ -n "$candidate" ]]; then
    reject_cr_lf "$candidate"
    if [[ "$candidate" == */* ]]; then
      local dir_part="${candidate%/*}"
      local base_part="${candidate##*/}"
      [[ -z "$dir_part" ]] && dir_part="/"
      if [[ -d "$dir_part" ]]; then
        local dir_abs_raw
        dir_abs_raw=$(cd -- "$dir_part" && pwd -P && printf x)
        local dir_abs="${dir_abs_raw%$'\n'x}"
        if [[ "$dir_abs" == "/" ]]; then
          BIN_EXEC="/$base_part"
        else
          BIN_EXEC="$dir_abs/$base_part"
        fi
      else
        BIN_EXEC="$candidate"
      fi
    else
      BIN_EXEC="$candidate"
    fi
    return 0
  fi

  # Prefer the binary bundled beside this script inside an extracted release tarball.
  if [[ -x "$SCRIPT_DIR/truesight-dota" ]]; then
    BIN_EXEC="$SCRIPT_DIR/truesight-dota"
  elif cmd_raw=$(command -v truesight-dota 2>/dev/null && printf x); then
    BIN_EXEC="${cmd_raw%$'\n'x}"
  elif [[ -x "$HOME/.local/bin/truesight-dota" ]]; then
    BIN_EXEC="$HOME/.local/bin/truesight-dota"
  else
    BIN_EXEC="truesight-dota"
  fi
}

# Serialize Exec= into EXEC_VALUE across both Desktop Entry layers:
# 1) Exec argument quoting/escaping (\, ", `, $, and %% for literal %),
# 2) Desktop Entry `string` value escaping (\ -> \\).
format_desktop_exec() {
  local raw="$1"
  reject_cr_lf "$raw"

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
  printf -v EXEC_VALUE '%s' "${exec_encoded//\\/\\\\}"
}

BIN_EXEC=""
EXEC_VALUE=""
resolve_exec_target "${1:-}"
format_desktop_exec "$BIN_EXEC"

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
