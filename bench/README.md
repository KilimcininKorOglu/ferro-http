# Benchmark

`bench.sh` builds the release binary, serves a small static file plus the
`/api/health` JSON route, and drives load with the first available tool
(`oha`, `wrk`, then ApacheBench `ab`). The server is always stopped on exit.

## Run

```sh
./bench/bench.sh
# overrides:
PORT=9000 CONCURRENCY=100 REQUESTS=500000 WORKERS=8 ./bench/bench.sh
```

`WORKERS=0` (default) derives the reactor count from available parallelism.

## Indicative baseline

Single box, not a portable guarantee. macOS 26.5, 10 cores, `ab -k`,
concurrency 50, `WORKERS=0`, 0 failed requests:

| Route             | Requests/sec | p50  | p99  |
|-------------------|--------------|------|------|
| `/` (static file) | ~21,600      | 2 ms | 3 ms |
| `/api/health`     | ~102,400     | 0 ms | 1 ms |

The static route is slower because each request does filesystem work
(canonicalize plus read; no asset cache yet). Zero-copy `sendfile` and an
in-memory asset cache are the planned optimizations for that path.

These numbers are indicative: they show relative behavior on one machine, not
a cross-machine throughput claim. Re-run on your own hardware to compare.
