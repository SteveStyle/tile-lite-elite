# Scrabble PX - Development and Operating Guide

## Quick Start (Scripts)

The `scripts/` folder contains helpers to start, stop, and monitor services:

```bash
# Start backend server + web dev client (background)
./scripts/services.sh start

# Check status
./scripts/services.sh status

# Launch a desktop client (foreground, can run multiple times)
./scripts/desktop.sh

# Stop all services
./scripts/services.sh stop
```

**Partial restarts** (useful if one service fails but you want to keep the other running):
```bash
# Restart only the web dev server (doesn't affect desktop clients)
./scripts/services.sh restart-web

# Restart only the backend server (doesn't affect web UI)
./scripts/services.sh restart-server
```

For development (both services in foreground):
```bash
./scripts/services.sh dev
# Ctrl+C to stop
```

View logs while running:
```bash
./scripts/services.sh logs
```

## Prerequisites

- Rust stable toolchain (`rustup install stable`)
- `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
- Dioxus CLI (`cargo install dioxus-cli`)
- wasm-bindgen CLI matching `Cargo.lock` (`cargo install wasm-bindgen-cli --version 0.2.103`)

## Architecture

The project is split into a server and two client types:

```
Backend (port 3000)   ←→   Web UI (port 8080, WASM in browser)
                      ←→   Desktop UI (native window)
```

The backend owns all game state, rule enforcement, scoring, and persistence.
Clients are thin presentation layers that call the server API.

## Starting Services

### Using Scripts (Recommended)

```bash
./scripts/services.sh start
```

This starts both the backend and web client in the background, with logging to `.logs/`.

### Manual: Backend Only

From the workspace root:

```bash
cargo run -p server-game
```

Defaults:
- Bind: `127.0.0.1:3000`
- Database: `./data/scrabble-px.sqlite3` (auto-created with migrations on first run)

Override with environment variables:
```bash
SCRABBLE_PX_BIND=0.0.0.0:3000 SCRABBLE_PX_DATABASE_URL=sqlite://data/scrabble-px.sqlite3 cargo run -p server-game
```

### Manual: Web Client

From `crates/ui/`:
```bash
RUSTC_WRAPPER="" CARGO_INCREMENTAL=0 ~/.cargo/bin/dx serve --platform web --port 8080
```

> **Note**: `RUSTC_WRAPPER=""` disables sccache for the WASM build. sccache is
> incompatible with the `wasm32-unknown-unknown` target and will hang the build.

### Manual: Desktop Client

From the workspace root:
```bash
SCRABBLE_PX_API_BASE_URL=http://127.0.0.1:3000 cargo run -p scrabble-ui --features desktop
```

## Admin CLI

`scrabble-admin` (`crates/admin-cli`) is operator tooling for a running server — list/delete users, reset a password, list/delete/force-end games. It's a thin HTTP client against `server-game`'s `/admin/*` endpoints, not a separate implementation, so it can't drift from what the server actually does (cascading deletes, password hashing, etc. all stay server-side).

```bash
cargo run -p admin-cli -- users list
cargo run -p admin-cli -- users reset-password <player_id>          # prints a generated password
cargo run -p admin-cli -- users reset-password <player_id> --password 'a specific one'
cargo run -p admin-cli -- users delete <player_id>

cargo run -p admin-cli -- games list
cargo run -p admin-cli -- games list --status waiting
cargo run -p admin-cli -- games list --older-than-days 30
cargo run -p admin-cli -- games delete <game_id>
cargo run -p admin-cli -- games force-end <game_id>
```

Or build once and run the binary directly: `cargo build -p admin-cli` produces `target/debug/scrabble-admin`.

**There's no admin account or token.** The `/admin/*` endpoints only accept requests whose peer address is loopback (`127.0.0.1`/`::1`), regardless of what `SCRABBLE_PX_BIND` is set to — running the CLI *from the server's own terminal* is the access control. Point `--server`/`SCRABBLE_PX_API_BASE_URL` at anything else and every request 403s, by design. This matters specifically because `SCRABBLE_PX_BIND=0.0.0.0:3000` (see the LAN-play example above) would otherwise expose these endpoints to the whole LAN, not just the machine running the server.

Deleting a user unclaims their seats (`player_id` set to null on `game_participants`) rather than deleting their games — game history and other players' records survive.

## Build from Scratch (cold)

First-time WASM build takes ~50 seconds. Subsequent incremental builds are ~10 seconds.

```bash
# 1. Backend
cargo build -p server-game

# 2. Web (WASM)
cd crates/ui
RUSTC_WRAPPER="" CARGO_INCREMENTAL=0 ~/.cargo/bin/dx build --platform web

# 3. Desktop
cargo build -p scrabble-ui --features desktop
```

## Running Tests

```bash
cargo test --workspace
```

Runs every crate's tests, including the `old-crates/*` prototypes (harmless — see `history.md`). To run just one crate:
```bash
cargo test -p rules-shared    # rules/scoring/validation unit tests
cargo test -p server-game     # HTTP-level integration tests against the real Axum router
cargo test -p scrabble-ui     # move-composer logic, game-creation seat presets
cargo test -p engine-core     # engine tests
```

No test coverage for `admin-cli` (it's a thin HTTP client with no logic of its own to test in isolation) or for the WASM target specifically — `cargo test` always runs against the host target, not `wasm32-unknown-unknown`; see "Manual: Web Client" above for how to sanity-check a WASM build compiles.

## Configuration

| Environment variable | Default | Description |
|---|---|---|
| `SCRABBLE_PX_BIND` | `127.0.0.1:3000` | Server listen address |
| `SCRABBLE_PX_DATABASE_URL` | `sqlite://data/scrabble-px.sqlite3` | SQLite database path |
| `SCRABBLE_PX_API_BASE_URL` | `http://127.0.0.1:3000` | Backend URL used by clients. Set at *build* time (`option_env!`), not runtime. An explicit empty string means "same origin as the page" — see [Container Deployment](#container-deployment) |
| `SCRABBLE_UI_PORT` | `8080` | Web dev server port |

## Resetting the Database

Occasionally useful during development — for example, after a schema change that only takes effect on a fresh database (see the migration limitation note in `schema.md`), or just to clear out test data.

```bash
./scripts/services.sh stop
rm -f data/scrabble-px.sqlite3 data/scrabble-px.sqlite3-wal data/scrabble-px.sqlite3-shm
./scripts/services.sh start
```

Notes:
- The server **must** be stopped first — it holds an open connection and an in-memory copy of every active game, so deleting the file while it's running has no visible effect until restart.
- The `-wal`/`-shm` files are SQLite's write-ahead-log sidecar files. They may not exist depending on journal mode; `rm -f` won't complain either way.
- `persistence::connect` explicitly sets `create_if_missing(true)`, so the server recreates the file with a fresh schema on startup. (Without this, connecting to a missing file fails with `SqliteError { code: 14, message: "unable to open database file" }` — this bit us once in practice.)

## Container Deployment

`Dockerfile`, `docker-compose.yml`, and `Caddyfile` at the repo root build and run the app as two containers:

- **`server`** — the Axum backend (`server-game`), release-built. Not published to the host; only reachable from `web` over the compose network, and via `docker compose exec`.
- **`web`** — Caddy, serving the release web build (`dx build --platform web --release`) as static files, reverse-proxying API/WebSocket paths to `server`, and handling automatic HTTPS. Published on `:80` and `:443`.

```bash
docker compose build
docker compose up -d
```

SQLite lives on a named volume (`scrabble-data`, mounted at `/data` in `server`) — it survives `docker compose down` and rebuilds, but not `docker compose down -v`. Back it up with:

```bash
docker run --rm -v scrabble-px_scrabble-data:/data -v "$PWD":/backup debian \
  tar czf /backup/scrabble-data.tgz -C /data .
```

Caddy's obtained TLS certificate lives on its own named volumes (`caddy-data`, `caddy-config`) for the same reason — losing them means a fresh certificate request on next start, not a functional problem, just unnecessary churn against Let's Encrypt's rate limits.

**Admin CLI**: `/admin/*` stays loopback-only exactly as it is locally — a request proxied in from the `web` container isn't a loopback connection, so the server rejects it the same as it would over a LAN. Reach it via:

```bash
docker compose exec server scrabble-admin games list
```

**Why one image serves both, same-origin**: the web build is compiled with `SCRABBLE_PX_API_BASE_URL=""` (explicitly empty, not unset — see the Configuration table above), which makes the client derive its API/WebSocket target from whatever origin actually served the page (`crates/ui/src/app.rs`'s `websocket_url`/`same_origin_websocket_url`). That's what lets the same compiled wasm bundle work regardless of the host's IP or domain, with no rebuild needed if either changes — and it sidesteps CORS entirely, since Caddy serves both the static assets and the proxied API from one origin.

### Redeploying (after a code change)

The live VM has 1GB RAM — not enough to compile the Rust/wasm workspace — so images are always built locally and shipped over, never built on the VM itself. `scripts/deploy.sh` automates the whole cycle:

```bash
./scripts/deploy.sh
```

This builds both images locally, `docker save`s and gzips them, `scp`s them plus `docker-compose.yml` to the VM, `docker load`s them there, and runs `docker compose up -d`. Takes a few minutes, almost all of it the local build. Configurable via env vars (`DEPLOY_HOST`, `DEPLOY_USER`, `DEPLOY_SSH_KEY`, `DEPLOY_REMOTE_DIR`) if the target ever changes — see the script header.

There's no CI and no registry involved — this is a manual, on-demand push from a developer machine, appropriate for a hobby project's actual deploy frequency. Worth revisiting (e.g. push to a registry, `docker compose pull` on the VM instead of scp/load) if that ever changes.

### Oracle Cloud VM setup

The live instance runs on Oracle Cloud's **Always Free** tier — genuinely free indefinitely, not a trial (unlike some competitors that dropped their free tiers). Setting one up from scratch:

1. **Sign up** at `cloud.oracle.com` — needs phone + card verification (identity check, not a charge, as long as usage stays within Always Free limits).
2. **Convert to Pay-As-You-Go** (Account Management → "Upgrade Account") *before* creating the instance. This is the step that exempts the instance from Oracle's idle-reclamation policy (idle compute instances — 95th-percentile CPU under ~20% over a rolling 7-day window — get reclaimed on free-tier-only accounts; a low-traffic game server easily qualifies as idle). Staying within Always Free resource limits after upgrading still costs $0. This is a policy detail worth reverifying against Oracle's current docs before relying on it — it's the kind of thing that changes.
3. **Create the compute instance**: Ubuntu (24.04 LTS), shape `VM.Standard.E2.1.Micro` (Always Free eligible, 1 OCPU/1GB, x86_64 — matters for matching a normal x86_64 build machine with no cross-compilation) or the larger Ampere A1 shape if you'd rather have the RAM to build on the VM itself (2 OCPU/12GB on free tier as of mid-2026, more on PAYG). Assign a public IPv4 address. Paste an SSH public key at creation time rather than trying to recover/reuse an old instance's key later.
4. **Networking** — this is where almost all the actual setup time goes, and it's easy to get wrong in ways that fail silently (a timeout, no error message):
   - If reusing an existing VCN, check whether it already has a route to an internet gateway. The instance's **Networking** tab has a "Connect public subnet to internet" quick action if not — it also creates a new network security group, but leaves it with no inbound rules (only allows egress), so it alone won't get you reachable.
   - **A VCN can have more than one Security List, and only the one actually attached to your specific subnet matters.** We hit this directly: a "Default Security List" had SSH already open, but the subnet in use was actually attached to a *different*, completely empty security list (no ingress, no egress — blocking outbound too, e.g. `apt-get`). Check **Subnet → Security tab** (not the subnet's Details tab, which doesn't show this) to see which list(s) are really attached, and add ingress rules there specifically. Traffic is allowed if *either* the security list *or* an NSG permits it (not both required), so one correctly-configured security list is enough — no need to also chase down the NSG the quick action created.
   - Ingress rules needed: TCP 22 (SSH), TCP 80 (HTTP), TCP 443 + UDP 443 (HTTPS, the UDP for optional HTTP/3). Egress: allow all — the default "TCP only" some security lists ship with can block outbound DNS (UDP 53), stopping `apt-get`/Docker pulls from working even though the instance is otherwise reachable.
5. **Install Docker** on the VM (standard `apt` install from Docker's official repo — see their docs) and add a swapfile given the tight RAM (`fallocate`/`mkswap`/`swapon`, persist via `/etc/fstab`) — running the containers fits comfortably in 1GB, but the extra headroom is cheap insurance.
6. **`scp` the SSH key's *public* half only** into the instance creation form; keep the private key local (`~/.ssh/oracle_scrabble` in this setup) — it's what `scripts/deploy.sh` and manual `ssh`/`scp` commands use.

### HTTPS

Caddy provisions and renews TLS certificates automatically via Let's Encrypt — the entire config is giving it a real hostname instead of a bare `:80` in the `Caddyfile` (Let's Encrypt won't certificate a bare IP address). This deployment uses [sslip.io](https://sslip.io) rather than a purchased domain: `<ip>.sslip.io` (e.g. `129.151.69.246.sslip.io`) resolves straight back to that IP with no registration or DNS configuration needed, and Let's Encrypt validates and issues a real, browser-trusted certificate against it immediately. The URL is uglier than a real domain, but it's free and required zero setup beyond editing the `Caddyfile`. Swapping to a real domain later is a one-line change (replace the sslip.io hostname) plus pointing an A record at the instance's IP.

Plain `http://` requests get redirected to `https://` automatically — no separate config for that either.

## Known Build Issues

### sccache hangs the WASM build
sccache (configured in `.cargo/config.toml`) is incompatible with the `wasm32-unknown-unknown` target. Always set `RUSTC_WRAPPER=""` when building web targets. The `run-desktop-linux.sh` script handles this automatically.

### wasm-bindgen version mismatch
The `wasm-bindgen-cli` version must exactly match the `wasm-bindgen` crate version in `Cargo.lock`. Check with:
```bash
grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version
```
Then install the matching CLI version:
```bash
cargo install wasm-bindgen-cli --version <version>
```

### reference-types feature required
wasm-bindgen 0.2.103 requires the `reference-types` and `multivalue` WebAssembly proposals to be enabled at compile time. These are set in `.cargo/config.toml`:
```toml
[target.wasm32-unknown-unknown]
rustflags = ["-C", "target-feature=+reference-types,+multivalue"]
```
