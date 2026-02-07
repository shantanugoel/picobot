#!/usr/bin/env bash
set -euo pipefail

DB_PATH="${1:-}"

if [[ -z "$DB_PATH" ]]; then
  CONFIG_PATH="${PICOBOT_CONFIG:-picobot.toml}"
  if [[ -f "$CONFIG_PATH" ]]; then
    DB_DIR=$(python3 - <<'PY'
import os
import sys
try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib

config_path = os.environ.get("PICOBOT_CONFIG", "picobot.toml")
with open(config_path, "rb") as handle:
    data = tomllib.load(handle)
data_dir = data.get("data_dir")
if not data_dir:
    home = os.path.expanduser("~")
    data_dir = os.path.join(home, "Library", "Application Support", "picobot")
print(data_dir)
PY
)
  else
    DB_DIR="$HOME/Library/Application Support/picobot"
  fi
  DB_PATH="$DB_DIR/sessions.db"
fi

if [[ ! -f "$DB_PATH" ]]; then
  echo "usage: $0 /path/to/sessions.db" >&2
  echo "missing database: $DB_PATH" >&2
  exit 1
fi

echo "Database: $DB_PATH"
echo ""

echo "== Usage by user =="
sqlite3 -header -column "$DB_PATH" <<'SQL'
SELECT
  user_id,
  COUNT(*) AS requests,
  SUM(input_tokens) AS input_tokens,
  SUM(output_tokens) AS output_tokens,
  SUM(total_tokens) AS total_tokens,
  SUM(cached_input_tokens) AS cached_input_tokens
FROM usage_events
GROUP BY user_id
ORDER BY total_tokens DESC, requests DESC;
SQL

echo ""
echo "== Usage by channel =="
sqlite3 -header -column "$DB_PATH" <<'SQL'
SELECT
  channel_id,
  COUNT(*) AS requests,
  SUM(input_tokens) AS input_tokens,
  SUM(output_tokens) AS output_tokens,
  SUM(total_tokens) AS total_tokens,
  SUM(cached_input_tokens) AS cached_input_tokens
FROM usage_events
GROUP BY channel_id
ORDER BY total_tokens DESC, requests DESC;
SQL

echo ""
echo "== Usage by model =="
sqlite3 -header -column "$DB_PATH" <<'SQL'
SELECT
  model,
  COUNT(*) AS requests,
  SUM(input_tokens) AS input_tokens,
  SUM(output_tokens) AS output_tokens,
  SUM(total_tokens) AS total_tokens,
  SUM(cached_input_tokens) AS cached_input_tokens
FROM usage_events
GROUP BY model
ORDER BY total_tokens DESC, requests DESC;
SQL

echo ""
echo "== Usage by day =="
sqlite3 -header -column "$DB_PATH" <<'SQL'
SELECT
  substr(created_at, 1, 10) AS day,
  COUNT(*) AS requests,
  SUM(input_tokens) AS input_tokens,
  SUM(output_tokens) AS output_tokens,
  SUM(total_tokens) AS total_tokens,
  SUM(cached_input_tokens) AS cached_input_tokens
FROM usage_events
GROUP BY day
ORDER BY day DESC;
SQL
