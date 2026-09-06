# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.96.1
ARG DEBIAN_VERSION=bookworm

FROM docker.io/library/rust:${RUST_VERSION}-slim-${DEBIAN_VERSION} AS build

WORKDIR /src

# We need to build against libssl
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential \
        cmake \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Fetch dependencies separately so changes to application code do not invalidate
# the dependency cache. Cache mounts also keep Cargo artifacts between builds.
COPY Cargo.toml Cargo.lock ./
# cargo fetch requires a source file to compile
RUN mkdir --parents src && printf 'fn main() {}\n' > src/main.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo fetch --locked

COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && install -D -m 0755 target/release/ph-webring /out/ph-webring

FROM docker.io/library/debian:${DEBIAN_VERSION}-slim AS runtime

LABEL org.opencontainers.image.description="Purdue Hackers webring server" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later" \
      org.opencontainers.image.source="https://github.com/purduehackers/webring"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        ca-certificates \
        curl \
        libssl3 \
        tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build --chown=0:0 --chmod=0755 /out/ph-webring /usr/bin/webring
COPY static /usr/share/webring/static
RUN install -d -m 0755 /etc/webring /var/cache/webring /var/lib/webring

WORKDIR /var/lib/webring

EXPOSE 80

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "--max-time", "2", "--output", "/dev/null", "http://[::1]/"]

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/bin/webring"]
CMD ["-f", "/etc/webring/webring.toml"]
