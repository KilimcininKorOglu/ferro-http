# syntax=docker/dockerfile:1

# ---- builder: compile a fully static musl binary for the target arch ----
# Under `docker buildx --platform`, the whole file is built once per platform, so
# cargo compiles natively for each arch (alpine's default host is the *-musl
# target). The runtime stages then copy a binary that already matches the arch.
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev gcc
WORKDIR /build
COPY . .
RUN cargo build --release -p ferro

# ---- runtime: distroless (non-root, ca-certs); build with --target ----
# A static binary runs on the distroless static base; the `nonroot` tag drops
# privileges to uid 65532, which the scratch image cannot do (it runs as root).
FROM gcr.io/distroless/static-debian12:nonroot AS distroless-runtime
COPY --from=builder /build/target/release/ferro /ferro
COPY docker/config.json /etc/ferro/config.json
COPY docker/public /srv/www
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/ferro", "/etc/ferro/config.json"]

# ---- runtime: scratch (default, smallest image) ----
# Kept as the last stage so `docker build` and `docker compose` produce it by
# default; the distroless variant is opt-in via `--target distroless-runtime`.
FROM scratch AS scratch-runtime
COPY --from=builder /build/target/release/ferro /ferro
COPY docker/config.json /etc/ferro/config.json
COPY docker/public /srv/www
EXPOSE 8080
ENTRYPOINT ["/ferro", "/etc/ferro/config.json"]
