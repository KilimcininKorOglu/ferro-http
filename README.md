# ferro

A near-zero-dependency, multi-platform HTTP server written in Rust.

ferro is built around a single allocation-only `no_std` core that holds all HTTP
logic, with two build-time profiles:

- **std** (default): high-performance event loop, serves from the filesystem.
  Targets Linux, macOS, Windows, Docker, and embedded Linux.
- **embedded**: `no_std` with smoltcp and compile-time embedded assets and
  config. Targets bare-metal systems. (Added in a later phase.)

## Workspace layout

- `crates/core` (`ferro-core`) — allocation-only `no_std` HTTP core.
- `crates/std-server` (`ferro`) — std-profile binary.

## Build and test

    cargo build --workspace
    cargo test --workspace

Verify the core stays `no_std` by cross-building it for a bare-metal target:

    cargo build -p ferro-core --target thumbv7em-none-eabi

## Status

Early scaffolding. The detailed work plan and architecture decisions live in the
project plan document alongside this repository.
