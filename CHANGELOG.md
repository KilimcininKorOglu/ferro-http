# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and this project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Support for the HTTP QUERY method (RFC 10008): a safe, idempotent request
  whose content describes a server-side query. A `query_content_type_check`
  helper enforces the media type (400 when absent, 415 with an `Accept-Query`
  field when unsupported), and a demo `QUERY /api/search` endpoint shows the
  flow alongside the GET API.
- Method-aware discovery for router resources: `OPTIONS` returns the resource's
  `Allow` set and an unsupported method returns `405 Method Not Allowed` with
  `Allow`, instead of `404`. Static paths remain GET/HEAD-only.

### Fixed
- A HEAD request to a GET API route is now served (its body dropped at
  serialization) instead of falling through to a 404.

## [1.0.1] - 2026-06-23

### Added
- A `min-size` Cargo profile (`opt-level = "z"`, `panic = "abort"`) and
  `config.embedded.json` for tight embedded Linux targets such as OpenWRT.

### Changed
- The release workflow now also builds OpenWRT binaries (`aarch64` and `armv7`
  musl) via `cross`.
- Documentation: refreshed the project description in the README.

## [1.0.0] - 2026-06-23

First stable release. A near-zero-dependency, JSON-configured, multi-platform
HTTP/1.1 server built around a single allocation-only `no_std` core.

### Added
- Allocation-only `no_std` HTTP/1.1 core: incremental parser, response builder,
  pattern-matching router, and a transport-agnostic connection state machine.
- JSON parser and JSON-backed configuration with full defaults; a config
  serializer so settings can be written back.
- std profile: a mio event-loop server with `SO_REUSEPORT` reactor sharding and
  end-to-end static file serving.
- `Date`/`Connection` headers with header-injection protection, MIME resolution
  by extension with config overrides, and config-gated security headers.
- Chunked `Transfer-Encoding` request-body decoding and `request_max_bytes`
  enforcement.
- Static file serving hardened against path traversal and symlink escape, with
  `follow_symlinks` enforcement.
- Per-peer rate limiting (fixed window with progressive ban) and access logging.
- Graceful shutdown on `SIGINT`/`SIGTERM` that drains in-flight connections.
- Optional `gzip` response compression (default off).
- Optional `tls` TLS 1.2/1.3 termination via rustls (default off).
- Optional `webui` embedded admin panel (default off): a single responsive page
  with live config editing and hot-reload, statistics, a password-change modal,
  and English/Turkish i18n, behind HTTP Basic auth with a hand-rolled SHA-256
  password hash. No external resources.
- Embedded profile: a `no_std` smoltcp transport, a `StaticRouter` service,
  compile-time embedded assets, and a bare-metal Cortex-M firmware binary.
- Version display via the CLI `--version`/`-V` flag and in the admin panel.

### Changed
- Performance: `TCP_NODELAY` on accepted connections, accept sharded across
  `SO_REUSEPORT` reactor threads, and a reproducible load-benchmark harness.
- Docker: a musl-static image with multi-arch (amd64 + arm64) and non-root
  distroless variants.
- CI: a cross-platform matrix that also covers the optional features and the
  embedded/no_std cross-builds, plus a release workflow for version tags.

### Fixed
- Drain TLS plaintext between reads so request bodies larger than rustls'
  internal buffer are served instead of dropping the connection.
