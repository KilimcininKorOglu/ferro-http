# ferro

A near-zero-dependency, JSON-configured, multi-platform HTTP/1.1 server written in Rust.

ferro is built around a single allocation-only `no_std` core that holds all HTTP
logic, exposed through narrow seam traits and driven by two build-time profiles:

- **std** (default): a high-performance mio event loop (one `SO_REUSEPORT` reactor
  per core) that serves files from the filesystem. Targets Linux, macOS, Windows,
  Docker, and embedded Linux.
- **embedded**: `no_std` over smoltcp with compile-time-embedded assets and config.
  Targets bare-metal Cortex-M.

The core has zero required dependencies; the std profile adds only mio, socket2,
and signal-hook. No async runtime, no framework.

## Features

- HTTP/1.1 with keep-alive, request pipelining, and chunked transfer decoding.
- Static file serving with path-traversal and symlink-escape protection, MIME
  detection, and configurable index files.
- A pattern-matching API router (`:param` routes, JSON responses).
- Per-IP rate limiting (fixed window, progressive ban with recovery) and
  configurable security headers.
- JSON configuration with full defaults; malformed config fails loudly.
- Graceful shutdown (SIGINT/SIGTERM) that drains in-flight connections.
- Optional, default-off cargo features:
  - `gzip` — response compression (miniz_oxide).
  - `tls` — TLS 1.2/1.3 termination (rustls, ring backend; no cmake needed).
  - `webui` — an embedded, responsive web admin panel (English/Turkish) for live
    config editing with hot-reload, statistics, and password change, behind HTTP
    Basic auth. No external resources; everything is baked into the binary.

## Workspace layout

- `crates/core` (`ferro-core`) — allocation-only `no_std` HTTP core (zero deps).
- `crates/std-server` (`ferro`) — std-profile binary (mio event loop).
- `crates/embedded` (`ferro-embedded`) — `no_std` smoltcp transport; host-buildable
  so its loopback transport test runs under `cargo test`.
- `crates/embedded-server` — bare-metal Cortex-M firmware. Excluded from the
  workspace and built standalone for `thumbv7em-none-eabi`.

## Build and test

    cargo build --workspace
    cargo test --workspace

Verify the `no_std` crates still cross-build for a bare-metal target:

    cargo build -p ferro-core --target thumbv7em-none-eabi
    cargo build -p ferro-embedded --target thumbv7em-none-eabi
    (cd crates/embedded-server && cargo build)   # links the firmware ELF

Optional features are off by default and are not compiled by the plain build;
enable them explicitly:

    cargo build -p ferro --release --features "gzip,tls,webui"
    cargo test  -p ferro --features tls           # exercises feature-gated code

## Running

    ferro config.json            # or: cargo run -p ferro -- config.json

With no argument ferro looks for `./config.json`, falling back to built-in
defaults if it is absent.

## Configuration

Configuration is a single JSON file; every field has a default, so a partial file
is valid. A representative `config.json`:

    {
      "server": { "bind": "0.0.0.0", "port": 8080, "worker_threads": 0,
                  "max_connections": 1024, "keep_alive_secs": 15,
                  "request_max_bytes": 1048576 },
      "static": { "root": "./public", "index_files": ["index.html"],
                  "follow_symlinks": false },
      "compression": { "gzip": true, "min_bytes": 1024 },
      "security": { "enable_security_headers": true,
                    "rate_limit": { "enabled": true, "requests": 600,
                                    "window_secs": 600, "ban_secs": 300 } },
      "tls": { "enabled": false, "cert_path": "", "key_path": "" },
      "admin": { "username": "admin", "password_sha256": "<sha256-hex of password>" },
      "logging": { "level": "info", "access_log": true }
    }

`worker_threads: 0` derives the reactor count from available CPUs. With the `tls`
feature, set `tls.enabled` plus the PEM `cert_path`/`key_path`. With the `webui`
feature and an `admin` username + `password_sha256` set, the admin panel is served
at `/admin` (HTTP Basic auth); changes saved there are written back to this file.

## Docker

The std binary ships as a fully static musl build. The default image is `scratch`:

    docker build -t ferro .
    docker compose up                # serves on host port 8180

A hardened, non-root variant on distroless is opt-in:

    docker build --target distroless-runtime -t ferro:distroless .

Multi-architecture images (`linux/amd64` + `linux/arm64`) are built with buildx
bake; a multi-platform image must be pushed to a registry or exported as an OCI
archive rather than loaded into the local store:

    docker buildx bake               # scratch, both arches
    docker buildx bake distroless    # distroless, both arches

## Status

Feature-complete for the v1 scope (static file server + API router). The std
profile is live-tested and benchmarked; the embedded transport is verified on the
host via a smoltcp loopback, while running the firmware on real hardware or QEMU
is not yet verified. The phased work plan and locked architecture decisions live
in the project plan document alongside this repository.
