#!/usr/bin/env bash
# End-to-end smoke test on macOS and Linux: install PHP and services through tez, serve a site,
# check PHP, PostgreSQL through PDO and the metrics API, then stop. Used by CI and releases.
set -euo pipefail
BIN="${1:-./target/release/tez}"
trap 'echo "---- logs"; tail -n 40 "$HOME"/.tez/logs/*.log 2>/dev/null || true' ERR

"$BIN" install --php 8.4 --services redis,postgres
export PATH="$HOME/.tez/bin:$PATH"
tez version
php -v | head -1
php -r 'echo implode(",", PDO::getAvailableDrivers()), "\n";'

mkdir -p "$HOME/Sites/hello/public"
echo '<?php echo "tez-ok-" . PHP_VERSION;' > "$HOME/Sites/hello/public/index.php"
tez start
HTTP=$(tez status --json | jq -r .http_port)
body=""
for _ in $(seq 1 60); do
  body=$(curl -s -H 'Host: hello.localhost' "http://127.0.0.1:$HTTP/" || true)
  case "$body" in tez-ok-8.4.*) break ;; esac
  sleep 1
done
echo "site says: $body"
[[ "$body" == tez-ok-8.4.* ]]

curl -sf "http://127.0.0.1:7979/api/metrics?range=15m" | jq -e '.machine.total > 0' >/dev/null
echo "metrics: ok"

for _ in $(seq 1 30); do tez db create smoke && break; sleep 2; done
# The DB_HOST tez writes into a project's .env: the socket folder on macOS (see services::pg_host).
PGH=127.0.0.1; [ "$(uname)" = Darwin ] && PGH="$HOME/.tez/run"
PGH="$PGH" php -r '$p = new PDO("pgsql:host=" . getenv("PGH") . ";port=5432;dbname=smoke", "postgres", ""); echo $p->query("select 40+2")->fetchColumn(), "\n";' | grep -q 42
echo "postgres through PDO ($PGH): ok"

tez stop
