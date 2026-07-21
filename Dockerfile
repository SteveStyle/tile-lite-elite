# syntax=docker/dockerfile:1

# Single Dockerfile, multiple final targets — `runtime-server` and
# `runtime-web` both build from the same `builder` stage so Docker only
# compiles the workspace once. Select which one to build with `--target`
# (docker-compose.yml does this per service).

FROM rust:1-bookworm AS builder
WORKDIR /workspace

# dioxus-cli version pinned to match crates/ui's `dioxus`/`dioxus-web` deps
# (0.6.3) — a mismatched dx/wasm-bindgen version is a known source of wasm
# build failures in this project (see docs/operations.md). wasm-bindgen-cli
# itself must match the `wasm-bindgen` crate version pinned in Cargo.lock —
# `dx build` doesn't provision this automatically, so it's installed
# explicitly rather than left implicit.
RUN rustup target add wasm32-unknown-unknown \
    && cargo install dioxus-cli --version 0.6.3 --locked \
    && cargo install wasm-bindgen-cli --version 0.2.103 --locked

# .cargo/config.toml sets required wasm32 rustflags
# (target-feature=+reference-types,+multivalue) that wasm-bindgen needs to
# process the compiled binary — dropping this file breaks the wasm build
# with a misleading "failed to read file" error, not an obvious one. It also
# sets `rustc-wrapper = sccache` for fast local rebuilds, which doesn't
# exist in this image; RUSTC_WRAPPER="" below overrides that (env vars take
# precedence over the config file), matching what scripts/services.sh does
# for local wasm dev builds.
COPY .cargo ./.cargo
ENV RUSTC_WRAPPER=""

# Workspace manifests first, so dependency compilation is cached across
# rebuilds that only touch application code. old-crates/{first-try,
# second-try} are separate workspace members that depend on `srm-utils`, a
# path dependency (`../utils`, relative to the workspace root) living
# outside this repo entirely — Cargo needs *some* crate there to load the
# workspace manifest, even though nothing built here depends on it. A
# trivial stub satisfies that without needing the real, private, out-of-repo
# crate as part of the build context (this does mean old-crates' own real
# dependency tree gets dropped from the image's Cargo.lock, which is fine —
# old-crates aren't built here).
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY old-crates ./old-crates
RUN mkdir -p /utils/src \
    && printf '[package]\nname = "srm-utils"\nversion = "0.2.3"\nedition = "2021"\n' > /utils/Cargo.toml \
    && touch /utils/src/lib.rs

# Baked into both binaries via `option_env!` (see each crate's
# `app_version()`) as SemVer build metadata, e.g. `0.2.0+a1c9f02`. Passed
# through from docker-compose.yml's `build.args`, which scripts/deploy.sh
# sets to the current git short SHA — see docs/operations.md's
# "Versioning" section. Placed just before the two build steps below
# rather than at the top of the stage, so a rebuild of the same commit
# (same ARG value) still hits Docker's layer cache.
ARG TILE_LITE_ELITE_BUILD_ID
ENV TILE_LITE_ELITE_BUILD_ID=${TILE_LITE_ELITE_BUILD_ID}

RUN cargo build --release -p server-game -p admin-cli

# Built with an empty API base URL baked in — the client then talks to
# whatever origin it was served from (see `websocket_url` /
# `RootApp`'s `server_url` in crates/ui/src/app.rs), which is what lets one
# wasm build work behind the Caddy reverse proxy regardless of the host's
# actual IP or domain, with no rebuild needed if that changes.
RUN cd crates/ui && CARGO_INCREMENTAL=0 TILE_LITE_ELITE_API_BASE_URL="" dx build --platform web --release

# ---------------------------------------------------------------------------

FROM debian:bookworm-slim AS runtime-server
# curl is otherwise unused here — pulled in solely so HEALTHCHECK below has
# something to hit /health with, without reaching for a heavier base image.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data

COPY --from=builder /workspace/target/release/server-game /usr/local/bin/server-game
COPY --from=builder /workspace/target/release/tile-lite-elite-admin /usr/local/bin/tile-lite-elite-admin

# Not published by docker-compose.yml — reachable only from the `web`
# (Caddy) container over the compose network. `tile-lite-elite-admin` is run via
# `docker compose exec server tile-lite-elite-admin ...`, which is a genuinely
# loopback connection from inside this container, satisfying the server's
# existing loopback-only guard on /admin/* without weakening it.
EXPOSE 3000
# Gives `docker compose ps`/orchestration a real "unhealthy" signal instead
# of only "still running" — a hung server (e.g. deadlocked on the DB pool)
# would otherwise look identical to a working one until a request failed.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD curl -f http://localhost:3000/health || exit 1
ENTRYPOINT ["/usr/local/bin/server-game"]

# ---------------------------------------------------------------------------

FROM caddy:2-alpine AS runtime-web
COPY --from=builder /workspace/target/dx/tile-lite-elite-ui/release/web/public /srv
COPY Caddyfile /etc/caddy/Caddyfile
EXPOSE 80
# Goes through the same reverse-proxy path a real client uses, so this
# checks both "Caddy is up" and "Caddy can actually reach the server" —
# alpine's busybox wget is already present, no extra package needed.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD wget --no-verbose --tries=1 --spider http://localhost/health || exit 1
