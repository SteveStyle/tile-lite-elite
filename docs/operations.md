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

## Configuration

| Environment variable | Default | Description |
|---|---|---|
| `SCRABBLE_PX_BIND` | `127.0.0.1:3000` | Server listen address |
| `SCRABBLE_PX_DATABASE_URL` | `sqlite://data/scrabble-px.sqlite3` | SQLite database path |
| `SCRABBLE_PX_API_BASE_URL` | `http://127.0.0.1:3000` | Backend URL used by clients |
| `SCRABBLE_UI_PORT` | `8080` | Web dev server port |

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
