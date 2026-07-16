# tile-lite-elite-ui

Dioxus-based client for the Tile Lite Elite server. Compiles to both a web WASM app and a native desktop app from the same source.

See [docs/operations.md](../../docs/operations.md) for full setup instructions.

## Quick Start

The UI is a thin client — **use the scripts in the root `scripts/` folder** to manage backend and clients.

### Easiest Way (Scripts)

From the workspace root:

```bash
# Start backend server + web client (background)
./scripts/services.sh start

# Launch a desktop client (can run multiple times)
./scripts/desktop.sh

# Check status
./scripts/services.sh status

# Stop all
./scripts/services.sh stop
```

### Manual Startup

If you prefer manual control:

#### 1. Start the backend (from workspace root)

```bash
cargo run -p server-game
# Listening on http://127.0.0.1:3000
```

### 2a. Web client (manual)

```bash
RUSTC_WRAPPER="" CARGO_INCREMENTAL=0 ~/.cargo/bin/dx serve --platform web --port 8080
# Open http://127.0.0.1:8080
```

### 2b. Desktop client (manual)

```bash
cargo run -p tile-lite-elite-ui --features desktop -- --server-url http://127.0.0.1:3000
```

See `src/config.rs` for the compiled-in default environments and the
`--server-url`/`--env` CLI overrides (also usable via `scripts/desktop.sh`).

## How to Play

1. Click **New Human vs Engine** to create a game, then **Start**.
2. **Drag** a tile from the rack onto an empty board square to stage it.
3. **Right-click** a staged tile on the board to remove it.
4. The preview banner shows legality and score as you stage tiles.
5. Click **Submit Staged Move** to play, or **Play Suggested Move** to let the engine choose.
6. Words in the move history are linked to the Collins Dictionary.

## Project Structure

```
crates/ui/
├── assets/styling/main.css   # All CSS
├── src/
│   ├── app.rs                # Root component, all state signals, event handlers
│   ├── main.rs               # Dioxus entry point
│   ├── config.rs             # Desktop-only: compiled-in server environments + CLI override
│   ├── components/
│   │   ├── board_view.rs     # 15×15 board with drag-and-drop
│   │   ├── rack_view.rs      # Draggable tile rack
│   │   └── sidebar.rs        # Scores, move history, Collins links
│   └── views/
│       └── home.rs           # Main game layout
├── Cargo.toml
└── Dioxus.toml
```

Launch scripts live in the repo root's `scripts/` folder, not here — see
`scripts/desktop.sh` and `scripts/services.sh`.

## Dependencies

| Crate | Purpose |
|---|---|
| `dioxus` | UI framework (web + desktop from shared code) |
| `api` | Shared DTOs with the server |
| `gloo-net` | HTTP + WebSocket for WASM target |
| `reqwest` | HTTP client for native desktop target |
| `tokio-tungstenite` | WebSocket for native desktop target |

## Build Notes

- Web builds require `RUSTC_WRAPPER=""` to bypass sccache (sccache and WASM targets conflict).
- `+reference-types,+multivalue` WASM target features are set in `.cargo/config.toml` — required by wasm-bindgen 0.2.103.
- `wasm-bindgen-cli` must match the version in `Cargo.lock`: `cargo install wasm-bindgen-cli --version 0.2.103`.


