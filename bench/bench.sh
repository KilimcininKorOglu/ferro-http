#!/usr/bin/env bash
#
# Reproducible ferro load benchmark.
#
# Builds the release binary, serves a tiny static file plus the /api/health
# JSON route, and drives load with the first available tool (oha, wrk, then
# ApacheBench `ab`). The server is always stopped on exit.
#
# Numbers are INDICATIVE and single-box: they show relative behavior on the
# machine you run them on, not a portable throughput guarantee.
#
# Env overrides: PORT, CONCURRENCY, REQUESTS, WORKERS, DURATION (oha/wrk only).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-8799}"
CONCURRENCY="${CONCURRENCY:-50}"
REQUESTS="${REQUESTS:-200000}"
WORKERS="${WORKERS:-0}"
DURATION="${DURATION:-10s}"

work="$(mktemp -d)"
cleanup() {
  [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

printf '<!doctype html><title>bench</title><h1>ferro</h1>' >"$work/index.html"
cat >"$work/config.json" <<JSON
{
  "server": { "bind": "127.0.0.1", "port": $PORT, "worker_threads": $WORKERS },
  "static": { "root": "$work" },
  "logging": { "level": "warn" }
}
JSON

echo "==> building release"
cargo build --release -p ferro --manifest-path "$ROOT/Cargo.toml" >/dev/null

echo "==> starting server (port $PORT, workers $WORKERS)"
"$ROOT/target/release/ferro" "$work/config.json" >"$work/server.log" 2>&1 &
SRV=$!

# Wait for the port to accept connections (no curl: use bash /dev/tcp).
for _ in $(seq 1 50); do
  if (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then exec 3>&- 3<&-; break; fi
  sleep 0.1
done

run_load() {
  local url="$1" label="$2"
  echo "==> $label  ($url)"
  if command -v oha >/dev/null 2>&1; then
    oha -z "$DURATION" -c "$CONCURRENCY" --no-tui "$url"
  elif command -v wrk >/dev/null 2>&1; then
    wrk -t4 -c "$CONCURRENCY" -d "$DURATION" "$url"
  elif command -v ab >/dev/null 2>&1; then
    ab -k -c "$CONCURRENCY" -n "$REQUESTS" "$url" 2>/dev/null \
      | grep -E "Requests per second|Time per request|Failed requests|^  (50|99)%"
  else
    echo "    no load tool found (install oha, wrk, or ab)"
  fi
}

run_load "http://127.0.0.1:$PORT/" "static file"
run_load "http://127.0.0.1:$PORT/api/health" "api json"
