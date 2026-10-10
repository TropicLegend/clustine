# syntax=docker/dockerfile:1

# One image for every Clustine service. It holds the `clustine` binary, whose subcommand
# says which service a container is, and `clustine-botswarm`, the scripted test client.
#
#     docker build -t clustine:dev .
#
# Nothing here needs BuildKit, so the image also builds where Docker's buildx plugin is
# not installed. The price is that a change to any source file compiles everything
# again: only whole layers are cached.

# Both stages are the same Debian release. The binaries link dynamically against glibc
# (and libgcc, which comes with it) and against nothing else, and a binary that was built
# against a newer glibc than the one it finds does not start.
ARG DEBIAN_RELEASE=trixie
# Where the two base images come from. Docker Hub limits how often a machine may pull
# without signing in, and GitHub's runners share addresses: the workflow that builds
# this image there names a mirror of Docker Hub's official images instead.
ARG REGISTRY=docker.io/library

FROM ${REGISTRY}/rust:1-slim-${DEBIAN_RELEASE} AS build

WORKDIR /src

# .dockerignore lets through only what cargo reads, so a change anywhere else in the
# repository leaves this layer and the build after it cached.
COPY . .

# The slim image has the C compiler that zstd and blake3 are built with, and nothing
# else is needed.
RUN cargo build --release --locked -p clustine -p clustine-botswarm \
    && mkdir /out \
    && cp target/release/clustine target/release/clustine-botswarm /out/


FROM ${REGISTRY}/debian:${DEBIAN_RELEASE}-slim

LABEL org.opencontainers.image.title="Clustine" \
      org.opencontainers.image.source="https://github.com/TropicLegend/clustine" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"

# The user has a fixed number rather than only a name: Kubernetes can then see that it
# is not root, and the manifests in deploy/kubernetes name the same number as `fsGroup`.
# /data is where the world store is told to keep the world; it belongs to the user so
# that this also works without a volume mounted there.
RUN groupadd --gid 10001 clustine \
    && useradd --uid 10001 --gid 10001 --no-log-init --no-create-home \
        --home-dir /data --shell /usr/sbin/nologin clustine \
    && install -d -o clustine -g clustine /data

COPY --from=build /out/ /usr/local/bin/

# The services colour their log unless told not to, whether or not a terminal reads it,
# and what collects a container's output has no use for the escape codes.
ENV NO_COLOR=1

USER 10001:10001
WORKDIR /data

# No shell in between: the service is process 1 and receives SIGTERM itself, on which it
# shuts down cleanly.
ENTRYPOINT ["clustine"]
