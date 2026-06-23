# syntax=docker/dockerfile:1

# ---- builder: compile a fully static musl binary ----
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev gcc
WORKDIR /build
COPY . .
RUN cargo build --release -p ferro

# ---- runtime: the static binary on an empty image ----
FROM scratch
COPY --from=builder /build/target/release/ferro /ferro
COPY docker/config.json /etc/ferro/config.json
COPY docker/public /srv/www
EXPOSE 8080
ENTRYPOINT ["/ferro", "/etc/ferro/config.json"]
