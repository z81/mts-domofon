# syntax=docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e

FROM node:24-bookworm-slim@sha256:3638d9a6fe4030bd716be989438248074489337ba3275657f93595428be4fc03 AS frontend-build

WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,id=npm-cache,target=/root/.npm,sharing=locked \
    npm ci

COPY frontend/ ./
RUN npm run build

FROM rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663 AS rust-build

ARG TARGETARCH

RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ ./src/
COPY vendor/ ./vendor/
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git-${TARGETARCH},target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/app/target,sharing=locked \
    mkdir -p /out \
    && cargo build --locked --release \
    && install --mode=0755 /app/target/release/domru-mts /out/domru-mts

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime

ARG OCI_REVISION
ARG OCI_VERSION
ARG OCI_CREATED

LABEL org.opencontainers.image.title="domru-mts" \
      org.opencontainers.image.description="Unofficial local MTS/VDome intercom client" \
      org.opencontainers.image.source="https://github.com/z81/mts-domofon" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.licenses="NOASSERTION"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl ffmpeg libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 domru-mts \
    && useradd --system --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin domru-mts \
    && install --directory --mode=0700 --owner=10001 --group=10001 /var/lib/domru-mts \
    && mkdir -p /opt/domru-mts/frontend

COPY --from=rust-build /out/domru-mts /usr/local/bin/domru-mts
COPY --from=frontend-build /app/frontend/dist /opt/domru-mts/frontend/dist

ENV VDOME_BIND=127.0.0.1:3000 \
    VDOME_DATA_DIR=/var/lib/domru-mts \
    VDOME_WEB_DIST_DIR=/opt/domru-mts/frontend/dist \
    TMPDIR=/tmp

USER 10001:10001
EXPOSE 3000
STOPSIGNAL SIGINT
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=6 CMD curl --noproxy '*' --fail --silent --show-error --max-time 2 http://127.0.0.1:3000/health/live >/dev/null || curl --noproxy '*' --fail --silent --show-error --max-time 2 'http://[::1]:3000/health/live' >/dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/domru-mts"]
