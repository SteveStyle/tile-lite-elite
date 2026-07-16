# Tile Lite Elite - Development and Operating Guide

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

## Development Environment Setup

The dev machine (currently WSL2 Ubuntu, treated as disposable/recreatable — see below) needs: a Rust toolchain, the `wasm32-unknown-unknown` target, `dioxus-cli` and `wasm-bindgen-cli` at versions that match this project exactly (a mismatch fails confusingly rather than cleanly — see "Known Build Issues" below), `sccache`, and Docker Engine (for [Container Deployment](#container-deployment)).

```bash
git clone https://github.com/SteveStyle/tile-lite-elite.git
cd tile-lite-elite
./scripts/setup-dev-environment.sh
```

The script is idempotent (safe to re-run; every step checks whether it's already done first) and reads the required `wasm-bindgen-cli` version out of `Cargo.lock` itself rather than hardcoding it, so it won't silently go stale as dependencies update.

**Two things it deliberately doesn't do**, both manual:

1. **Restore the Oracle deploy SSH key.** It's a secret, not something a setup script should generate or fetch on its own. Copy `~/.ssh/oracle_tile_lite_elite` (private) and `.pub` back in from wherever you backed them up (this project's key is also kept on the Windows side, outside WSL, for exactly this recreate-the-VM-is-fine-the-key-survives scenario).
2. **Enable systemd in WSL**, if this is a fresh WSL instance and it's not already on. Docker needs it to manage its service. This is a Windows-side edit, not something a script running inside the distro can safely do to itself:
   ```
   # /etc/wsl.conf
   [boot]
   systemd=true
   ```
   then from Windows PowerShell: `wsl --shutdown`, and restart the shell. The setup script checks for this and warns if it's missing rather than failing silently later.

**Verify the result**:

```bash
cargo test --workspace
dx --version                 # should include 0.6.3
docker compose version
ssh -i ~/.ssh/oracle_tile_lite_elite ubuntu@129.151.69.246 echo ok
```

## Architecture

The project is split into a server and two client types:

```
Backend (port 3000)   ←→   Web UI (port 8080, WASM in browser)
                      ←→   Desktop UI (native window)
```

The backend owns all game state, rule enforcement, scoring, and persistence.
Clients are thin presentation layers that call the server API.

## Environments

Two genuinely different places this project runs, easy to conflate since some of the same commands (`docker compose ...`) work in both:

### Local dev machine

Where you write code, run tests, and build the images that get deployed. Has the full source tree; nothing here is what's actually serving live traffic.

**Components**: Rust toolchain + workspace crates, Docker Engine (used here only to *build* images and optionally run the stack for local testing — see [Container Deployment](#container-deployment) — not to serve real traffic), the git clone itself (pushed to/pulled from GitHub).

**Directory structure** (repo root, `~/tile-lite-elite` in this WSL setup):

```
tile-lite-elite/
├── crates/                  # the six workspace crates
│   ├── api/                 # shared request/response DTOs
│   ├── rules-shared/        # pure rules/scoring/move-generation
│   ├── engine-core/         # GameEngine trait + GreedyEngine
│   ├── server-game/         # Axum backend
│   ├── ui/                  # Dioxus web/desktop client
│   └── admin-cli/           # tile-lite-elite-admin operator CLI
├── old-crates/              # early prototypes, kept for design precedent only
├── docs/                    # this file and other design docs
├── scripts/                 # admin.sh, deploy.sh, services.sh, setup-dev-environment.sh, desktop.sh
├── data/                    # local dev SQLite file (TILE_LITE_ELITE_DATABASE_URL's default)
├── target/                  # cargo build output (gitignored)
├── .cargo/config.toml       # sccache + wasm rustflags (see Known Build Issues)
├── Dockerfile, docker-compose.yml, Caddyfile, .dockerignore
└── Cargo.toml                # workspace manifest
```

### Oracle Cloud VM (production)

Where the live deployment actually runs. **Does not have the source tree at all** — no git clone, no `crates/`, nothing to build. Just the compose file and whatever Docker itself stores (images, volumes) — `scripts/deploy.sh` builds everything locally and ships only the finished images plus `docker-compose.yml` over.

**Components**: Docker Engine only. Two running containers (`tile-lite-elite-server-1`, `tile-lite-elite-web-1`) and three named volumes (`tile-lite-elite-data`, `tile-lite-elite-caddy-data`, `tile-lite-elite-caddy-config`) — see [Container Deployment](#container-deployment) for what each holds.

**Directory structure** (`~/tile-lite-elite` on the VM — same path as local, different machine, don't let that imply it's the same *kind* of directory):

```
~/tile-lite-elite/
└── docker-compose.yml       # the only file that lives here
```

That's genuinely everything on disk outside of Docker's own storage. What's *inside* the containers (via `docker compose exec <service> ls ...`):

```
server container:
├── /usr/local/bin/
│   ├── server-game          # the release binary actually running
│   └── tile-lite-elite-admin       # copied in but not running — invoked on demand via `docker compose exec`
└── /data/                   # the tile-lite-elite-data volume
    └── tile-lite-elite.sqlite3

web container:
├── /srv/                    # the built web client (index.html, assets/, wasm/) — served as static files
├── /etc/caddy/Caddyfile     # baked into the image at build time, not a volume
├── /data/caddy/             # the caddy-data volume — obtained TLS certificates live here
└── /config/caddy/           # the caddy-config volume — Caddy's own runtime state
```

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
- Database: `./data/tile-lite-elite.sqlite3` (auto-created with migrations on first run)

Override with environment variables:
```bash
TILE_LITE_ELITE_BIND=0.0.0.0:3000 TILE_LITE_ELITE_DATABASE_URL=sqlite://data/tile-lite-elite.sqlite3 cargo run -p server-game
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
TILE_LITE_ELITE_API_BASE_URL=http://127.0.0.1:3000 cargo run -p tile-lite-elite-ui --features desktop
```

## Admin CLI

The admin CLI — crate `crates/admin-cli`, binary/command name `tile-lite-elite-admin` — is operator tooling for a running server: list/delete users, reset a password, list/delete/force-end games. It's a thin HTTP client against `server-game`'s `/admin/*` endpoints, not a separate implementation, so it can't drift from what the server actually does (cascading deletes, password hashing, etc. all stay server-side).

**There's no admin account or token.** The `/admin/*` endpoints only accept requests whose peer address is loopback (`127.0.0.1`/`::1`), regardless of what `TILE_LITE_ELITE_BIND` is set to — running the CLI *from the server's own terminal* is the access control. This matters specifically because `TILE_LITE_ELITE_BIND=0.0.0.0:3000` (see the LAN-play example above) would otherwise expose these endpoints to the whole LAN, not just the machine running the server. It also means where you run `tile-lite-elite-admin` *from* isn't a preference, it's the only thing that determines whether it works at all — the two cases below are genuinely different, not interchangeable:

**Local dev server** (running directly on this machine, not in a container):

```bash
./scripts/admin.sh users list
./scripts/admin.sh users reset-password <player_id>          # prints a generated password
./scripts/admin.sh users reset-password <player_id> --password 'a specific one'
./scripts/admin.sh users delete <player_id>

./scripts/admin.sh games list
./scripts/admin.sh games list --status waiting
./scripts/admin.sh games list --older-than-days 30
./scripts/admin.sh games delete <game_id>
./scripts/admin.sh games force-end <game_id>
```

`scripts/admin.sh` builds `admin-cli` in **release** mode and runs that binary — a plain `cargo run -p admin-cli` builds and runs a *debug* binary instead, which still works but is worth avoiding out of habit now that a script exists to do the right thing by default.

**The Oracle VM's (or any container deployment's) server**: `scripts/admin.sh` can't reach it — it always targets `127.0.0.1`, and that's a different loopback than the VM's, by design (see above; pointing `--server`/`TILE_LITE_ELITE_API_BASE_URL` at the VM's public address from your own machine just gets a 403, it isn't a workaround). Run it *inside* the server container instead, where `127.0.0.1` genuinely is that container.  An alias 'sa' has been created which allows it to be called from any directory.

```bash
ssh -i ~/.ssh/oracle_tile_lite_elite ubuntu@129.151.69.246
sa games list
cd ~/tile-lite-elite
docker compose exec server tile-lite-elite-admin games list
```

That binary is the release build baked into the `runtime-server` image by the `Dockerfile` — there's nothing extra to build or configure on the VM itself.

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
cargo build -p tile-lite-elite-ui --features desktop
```

## Running Tests

```bash
cargo test --workspace
```

Runs every crate's tests, including the `old-crates/*` prototypes (harmless — see `history.md`). To run just one crate:
```bash
cargo test -p rules-shared    # rules/scoring/validation unit tests
cargo test -p server-game     # HTTP-level integration tests against the real Axum router
cargo test -p tile-lite-elite-ui     # move-composer logic, game-creation seat presets
cargo test -p engine-core     # engine tests
```

No test coverage for `admin-cli` (it's a thin HTTP client with no logic of its own to test in isolation) or for the WASM target specifically — `cargo test` always runs against the host target, not `wasm32-unknown-unknown`; see "Manual: Web Client" above for how to sanity-check a WASM build compiles.

## Configuration

| Environment variable | Default | Description |
|---|---|---|
| `TILE_LITE_ELITE_BIND` | `127.0.0.1:3000` | Server listen address |
| `TILE_LITE_ELITE_DATABASE_URL` | `sqlite://data/tile-lite-elite.sqlite3` | SQLite database path |
| `TILE_LITE_ELITE_API_BASE_URL` | `http://127.0.0.1:3000` | Backend URL used by clients. Set at *build* time (`option_env!`), not runtime. An explicit empty string means "same origin as the page" — see [Container Deployment](#container-deployment) |
| `TILE_LITE_ELITE_UI_PORT` | `8080` | Web dev server port |
| `RUST_LOG` | `server_game=info,tower_http=info,warn` | Log verbosity for `server-game`. See [Logging](#logging) |
| `TILE_LITE_ELITE_BUILD_ID` | unset | Optional build identifier baked in at *build* time (`option_env!`), appended as SemVer build metadata to the app version (e.g. `0.1.0+a1c9f02`). Unset (the default, used for production releases) shows only `Major.Minor.Patch`. See [Versioning](#versioning) |

## Versioning

Two independent version numbers, on purpose — see the doc comments on
`api::API_VERSION` and each binary's `app_version()` (in `server-game`'s
and `tile-lite-elite-ui`'s `main.rs`) for the full rationale.

**API contract version** (`Major.Minor`, e.g. `1.0`) — lives once in the
shared `api` crate as `api::API_VERSION`, so both the server and any given
client binary embed whatever it was at *their own* build time. Bump
`major` for a breaking DTO/route change, `minor` for an additive,
non-breaking one. The desktop client checks this against the server's
`/health` response the moment it first connects
(`check_api_version`/`compare_api_version` in `crates/ui/src/app.rs`): a
major mismatch blocks further use with an "update the app" message, a
minor mismatch shows a soft, non-blocking notice. There's deliberately no
patch/build component here — those never change the wire contract, so
including them would make the check fire on every routine bugfix deploy.

**App/build version** (`Major.Minor.Patch[+build]`) — `Major.Minor.Patch`
comes straight from each crate's `Cargo.toml` `version` (currently `0.1.0`
everywhere). An optional `+<build>` suffix (standard SemVer build
metadata) is appended when `TILE_LITE_ELITE_BUILD_ID` is set at compile time —
e.g. a git short SHA or CI run number, for telling internal/test builds
apart:

```bash
# Internal/test build with a build id
TILE_LITE_ELITE_BUILD_ID=$(git rev-parse --short HEAD) cargo build -p server-game --release

# Production release — no build id set, shows "0.1.0" not "0.1.0+..."
cargo build -p server-game --release
```

`server-game` logs its app version alongside the API version at startup;
the desktop client puts its app version in the window title.

## Logging

`server-game` uses `tracing`, not `eprintln!`. Application-level events (registration, login success/failure, game created/started/finished, invitations, admin actions, move-time-limit retirement) log at `info` by default; per-HTTP-request spans (method, path, status, latency) from `tower-http`'s `TraceLayer` log at `debug` and are off by default to keep normal output readable.

```bash
# Default verbosity — app events, no per-request noise
cargo run -p server-game

# See per-request HTTP tracing too
RUST_LOG=server_game=info,tower_http=debug cargo run -p server-game

# Everything, very verbose
RUST_LOG=debug cargo run -p server-game
```

Failed logins log the attempted display name (never the password) at `warn`, along with the reason (unknown name vs. wrong password) — visible only to whoever can read the server's own logs, so it doesn't weaken the login endpoint's existing anti-enumeration behavior (the client always gets the same generic error either way). Admin actions (`admin_delete_user`, `admin_reset_password`, `admin_delete_game`, `admin_force_end_game`) log at `warn` specifically so they stand out as an audit trail even at default verbosity.

In the container deployment, this all goes to `docker compose logs server` (or `-f` to follow); `RUST_LOG` can be set as an extra `environment:` entry in `docker-compose.yml`'s `server` service if you need more/less than the default.

## Resetting the Database

Occasionally useful during development — for example, after a schema change that only takes effect on a fresh database (see the migration limitation note in `schema.md`), or just to clear out test data.

```bash
./scripts/services.sh stop
rm -f data/tile-lite-elite.sqlite3 data/tile-lite-elite.sqlite3-wal data/tile-lite-elite.sqlite3-shm
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

SQLite lives on a named volume (`tile-lite-elite-data`, mounted at `/data` in `server`) — it survives `docker compose down` and rebuilds, but not `docker compose down -v`. Back it up with:

```bash
docker run --rm -v tile-lite-elite-data:/data -v "$PWD":/backup debian \
  tar czf /backup/tile-lite-elite-data.tgz -C /data .
```

Caddy's obtained TLS certificate lives on its own named volumes (`caddy-data`, `caddy-config`) for the same reason — losing them means a fresh certificate request on next start, not a functional problem, just unnecessary churn against Let's Encrypt's rate limits.

**Admin CLI**: `/admin/*` stays loopback-only exactly as it is locally — a request proxied in from the `web` container isn't a loopback connection, so the server rejects it the same as it would over a LAN. See the [Admin CLI](#admin-cli) section above for how to reach it here (`docker compose exec server tile-lite-elite-admin ...`) versus against a local dev server — they're not interchangeable.

**Why one image serves both, same-origin**: the web build is compiled with `TILE_LITE_ELITE_API_BASE_URL=""` (explicitly empty, not unset — see the Configuration table above), which makes the client derive its API/WebSocket target from whatever origin actually served the page (`crates/ui/src/app.rs`'s `websocket_url`/`same_origin_websocket_url`). That's what lets the same compiled wasm bundle work regardless of the host's IP or domain, with no rebuild needed if either changes — and it sidesteps CORS entirely, since Caddy serves both the static assets and the proxied API from one origin.

### Redeploying (after a code change)

The live VM has 1GB RAM — not enough to compile the Rust/wasm workspace — so images are always built locally and shipped over, never built on the VM itself. `scripts/deploy.sh` automates the whole cycle:

```bash
./scripts/deploy.sh
```

This builds both images locally, `docker save`s and gzips them, `scp`s them plus `docker-compose.yml` to the VM, `docker load`s them there, and runs `docker compose up -d`. Takes a few minutes, almost all of it the local build. Configurable via env vars (`DEPLOY_HOST`, `DEPLOY_USER`, `DEPLOY_SSH_KEY`, `DEPLOY_REMOTE_DIR`) if the target ever changes — see the script header.

There's no CI and no registry involved — this is a manual, on-demand push from a developer machine, appropriate for a hobby project's actual deploy frequency. Worth revisiting (e.g. push to a registry, `docker compose pull` on the VM instead of scp/load) if that ever changes.

If the change involves schema changes then the database should be backed up and then removed.  It will be recreated when the updated server starts to run.

```bash
ssh tile-lite-elite
cd ~/tile-lite-elite

# 1. Stop services — leaves the named volumes (tile-lite-elite-data, tile-lite-elite-caddy-data,
#    caddy-config) untouched, just stops the containers so nothing's
#    writing to the DB while you back it up.
docker compose down

# 2. Full backup of the data volume (this is docs/operations.md's own
#    documented backup command) — portable, pull it off the VM if you want
#    a copy elsewhere.
docker run --rm -v tile-lite-elite-data:/data -v "$PWD":/backup debian \
  tar czf /backup/tile-lite-elite-data-$(date +%Y%m%d).tgz -C /data .

# 3. Clear the old DB from the volume so the new server starts fresh
#    (create_if_missing(true) recreates it with the new schema on next
#    start). Renaming aside instead of deleting, if you'd rather keep a
#    copy in place as well as the tarball:
docker run --rm -v tile-lite-elite-data:/data debian \
  sh -c 'rm -f /data/tile-lite-elite.sqlite3 /data/tile-lite-elite.sqlite3-wal /data/tile-lite-elite.sqlite3-shm'
```

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
6. **`scp` the SSH key's *public* half only** into the instance creation form; keep the private key local (`~/.ssh/oracle_tile_lite_elite` in this setup) — it's what `scripts/deploy.sh` and manual `ssh`/`scp` commands use.

### HTTPS

Caddy provisions and renews TLS certificates automatically via Let's Encrypt — the entire config is giving it a real hostname instead of a bare `:80` in the `Caddyfile` (Let's Encrypt won't certificate a bare IP address). This deployment uses [sslip.io](https://sslip.io) rather than a purchased domain: `<ip>.sslip.io` (e.g. `129.151.69.246.sslip.io`) resolves straight back to that IP with no registration or DNS configuration needed, and Let's Encrypt validates and issues a real, browser-trusted certificate against it immediately. The URL is uglier than a real domain, but it's free and required zero setup beyond editing the `Caddyfile`. Swapping to a real domain later is a one-line change (replace the sslip.io hostname) plus pointing an A record at the instance's IP.

Plain `http://` requests get redirected to `https://` automatically — no separate config for that either.

## Known Build Issues

### sccache hangs the WASM build
sccache (configured in `.cargo/config.toml`) is incompatible with the `wasm32-unknown-unknown` target. Always set `RUSTC_WRAPPER=""` when building web targets. `scripts/services.sh` handles this automatically.

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
