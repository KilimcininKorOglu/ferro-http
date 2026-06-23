# Multi-arch build definitions for ferro.
#
#   docker buildx bake              # scratch image, linux/amd64 + linux/arm64
#   docker buildx bake distroless   # non-root distroless variant, both arches
#
# A multi-platform image cannot be loaded into the local docker store; export it
# to a registry with `--push`, or to an OCI archive with
# `--set <target>.output=type=oci,dest=ferro.tar`.

variable "TAG" {
  default = "ferro:latest"
}

# Rust target triples corresponding to the Docker platforms below:
#   linux/amd64 -> x86_64-unknown-linux-musl
#   linux/arm64 -> aarch64-unknown-linux-musl
group "default" {
  targets = ["scratch"]
}

target "scratch" {
  context    = "."
  dockerfile = "Dockerfile"
  target     = "scratch-runtime"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = [TAG]
}

target "distroless" {
  context    = "."
  dockerfile = "Dockerfile"
  target     = "distroless-runtime"
  platforms  = ["linux/amd64", "linux/arm64"]
  tags       = ["ferro:distroless"]
}
