# ferro

A near-zero-dependency, multi-platform HTTP server written in Rust.

ferro is built around a single allocation-only `no_std` core that holds all HTTP
logic, with two build-time profiles:

- **std** (default): high-performance event loop, serves from the filesystem.
  Targets Linux, macOS, Windows, Docker, and embedded Linux. Optional, default-off
  cargo features add gzip compression and TLS termination (rustls).
- **embedded**: `no_std` with smoltcp and compile-time embedded assets and
  config. Targets bare-metal systems.

## Workspace layout

- `crates/core` (`ferro-core`) — allocation-only `no_std` HTTP core.
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

Early scaffolding. The detailed work plan and architecture decisions live in the
project plan document alongside this repository.
