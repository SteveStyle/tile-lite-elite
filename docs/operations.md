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
| `SCRABBLE_PX_API_BASE_URL` | `http://127.0.0.1:3000` | Backend URL used by clients |
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
