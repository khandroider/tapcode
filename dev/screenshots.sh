#!/bin/sh
# Takes README / site screenshots from the sample project (dev/demo.sh) with clean settings.
# Usage: sh dev/screenshots.sh <demo-dir> <out-dir>
# Needs: target/release/tapcode, node (npx), tapcode-pg from dev/docker-compose.yml for the db shot.
set -eu
DEMO="$1"; OUT="$2"
cd "$(dirname "$0")/.."
BIN=target/release/tapcode
CFG="$HOME/Library/Application Support/tapcode/settings.toml"
mkdir -p "$OUT" "$(dirname "$CFG")"

# Use clean settings so no personal projects, files or connections show up. Restore on exit.
BAK="$(mktemp)"
[ -f "$CFG" ] && cp "$CFG" "$BAK"
trap '[ -s "$BAK" ] && cp "$BAK" "$CFG" || rm -f "$CFG"; rm -f "$BAK"' EXIT
cat > "$CFG" <<'TOML'
theme = "palenight"
autosave = true
autosave_delay_ms = 1000
font_size = 16.0
recent_projects = []
recent_files = []
lsp_php = ["npx", "-y", "intelephense", "--stdio"]
lsp_ts = ["npx", "-y", "typescript-language-server", "--stdio"]
tsserver_path = ""

[[connections]]
name = "dev-pg"
kind = "Postgres"
host = "127.0.0.1"
port = 5440
user = "postgres"
database = "demo"
read_only = true
TOML

shot() { # name delay demo [files...]
  name="$1"; delay="$2"; demo="$3"; shift 3
  echo "-> $name"
  TAPCODE_SCREENSHOT="$OUT/$name.png" TAPCODE_SCREENSHOT_DELAY="$delay" TAPCODE_DEMO="$demo" \
    "$BIN" "$DEMO" "$@" >/dev/null 2>&1 || echo "   failed: $name"
}

shot screenshot         3  ""                     "$DEMO/src/Cart.php"
shot search             4  "all:cart"             "$DEMO/src/Cart.php"
shot completion         12 "lsp:15:25"            "$DEMO/src/Cart.php"
shot git                4  "git:commit"           "$DEMO/src/Cart.php"
shot database-readonly  6  "db:dev-pg:devpass:UPDATE products SET price_cents = 0" "$DEMO/src/Cart.php"
shot terminal           4  "terminal:"            "$DEMO/src/Cart.php"
ls -la "$OUT"
