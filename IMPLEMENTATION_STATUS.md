# Implementation Status vs Architecture Plan

## Summary

The project has successfully implemented the core MVP architecture: a server-authoritative Scrabble game system with shared rules library, engine plugin interface, and multiple client types (web, desktop). All fundamental boundaries from the architecture doc are in place and working.

## Architecture Alignment

### ✅ Suggested Boundaries - Implemented

#### `crates/api/`
- **Purpose**: Request/response types shared by clients and server
- **Status**: ✅ Implemented
- **Contents**: Enums for SeatKind, GameStatus, DirectionDto, PremiumDto, EngineProfileDto, GameStateDto, etc.
- **Notes**: Correctly uses `#[serde(rename_all = "snake_case")]` for JSON serialization (supports lowercase variants expected by clients)

#### `crates/rules-shared/`
- **Purpose**: Pure Scrabble rules, move generation, scoring, legality, previews
- **Status**: ✅ Implemented
- **Contents**: 
  - `board.rs` - BoardCell, BoardState, EmptyCell, FilledCell
  - `dictionary.rs` - Dictionary, SOWPODS word list, is_word()
  - `cache.rs` - RuleCache, CrossCheck, AnchorFlags (performance optimization)
  - `generate.rs` - MoveGenerator (produces candidate moves)
  - `validate.rs` - GameState, RulesEngine, move validation
  - `score.rs` - Scoring logic
  - `model.rs` - Core types: Tile, Position, Direction, Rack, MoveCandidate
- **Notes**: Properly separated from server concerns; clients can import for local preview

#### `crates/engine-core/`
- **Purpose**: Engine search, heuristics, move selection, engine-only metadata
- **Status**: ✅ Implemented
- **Contents**:
  - EngineMetadata struct with versioning (id, name, version, author, description, supported_variants)
  - EngineCapabilities (supports_timed_play, supports_analysis, supports_ranking)
  - EngineRequest contract (state, seat_number, rack, time_budget_ms)
  - ScrabbleEngine trait (abstracts move generation)
  - GreedyEngine impl (simple greedy move selection)
- **Notes**: Properly versioned from the start; GreedyEngine is reference implementation

#### `crates/server-game/`
- **Purpose**: Game lifecycle, turn sequencing, persistence, engine proxy
- **Status**: ✅ Implemented
- **Contents**:
  - `app.rs` - Axum router, HTTP handlers (create_game, list_games, get_game, start_game, submit_action, preview_move, suggest_move, game_events, register_player, login_player, validate_session, invite/accept/reject invitation), the auth helpers (argon2 password hashing, sha256 token hashing, bearer-token resolution), and the `/admin/*` handlers (list/delete users, reset password, list/delete/force-end games) behind a loopback-only guard (`require_loopback`, using `ConnectInfo<SocketAddr>` — the server is started with `into_make_service_with_connect_info` specifically so this works)
  - `game_state.rs` - GameSession, ParticipantState, EngineRegistry; `maybe_run_engine_turn` is async, running the engine via `spawn_blocking` with a timeout; also owns the standard end-of-game rules (rack-out scoring, pass-out) and the shared `format_move_error` wording used by both `/preview` and real submissions
  - `persistence.rs` - SQLite migrations, save/load game, session/player lookups, and the admin functions (`list_players`, `delete_player`, `update_player_password`, `delete_game`) — deleting a user unclaims their seats (`player_id` set to null) rather than deleting their games, so history and other players' records survive
- **Notes**: Server is properly authoritative; all rule validation happens server-side. Engine-originated moves flow through the exact same `apply_*` methods a human's HTTP action does — no special-cased trust path for engines. New games get a genuinely random shuffle seed by default (`rand::random()`) — this used to fall back to a fixed constant, so every game dealt identical racks in identical order.

#### `crates/ui/`
- **Purpose**: Web and desktop presentation layers (Dioxus framework)
- **Status**: ✅ Implemented
- **Contents**:
  - `app.rs` - RootApp component, three-column layout composition (Games / board+rack / Seats+Recent Moves), game state management, event handlers, auth session state, connection-status tracking (`static IS_ONLINE: GlobalSignal<bool>`) and the background reconnect/reload loop
  - `main.rs` - Application entry point (desktop window: 1400×1150 default, 800×600 minimum)
  - `local_storage.rs` - Cross-platform persistence for "remember me" / "stay logged in" (browser localStorage on web, a plain JSON config file on desktop — not encrypted either way)
  - `time_format.rs` - Relative-time formatting for the games list ("3m ago")
  - `components/` - `board_view` (click-to-select + drag-and-drop), `rack_view` (click-to-place + exchange-mode tile selection), `sidebar` (Seats + Recent Moves), `games_panel` (game management, selectable list, and the "vs Engine"/"vs Human"/"Engine vs Engine" new-game presets), `auth_panel` (Login/Register widget)
  - `views/home.rs` - Center-column game view (board, rack, move composer, turn actions: Start/Pass/Play/Exchange); trimmed of the original hero/marketing copy and technical details (server URL, "Client Role" explanation) during a UI cleanup pass
- **Notes**: Dual-target (web WASM + desktop native), uses same codebase via feature flags. Layout is a CSS Grid with each column independently scrollable within a viewport-relative height, since the 15×15 board plus rack can exceed a typical window's height. Move composition supports both drag-and-drop and click/keyboard placement (select a cell, then click a rack tile or type its letter — auto-advances in the direction the staged word is reading, skipping over tiles already on the board; a typed letter with no matching tile falls back to an unused blank).

#### `crates/admin-cli/`
- **Purpose**: Operator tooling for a running server — list/delete users, reset passwords, list/delete/force-end games
- **Status**: ✅ Implemented
- **Contents**: `main.rs` — a `clap`-based CLI (`scrabble-admin`) that's a thin HTTP client against `server-game`'s `/admin/*` endpoints; no business logic of its own (cascading deletes and password hashing stay server-side, so the CLI can't drift from what the server actually does)
- **Notes**: Not authenticated by account/token — the server's admin routes only accept loopback connections (see `require_loopback` in `server-game/app.rs`), regardless of what `SCRABBLE_PX_BIND` is set to. Running the CLI *is* the access control: it only works from the server's own terminal.

---

## MVP Checklist

### ✅ Core Server Architecture

- [x] Server-owned game state and rule enforcement
  - GameSession holds canonical state
  - RulesEngine validates all moves server-side
  - AppState manages game persistence

- [x] Human-vs-human, human-vs-engine, engine-vs-engine games
  - SeatKind::Human and SeatKind::Engine in API
  - ParticipantState tracks both human and engine seats
  - EngineRegistry plugs engines into seats

- [x] One stable engine interface
  - ScrabbleEngine trait with metadata and versioning
  - EngineMetadata includes id, version, author, description
  - EngineRequest contract is stable
  - GreedyEngine reference implementation

- [x] Shared pure rules library
  - rules-shared compiles for native and WASM
  - Clients can import for legality/score preview (not tested yet)
  - Server uses same rules for canonical validation

- [x] Game creation, start, resign, pass, exchange
  - POST /games - create with seats
  - POST /games/{id}/start - begin match
  - POST /games/{id}/actions - submit moves/passes/exchanges
  - PlayerActionDto enum: PlaceTiles, Pass, Exchange, Resign

- [x] Standard end-of-game rules (beyond resignation)
  - Going out: a player emptying their rack with the bag already empty ends the game immediately — everyone's score is reduced by the value of the tiles left on their rack, and the player who went out additionally receives the sum of everyone else's leftover rack value
  - Pass-out: 6 consecutive scoreless plays (passes or exchanges), summed across all seats, also ends the game (no bonus, same per-rack deduction) — the standard tournament rule, equivalent to each player passing 3 times in a row heads-up; the counter resets on any scoring placement
  - `GameSession::finish_game` computes the winner as the highest final score, or `None` on an exact tie
  - Previously the only way a game reached `Finished` was manual resignation; engines never resign, so engine-vs-engine games could not end on their own before this

- [x] Server-side scoring and legality
  - MoveValidator enforces rules
  - Score calculation via score.rs
  - Illegal moves rejected with error response

- [x] Game move history
  - MoveRecord struct persists (move_number, seat_number, move_type, main_word, score_delta, description)
  - Stored in game_moves table
  - Available via GameStateDto

- [x] Deterministic tests for move legality and scoring
  - rules-shared: 22 unit tests (anchors, cross-checks, dictionary lookup, move generation including blanks, bingo bonus, gap rejection, and multi-word invalid-move reporting — a placement that breaks both the main word and a cross word at once now names both, e.g. "Z and CZT are not in the dictionary" instead of only ever reporting whichever was checked first)
  - server-game: 23 integration tests against the real Axum router (create/list/get, human move + engine auto-reply, persistence reload, all four player actions, wrong-turn/not-found/illegal-move rejection, seat-ownership enforcement, display-name uniqueness, going out with an empty bag finishes the game with the correct rack-penalty scoring, a two-engine game runs to completion off a single `/start` call, seedless games get different racks, and the full `/admin/*` surface including the loopback-only rejection)
  - scrabble-ui: 13 unit tests (game-creation seat presets, the click/keyboard composer's direction-inference and cell-stepping logic, blank-tile resolution)
  - engine-core: 1 test (greedy engine opening move)
  - 59 tests total, all passing as of the last full run

- [x] Move-composer UX (beyond drag-and-drop)
  - Click a board cell to select it, then click a rack tile or type its letter to place it — auto-advances to the next open cell in the direction the staged word is reading (inferred from adjacency/alignment, defaulting horizontal when ambiguous), skipping over tiles already on the board
  - Backspace clears the tile at the selected cell and steps back
  - Typing a letter with no matching rack tile but an unused blank auto-resolves the blank to that letter, skipping the manual blank-letter picker
  - Illegal-move text is short and specific ("QX is not in the dictionary." / "Incorrect tile placement."), shared between the live `/preview` endpoint and the real submit path so they always agree, and rendered in a fixed-height slot so it never shifts the rack
  - A placement invalid in more than one way at once (main word *and* a cross word) now names every invalid word, not just whichever one validation happened to check first

- [x] Client resilience to server/network outages
  - `IS_ONLINE` (a global signal) is set the moment an HTTP call fails at the network level — before any response comes back — distinguishing "server unreachable" from a legitimate rejection (illegal move, wrong turn), which previously looked identical (raw reqwest/gloo error text either way)
  - A background loop pings `/health` every 3s while offline and reloads the games list + current game the moment it succeeds, since the WebSocket doesn't replay missed events to a reconnecting socket
  - The WebSocket subscription retries forever (3s) instead of dying permanently after the first drop — this was a real bug: `websocket_game_id` stayed set after a failed subscribe, so the "should I resubscribe?" check never re-fired
  - Action buttons disable while offline; a topbar indicator shows "Can't reach the server — reconnecting..." and clears on recovery
  - Verified live: killed the server under an active desktop client, confirmed no crash/busy-loop, restarted it, confirmed via `ss` that the same client process re-established a live connection

- [x] Admin tooling
  - `scrabble-admin` CLI (`crates/admin-cli`): list/delete users, reset a password, list/delete/force-end games
  - Backed by `/admin/*` endpoints on `server-game` rather than direct DB access, so cascading deletes and password hashing can't drift between two implementations
  - No account/token — the endpoints reject anything that isn't a loopback connection, regardless of `SCRABBLE_PX_BIND` (which `operations.md` documents setting to `0.0.0.0` for LAN play — the guard exists specifically so that doesn't also expose admin routes to the LAN)
  - Deleting a user unclaims their seats rather than deleting their games, preserving history and other players' records

### ✅ Clients

- [x] Web client
  - Dioxus web WASM target
  - Compiles to wasm32-unknown-unknown
  - Serves on localhost:8080 via dx serve

- [x] Desktop client
  - Dioxus desktop target
  - Native GTK application
  - Runs with ./run-desktop-linux.sh

- [x] Same codebase for both
  - Dual-target via Dioxus features (web/desktop)
  - Shared app.rs, components, HTTP client logic
  - Platform-specific condcompilation for reqwest vs fetch

### ✅ Persistence

- [x] SQLite database per environment
  - Default: ./data/scrabble-px.sqlite3 (65 KB on disk)
  - SCRABBLE_PX_DATABASE_URL env var configurable

- [x] Migrations create schema
  - tables: schema_migrations, players, engine_profiles, games, game_participants, game_moves
  - Auto-created on server startup via persistence::migrate()

- [x] Durable game and platform data
  - games table: id, status, variant, language, board_layout, turn_number, current_seat, winner_seat, random_seed, snapshot_json
  - game_participants: seat_number, kind, display_name, player_id, engine_id, score, joined_at, left_at
  - game_moves: move_number, seat_number, move_type, main_word, score_delta, description
  - players: display_name (unique), email, password_hash (argon2)

### ✅ Server Infrastructure

- [x] Axum HTTP server
  - Listening on 127.0.0.1:3000 (configurable via SCRABBLE_PX_BIND)
  - Routes: /health, /engines, /games, /games/{game_id}, /games/{game_id}/start, /games/{game_id}/actions, /games/{game_id}/events
  - CORS enabled (CorsLayer::permissive())

- [x] REST API shape correct
  - POST /games with CreateGameRequest (binds the creator's first human seat if authenticated; anonymous creation still works, fully open)
  - GET /games returns `Vec<GameSummaryDto>` (id, status, participants, current seat, last-activity time) — not bare IDs
  - GET /games/{id} returns GameStateDto
  - POST /games/{id}/actions with GameActionRequest — rejects a claimed seat if the caller isn't its owner
  - All responses are JSON, serde-compatible

- [x] Reconnect support
  - GET /games/{id} can reload full game state
  - Client can reconnect and fetch current board/racks/history

---

## Known Gaps vs Architecture Doc

### ⚠️ Not Yet Implemented (But Documented)

| Feature | Status | Notes |
|---------|--------|-------|
| WebSocket events for live updates | Implemented client-side | Server broadcasts, and the client (`crates/ui/src/app.rs`) does subscribe and apply live updates — this row was stale, the earlier "client doesn't subscribe" claim is no longer true |
| Engine vs engine play | Tested | `engine_vs_engine_game_runs_to_completion` creates a two-`greedy-v1` game and asserts a single `/start` call drives it to `Finished`. Required fixing two real gaps first: no natural end-of-game condition existed at all (see "Standard end-of-game rules" above), and `run_engine_turns` was capped at `participants.len()` iterations per trigger — fine for human+engine games (a human seat always breaks the loop) but meant an all-engine game only ever advanced one round per `/start` and then stalled forever with no human to trigger another. Now uncapped (with a generous 400-iteration safety ceiling against a hypothetical buggy engine) |
| Multiple engine implementations | Reference only | Only GreedyEngine exists; arch supports plugging in more |
| Engine benchmarking | Not built | Engine trait supports it; CLI for benchmarking not implemented |
| Player identity (user accounts) | Implemented | Register/login/validate work end-to-end, with a real login UI (web + desktop) and seat-ownership enforcement on `submit_action`. See `authentication.md`'s status section for exactly what's still missing (email verification, forgot-password, ownership checks on other endpoints) |
| Spectator mode | Not implemented | No spectator_id in schema or endpoints |
| Audit log | Not implemented | Schema exists (audit_log table), not used |
| Versioned engine contract | Not tested | Version strings exist in metadata, no migration tested |
| Engine turn timeout | Implemented | Engine runs via `spawn_blocking` raced against a 5s `tokio::time::timeout`; a timed-out engine auto-passes rather than stalling the game. No test yet forces the timeout branch itself |
| Save/load game state | Partial | Save works; load requires manual game ID (no UI) |
| CLI client | Not built | Architecture supports it; Dioxus CLI would compile it |
| Mobile client | Not built | Architecture supports Dioxus mobile target |
| Timed play | Not implemented (for humans) | Engine-side timeout exists (above); no equivalent per-turn clock for human players |
| Admin tooling | Implemented | `scrabble-admin` CLI + `/admin/*` endpoints — see the Admin tooling checklist item above |
| Client resilience to outages | Implemented | Connection-status tracking, background reconnect/reload, self-healing WebSocket — see the Client resilience checklist item above |

### ⚠️ Partially Implemented

**WebSocket Events**:
- Route `/games/{game_id}/events` exists
- broadcast channel created in AppState
- GameEventDto enum defined (GameStarted, MoveMade, GameEnded, PlayerJoined, PlayerLeft)
- Events sent after start_game() and submit_action()
- Client subscribes and applies live updates, and now retries the subscription forever (3s) instead of giving up permanently after the first drop
- **Still missing**: the server doesn't replay missed events to a freshly (re)connected socket — a reconnecting client has to fall back to an explicit HTTP reload to catch up, which the client's background recovery loop now does automatically

**Engine Execution**:
- EngineRegistry holds engines
- GreedyEngine compiles and produces moves
- Server calls `run_engine_turns()` after game start, human moves, and suggest-move
- Engine moves submitted to authoritative game state through the same `apply_*` methods a human action goes through — no special-cased trust path
- Runs via `tokio::task::spawn_blocking` (CPU-bound work off the async runtime) raced against a 5s `tokio::time::timeout`; a timeout auto-passes the seat rather than stalling the game
- **Missing**: sandboxing, diagnostics/explanation output, a test that actually forces the timeout branch

**Authentication**:
- ✅ Schema includes players table (id, display_name **[unique]**, email, password_hash, created_at, updated_at, last_seen_at)
- ✅ Schema includes sessions table (id, player_id, token_hash, created_at, last_seen_at, expires_at)
- ✅ POST /auth/register — creates a new player, rejects a duplicate `display_name` with a clear error, hashes the password with argon2
- ✅ POST /auth/login — verifies against the argon2 hash; returns the same generic error whether the name is unknown or the password is wrong, to avoid leaking which names are registered
- ✅ POST /auth/validate — fully implemented (previously a stub returning "not yet implemented"); resolves a bearer token to a player
- ✅ Bearer-token auth (`Authorization: Bearer <token>`) is threaded through the client for every action-submitting request
- ✅ Seat-ownership enforcement: if a game is created while authenticated, the creator's first human seat is bound to their player id; `submit_action` then rejects any other (or no) authenticated caller for that seat. Anonymous game creation still works exactly as before, fully open — this was a deliberate backward-compatibility choice, not an oversight
- ✅ Login/Register UI exists in `crates/ui` (web + desktop), with "Remember me" (pre-fills display name next time) and "Stay logged in" (persists the session token) checkboxes
- ⚠️ Only `submit_action` checks ownership — `start_game`, `preview_move`, `suggest_move`, and the WebSocket events endpoint don't check identity at all yet
- ⚠️ Email not verified in MVP (captured for future use); no "forgot password" flow exists
- ⚠️ Claiming a *second* human seat (inviting someone else in) isn't wired to the invitation-accept flow — see the Game Invitations notes below, unchanged

**Game Invitations**:
- ✅ Schema includes game_invitations table (id, game_id, invited_player_id, inviting_player_id, seat_number, status, created_at, responded_at)
- ✅ POST /games/{game_id}/invite - Invite player to join game
- ✅ GET /players/{player_id}/invitations - List pending/responded invitations
- ✅ POST /invitations/{invitation_id}/accept - Accept invitation
- ✅ POST /invitations/{invitation_id}/reject - Reject invitation
- ⚠️ Invitation response doesn't automatically place player in seat (manual integration needed)

---

## Code Quality Notes

### ✅ Strong Points

1. **Proper layer separation**
   - rules-shared has no dependencies on server
   - engine-core depends only on rules-shared
   - server-game orchestrates both layers
   - API types are isolated and serializable

2. **Type safety**
   - Rust enum types prevent invalid state transitions
   - SeatKind, GameStatus, DirectionDto all properly typed
   - serde contracts enforce valid JSON shapes

3. **Database design**
   - Foreign keys and uniqueness constraints present
   - Schema supports future features (player_id, engine_id, spectator tracking)
   - Migrations auto-run on server startup

4. **Deployment readiness**
   - Single SQLite file simplifies backup/restore
   - No external service dependencies
   - Can run on any Linux/macOS/Windows system with Rust

### ⚠️ Areas for Attention

1. **Error handling**
   - ApiProblem type is defined but error messages could be more detailed
   - Invalid move rejections could log rule violation reason

2. **Testing**
   - Real coverage now exists: 59 tests across the workspace (see the MVP Checklist above for the breakdown), including HTTP-level integration tests and the security-relevant seat-ownership/uniqueness tests
   - Still missing: engine-vs-engine play, a test that forces the engine timeout branch, anything beyond `submit_action` for auth/ownership, and any test of the WebSocket events path

3. **Observability**
   - No structured logging
   - Engine decisions not logged (would help debugging)
   - Performance metrics not tracked

4. **Client-side validation**
   - Desktop/web clients fetch game state but don't pre-validate moves locally
   - Could use shared rules library to show illegal moves before submission

---

## Verification

Both clients (web via `dx serve`, desktop via `cargo run -p scrabble-ui --features desktop`) connect to the backend and support the full game loop: create/select a game (choosing the seat mix), place tiles (drag-and-drop or click/keyboard), pass, exchange, resign, play against the greedy engine or another engine, and log in/register with a persisted session. `cargo test` at the workspace root runs all 59 tests; see `operations.md` for exact commands to run each piece, including the admin CLI.

The client's outage handling was verified live, not just by test: killed the server for ~15s under an active desktop client, confirmed it stayed up without crashing or busy-looping, restarted the server, and confirmed via `ss` that the same client process re-established a live connection. The admin CLI was verified live against the real dev database: registered a throwaway user, reset its password and confirmed the old one stopped working, listed/filtered real games, force-ended and deleted a test game, deleted the test user.

---

## Roadmap Alignment

### MVP (Current Target)

Status: **Core loop solid; player identity now real, not just schema**

✅ Done:
- Server-owned game state, rule enforcement
- Human vs engine games, shared rules library
- All four player actions (place, pass, exchange, resign)
- Persistence (SQLite), two client types (web + desktop)
- Live updates over WebSocket (server broadcasts, client subscribes and applies)
- Real test coverage (59 tests, see above)
- Engine turn timeout (auto-pass on a slow engine rather than stalling)
- Player accounts: register/login/validate, argon2 password hashing, unique display names, a real login UI with "remember me"/"stay logged in"
- Seat-ownership enforcement on `submit_action` for games created while logged in
- Standard end-of-game rules: going-out rack-penalty scoring and 6-scoreless-turn pass-out (previously only resignation could end a game)
- Engine-vs-engine test coverage (a two-engine game now runs to completion off one `/start` call)
- Game creation lets you choose the seat mix (vs Engine / vs Human / Engine vs Engine) instead of always creating one fixed human-vs-engine game
- New games get a real random shuffle seed by default (previously fixed, so every game dealt identical racks)
- Move composer supports click/keyboard placement alongside drag-and-drop, with an Exchange flow
- Illegal-move messages are specific and consistent between preview and real submission, and name every simultaneously-invalid word
- Client survives server/network outages: distinguishes "unreachable" from "rejected," auto-reconnects the WebSocket, and auto-recovers state once back online
- Admin CLI for operating the server (users, games)

❌ Not Yet:
- Ownership enforcement beyond `submit_action` (start/preview/suggest/WebSocket)
- Manual verification of seat-ownership cross-account behavior through the UI with two real accounts (only covered by the automated test)
- Forgot-password / email verification flows
- Claiming a second human seat via invitations (accept doesn't auto-place the player yet)
- Structured logging / engine decision diagnostics

### v1 (Next Phase)

Expected next focus:
- Multiple engine implementations
- Engine benchmarking CLI
- Finish the invitations → seat-claiming flow
- Full move validation test suite
- Client-side preview validation

---

## Conclusion

The architecture plan is **well-executed**. The codebase correctly separates concerns, uses the right technologies (Rust, Axum, Dioxus, SQLite), and has the foundation to support all documented features. Both clients work end-to-end today, including a real login/register flow with seat-ownership protection. The main remaining gaps are the parts of auth/identity not yet extended past `submit_action`, the invitations-to-seat-claiming handoff, observability (structured logging), and optional features (spectator mode, timed play for humans, engine benchmarking).
