# Implementation Status vs Architecture Plan

## Summary

The project has successfully implemented the core MVP architecture: a server-authoritative Scrabble game system with a shared rules library, an engine plugin interface, multiple client types (web, desktop), a full invitation/matchmaking model — including creator-managed roster edits after a game exists (add/remove/withdraw/resend/force-resign a seat) and inviting someone by raw email address with no account required — per-game move-time limits, structured logging, transactional email (Resend), self-service account editing, and a real production deployment (Docker + Caddy + automatic HTTPS, running on Oracle Cloud, at tileliteelite.com). All fundamental boundaries from the architecture doc are in place and working; most of what's left is optional/v1 scope (multiple engines, engine benchmarking) rather than gaps in the core loop.

## Architecture Alignment

### ✅ Suggested Boundaries - Implemented

#### `crates/api/`
- **Purpose**: Request/response types shared by clients and server
- **Status**: ✅ Implemented
- **Contents**: Enums for SeatKind, GameStatus, DirectionDto, PremiumDto, EngineProfileDto, GameStateDto, GameRelationship, SeatClaim (Creator/Named/Open/Email), SeatInvitationStatus, InvitationStatus, InvitationPreviewDto, ChangePasswordRequest, UpdatePlayerDetailsRequest, etc.
- **Notes**: Correctly uses `#[serde(rename_all = "snake_case")]` for JSON serialization (supports lowercase variants expected by clients). `GameSummaryDto` carries a caller-relative `relationship` tag (`YourTurn`/`Participant`/`InvitedByName`/`InvitedOpen`) and `invitation_id` — the server returns one flat, tagged list per caller from `GET /games` rather than pre-split buckets, so the client sorts/groups/filters however it wants.

#### `crates/rules-shared/`
- **Purpose**: Pure Scrabble rules, move generation, scoring, legality, previews
- **Status**: ✅ Implemented
- **Contents**: 
  - `board.rs` - BoardCell, BoardState, EmptyCell, FilledCell
  - `dictionary.rs` - `Dictionary` trait + `SowpodsDictionary`, backed by a sorted-word-list `SortedPrefixCursor` for incremental prefix search (see below)
  - `cache.rs` - RuleCache, CrossCheck, AnchorFlags (performance optimization)
  - `generate.rs` - MoveGenerator (produces candidate moves)
  - `validate.rs` - GameState, RulesEngine, move validation
  - `score.rs` - Scoring logic
  - `model.rs` - Core types: Tile, Position, Direction, Rack, MoveCandidate
- **Notes**: Properly separated from server concerns; clients can import for local preview. Move generation was found to be exponential-time on certain real board positions (an engine could take 13–26s on a single turn, well past the 5s engine timeout, causing spurious auto-passes on winnable positions). Fixed with dictionary-backed prefix pruning: `SortedPrefixCursor` narrows a sorted sub-slice of the word list one letter at a time as move generation explores the board, pruning a branch the instant no word can possibly continue — a 337×/283× speedup on the reproduction case, byte-identical output. `examples/repro_lexicon.rs` reproduces the exact board/racks that exposed the bug and is kept permanently as a benchmark harness for comparing future generator/engine approaches (a trie or GADDAG were discussed as future comparison points, not built).

#### `crates/engine-core/`
- **Purpose**: Engine search, heuristics, move selection, engine-only metadata
- **Status**: ✅ Implemented
- **Contents**:
  - EngineMetadata struct with versioning (id, name, version, author, description, supported_variants)
  - EngineCapabilities (supports_timed_play, supports_analysis, supports_ranking)
  - EngineRequest contract (state, seat_number, rack, time_budget_ms)
  - GameEngine trait (abstracts move generation)
  - GreedyEngine impl (simple greedy move selection)
- **Notes**: Properly versioned from the start; GreedyEngine is reference implementation

#### `crates/server-game/`
- **Purpose**: Game lifecycle, turn sequencing, persistence, engine proxy
- **Status**: ✅ Implemented
- **Contents**:
  - `app.rs` - Axum router and HTTP handlers. Full route list: `GET /health`, `GET /engines`, `GET /dictionaries/{name}`, `POST /auth/{register,login,validate,change-password,update-details,forgot-password,reset-password}`, `GET /players/search`, `POST /games` + `GET /games` (per-caller filtered/tagged), `GET /games/{id}`, `POST /games/{id}/{start,reorder-seats,seats,actions,chat,remove,preview,suggest,invite}`, `POST /games/{id}/seats/{seat_number}/{remove,withdraw,force-resign}`, `GET /games/{id}/events` (WebSocket), `GET /players/{id}/invitations`, `GET /invitations/{id}/preview`, `POST /invitations/{id}/{accept,reject}`, and `/admin/*` (users, games — loopback-gated). Also the auth helpers (argon2 password hashing, sha256 token hashing, bearer-token resolution) and `expire_overdue_turns`/`expire_overdue_turn` (the lazy move-time-limit check — see below)
  - `game_state.rs` - GameSession, ParticipantState, EngineRegistry; `maybe_run_engine_turn` is async, running the engine via `spawn_blocking` with a timeout; also owns the standard end-of-game rules (rack-out scoring, pass-out), the shared `format_move_error` wording used by both `/preview` and real submissions, `apply_move_timeout` (auto-retires the current seat — same effect as a resign — once it's sat on its turn past `move_time_limit_seconds`), and the post-creation roster methods (`add_seat`, `remove_seat`, `withdraw_seat`, `force_resign`) a creator uses to manage a `Waiting` game's seats after it exists
  - `persistence.rs` - SQLite migrations, save/load game, session/player lookups, the admin functions (`list_players`, `delete_player`, `update_player_password`, `delete_game`), invitation functions (`create_invitation` — now also carries an optional `invited_email` for `SeatClaim::Email`, `claim_invitation` — race-safe atomic accept, `get_open_invitations` — excludes email-targeted invitations, `get_invitation_by_id`, `get_pending_invitation_for_seat`, `shift_invitation_seat_numbers_down` — keeps invitation history in sync when a seat is removed), `update_player_details` (self-service display-name/email change), and `invalidate_sessions_for_player` (used by self-service password change)
- **Notes**: Server is properly authoritative; all rule validation happens server-side. Engine-originated moves flow through the exact same `apply_*` methods a human's HTTP action does — no special-cased trust path for engines. New games get a genuinely random shuffle seed by default (`rand::random()`). Seat ownership is enforced on every action-capable endpoint, and an unclaimed human seat now means an invitation is still outstanding — not "open to anyone" the way it used to (anonymous/unauthenticated game creation and play has been retired entirely in favor of the invitation model, below).

#### `crates/ui/`
- **Purpose**: Web and desktop presentation layers (Dioxus framework)
- **Status**: ✅ Implemented
- **Contents**:
  - `app.rs` - RootApp component, three-column layout composition (Games / board+rack / Seats+Recent Moves), game state management, event handlers, auth session state, connection-status tracking (`static IS_ONLINE: GlobalSignal<bool>`) and the background reconnect/reload loop. `server_url` resolves to the page's own origin when built with an empty `TILE_LITE_ELITE_API_BASE_URL` (used by the container deployment, where a reverse proxy serves both the client and API from one origin) instead of only supporting an explicit configured host.
  - `main.rs` - Application entry point (desktop window: 1400×1300 default, 800×600 minimum — tuned so the rack panel and its turn-action buttons aren't cut off below the visible window on launch)
  - `local_storage.rs` - Cross-platform persistence for "remember me" / "stay logged in" (browser localStorage on web, a plain JSON config file on desktop — not encrypted either way)
  - `time_format.rs` - Relative-time formatting for the games list ("3m ago") and a move-deadline countdown ("2h left" / "overdue")
  - `components/` - `board_view` (click-to-select + drag-and-drop), `rack_view` (click-to-place + exchange-mode tile selection), `sidebar` (Seats + Recent Moves), `games_panel` (grouped by relationship — your turn / your games / invited by name / open invitations — plus a seat-builder game-creation form: creator/named/open/email/engine rows and a per-move time limit, alongside the existing "vs Engine"/"vs Human"/"Engine vs Engine" one-click presets; a `Waiting` game's detail view also gets creator-only roster-management controls — send/resend an invitation, remove a seat, reorder, and a per-seat "Withdraw" for whoever holds a non-creator seat — plus creator-only force-resign once `Active`), `auth_panel` (Login/Register widget, an "Edit user details" dialog for display name/email with a nested password-change sub-section, and — when reached via an emailed join link — an inviter-name banner and an "Accept invitation from X?" confirmation)
  - `views/home.rs` - Center-column game view (board, rack, move composer, turn actions: Start/Pass/Play/Exchange), plus a move-deadline chip when a game is active
- **Notes**: Dual-target (web WASM + desktop native), uses same codebase via feature flags. Move composition supports both drag-and-drop and click/keyboard placement. Backspace had a real bug — it deleted the wrong tile and overshot the cursor by one extra cell after deleting, because it reused the same "skip past a just-typed tile" helper that forward-typing needs, when backspace actually needs to land *on* the previous tile, not skip past it; fixed with a dedicated backward-search helper, and Delete was added as a separate forward-delete that removes a tile without moving the cursor (previously only Backspace existed). Native HTML5 drag-and-drop was reported not working reliably in the desktop build (WSLg/webkit2gtk); investigated but not fixed — Dioxus's `DragData` API doesn't expose `dataTransfer`, which WebKit is known to want for reliable drag sequences, and there's no way to verify a fix without a real display. Click-to-place remains a fully working alternative.

#### `crates/admin-cli/`
- **Purpose**: Operator tooling for a running server — list/delete users, reset passwords, list/delete/force-end games
- **Status**: ✅ Implemented
- **Contents**: `main.rs` — a `clap`-based CLI (`tile-lite-elite-admin`) that's a thin HTTP client against `server-game`'s `/admin/*` endpoints; no business logic of its own (cascading deletes and password hashing stay server-side, so the CLI can't drift from what the server actually does)
- **Notes**: Not authenticated by account/token — the server's admin routes only accept loopback connections (see `require_loopback` in `server-game/app.rs`), regardless of what `TILE_LITE_ELITE_BIND` is set to. Running the CLI *is* the access control: it only works from the server's own terminal — including in the container deployment, where it's reachable via `docker compose exec server tile-lite-elite-admin ...` (a genuinely loopback connection from inside that container).

#### Deployment (new — not part of the original crate boundaries, but now a real part of the architecture)
- **Purpose**: Run the app somewhere other than a developer's own machine
- **Status**: ✅ Implemented and live
- **Contents**: `Dockerfile` (multi-stage: one `builder` stage, two final targets — the server binary + `admin-cli`, and Caddy serving the release web build), `docker-compose.yml`, `Caddyfile`, `scripts/deploy.sh`, `scripts/setup-dev-environment.sh`
- **Notes**: Caddy reverse-proxies API/WebSocket traffic to the server over the compose network and serves the static web build from the same origin (see the `crates/ui` same-origin note above) — same-origin also means no CORS configuration is needed for the deployed app. Caddy also handles automatic HTTPS via Let's Encrypt given a real hostname; the live deployment uses an [sslip.io](https://sslip.io) hostname (`<ip>.sslip.io`) rather than a purchased domain, since it resolves straight back to the host's IP with zero registration. `/admin/*` is deliberately not proxied, preserving its loopback-only guard. Currently running on an Oracle Cloud "Always Free" compute instance (1 OCPU/1GB RAM — not enough to compile the workspace itself, so `scripts/deploy.sh` always builds locally and ships the finished images over via `docker save`/`scp`/`docker load` rather than building on the VM). See `docs/3.4-deployment.md`'s "Container Deployment" section and `docs/3.1-setup.md`'s "Oracle Cloud VM setup" section for the full setup, including the Oracle networking gotchas that cost real time to work through.

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
  - GameEngine trait with metadata and versioning
  - EngineMetadata includes id, version, author, description
  - EngineRequest contract is stable
  - GreedyEngine reference implementation

- [x] Shared pure rules library
  - rules-shared compiles for native and WASM
  - Clients can import for legality/score preview (not tested yet)
  - Server uses same rules for canonical validation
  - Move generation is prefix-pruned against the dictionary (see `crates/rules-shared/` notes above) — a real performance fix, not just an optimization opportunity

- [x] Game creation, start, resign, pass, exchange
  - POST /games - create with seats (creator/named-invite/open-invite/engine per seat)
  - POST /games/{id}/start - begin match (requires every seat filled)
  - POST /games/{id}/actions - submit moves/passes/exchanges
  - PlayerActionDto enum: PlaceTiles, Pass, Exchange, Resign

- [x] Standard end-of-game rules (beyond resignation)
  - Going out: a player emptying their rack with the bag already empty ends the game immediately — everyone's score is reduced by the value of the tiles left on their rack, and the player who went out additionally receives the sum of everyone else's leftover rack value
  - Pass-out: 6 consecutive scoreless plays (passes or exchanges), summed across all seats, also ends the game (no bonus, same per-rack deduction)
  - Move-time-limit retirement: a seat that sits on its turn past `move_time_limit_seconds` (default 72h, configurable per game at creation) is auto-retired exactly like a resign, checked lazily on any touch of the game (no background scheduler)
  - `GameSession::finish_game` computes the winner as the highest final score, or `None` on an exact tie

- [x] Server-side scoring and legality
  - MoveValidator enforces rules
  - Score calculation via score.rs
  - Illegal moves rejected with error response

- [x] Game move history
  - MoveRecord struct persists (move_number, seat_number, move_type, main_word, score_delta, description)
  - Stored in game_moves table
  - Available via GameStateDto
  - `move_type` now includes `"timeout"` alongside `place`/`pass`/`exchange`/`resign`

- [x] Deterministic tests for move legality and scoring
  - rules-shared: 44 unit tests
  - server-game: 109 integration tests against the real Axum router
  - tile-lite-elite-ui: 93 unit tests
  - engine-core: 2 tests
  - **248 tests total** (excluding `old-crates/{first-try,second-try}`, two early prototypes kept for design-precedent reference but not counted here), all passing as of the last full run (2026-07-18) — re-run `cargo test --workspace` rather than trusting this number for long, it's climbed steadily every session

- [x] Move-composer UX (beyond drag-and-drop)
  - Click a board cell to select it, then click a rack tile or type its letter to place it — auto-advances to the next open cell in the direction the staged word is reading
  - Backspace deletes exactly the tile behind the cursor and lands on that cell (fixed — see `crates/ui/` notes above); Delete removes the tile at the cursor without moving it
  - Typing a letter with no matching rack tile but an unused blank auto-resolves the blank to that letter
  - Illegal-move text is short and specific, shared between the live `/preview` endpoint and the real submit path

- [x] Client resilience to server/network outages
  - `IS_ONLINE` (a global signal) is set the moment an HTTP call fails at the network level, distinguishing "server unreachable" from a legitimate rejection
  - A background loop pings `/health` every 3s while offline and reloads on recovery
  - The WebSocket subscription retries forever instead of dying permanently after the first drop
  - Verified live: killed the server under an active desktop client, confirmed no crash/busy-loop, restarted it, confirmed reconnection

- [x] Admin tooling
  - `tile-lite-elite-admin` CLI (`crates/admin-cli`): list/delete users, reset a password, list/delete/force-end games
  - Backed by `/admin/*` endpoints, loopback-gated regardless of environment (including inside the container deployment)

- [x] Game invitations (named, open/stranger, and email), fully wired to seat-claiming
  - Every human seat at creation is one of: the creator's own seat, a named invitation to a specific player, an open invitation any logged-in player can claim, or an email invitation to a raw address with no account required to exist yet
  - `GET /games` returns one flat list per caller, each entry tagged with why it's showing up (`YourTurn`/`Participant`/`InvitedByName`/`InvitedOpen`) and an `invitation_id` where relevant, so the client can accept/reject directly from the list — email invitations are deliberately *not* tagged `InvitedOpen` for every other player (excluded from `get_open_invitations`), since they're only meant to be reachable via their mailed link
  - Accepting an invitation (named, open, or email) atomically binds `player_id` to the seat — race-safe for open/email invitations (an atomic DB update means a second simultaneous accept sees "no longer available," not a silent double-claim), verified with a real two-player-race test
  - Anonymous/unauthenticated game creation and play has been retired entirely — creating or listing games now requires a session

- [x] Creator-managed roster on a `Waiting` game, after it's already been created
  - `POST /games/{id}/seats` adds a new seat without sending its invitation yet (staging); `POST /games/{id}/invite` sends or resends one, named/open/email; `POST /games/{id}/seats/{n}/remove` drops a seat (claimed or not) and renumbers the rest; `POST /games/{id}/seats/{n}/withdraw` lets the seat's own holder give it back voluntarily; `POST /games/{id}/reorder-seats` swaps turn order — all creator-only except `withdraw`
  - `POST /games/{id}/seats/{n}/force-resign` is the `Active`-game counterpart for an unresponsive participant — creator-only, ends the game immediately in the remaining player's favor, and (unlike a self-resign) doesn't require it to be that seat's own turn

- [x] Self-service password change and account-details editing
  - `POST /auth/change-password` requires the current password (not just a valid session token — protects against a stolen "remember me" token being enough to hijack the account on its own) and invalidates every session for the account on success, including the one that made the request
  - `POST /auth/update-details` changes display name and/or email without requiring the current password (a valid session is enough) and does *not* invalidate the session — the "Edit user details" dialog in `AuthPanel`

### ✅ Clients

- [x] Web client
  - Dioxus web WASM target
  - Compiles to wasm32-unknown-unknown
  - Serves on localhost:8080 via dx serve (dev), or as static files behind Caddy in the container deployment

- [x] Desktop client
  - Dioxus desktop target
  - Native GTK application
  - Runs with `cargo run -p tile-lite-elite-ui --features desktop` (or `./scripts/desktop.sh`)

- [x] Same codebase for both
  - Dual-target via Dioxus features (web/desktop)
  - Shared app.rs, components, HTTP client logic
  - Platform-specific conditional compilation for reqwest vs fetch

### ✅ Persistence

- [x] SQLite database per environment
  - Default: ./data/tile-lite-elite.sqlite3
  - TILE_LITE_ELITE_DATABASE_URL env var configurable (in the container deployment, a named Docker volume mounted at `/data`)

- [x] Migrations create schema
  - tables: schema_migrations (dead scaffolding — nothing inserts/reads it), players, engine_profiles, games, game_participants, game_moves, game_messages, sessions, game_invitations, password_reset_tokens — see `docs/4.2-database-schema.md` for the full field-by-field breakdown
  - Auto-created on server startup via persistence::migrate() — there's no real migration system beyond "create table if not exists", so a schema change (most recently `game_invitations.invited_email` and `sessions.stay_logged_in`) only takes effect on a fresh database; hit twice now, local dev and production DBs wiped both times — see the reset procedure in `docs/3.2-development.md`. Still an open gap, not a one-time cost — see Code Quality Notes below.
  - Also creates six indexes beyond the automatic primary-key/unique ones (`sessions(token_hash)`, `sessions(expires_at)`, `games(status, ended_at)`, `game_invitations(game_id)`, `game_invitations(invited_player_id)`, `game_messages(game_id)`) — added once real usage made clear which lookups were hot paths, not present from the start. Unlike a table/column addition, `create index if not exists` applies cleanly to an existing database, no wipe needed.

- [x] Durable game and platform data
  - games table: id, status, variant, language, board_layout, turn_number, current_seat, winner_seat, random_seed, snapshot_json (the actual authoritative state — full board/racks/bag/move-history/per-seat `resigned`/`removed_by_player`/`invited_email`; `game_participants`/`game_moves` below are denormalized read-optimizations derived from it, not a second source of truth)
  - game_participants: seat_number, kind, display_name, player_id, engine_id, score, joined_at, left_at
  - game_moves: move_number, seat_number, move_type, main_word, score_delta, description
  - game_messages: player_id, display_name, body, created_at (in-game chat)
  - players: display_name (unique), email, password_hash (argon2)
  - sessions: player_id, token_hash, expires_at (7 days, or null if `stay_logged_in`), stay_logged_in — expired rows deleted lazily on every `GET /games`
  - game_invitations: id, game_id, invited_player_id (nullable — null means open/stranger, or an email invitation before it's claimed), inviting_player_id, seat_number, status, created_at, responded_at, invited_email (nullable — set only for an email invitation; excluded from the generic open-invitations query)
  - password_reset_tokens: player_id, token_hash, expires_at (1 hour), consumed_at

### ✅ Server Infrastructure

- [x] Axum HTTP server
  - Listening on 127.0.0.1:3000 by default (configurable via TILE_LITE_ELITE_BIND — set to 0.0.0.0 for LAN play or behind the container's reverse proxy)
  - Full route list is in the `crates/server-game/` notes above
  - CORS enabled (CorsLayer::permissive()) for local dev; the container deployment sidesteps CORS entirely by serving client + API same-origin through Caddy

- [x] REST API shape correct
  - POST /games with CreateGameRequest (every human seat requires a claim — creator/named/open/email; auth required)
  - GET /games returns a per-caller filtered/tagged `Vec<GameSummaryDto>` (auth required)
  - GET /games/{id} returns GameStateDto
  - POST /games/{id}/actions with GameActionRequest — rejects a claimed seat if the caller isn't its owner; an unclaimed human seat rejects everyone
  - All responses are JSON, serde-compatible

- [x] Reconnect support
  - GET /games/{id} can reload full game state
  - Client can reconnect and fetch current board/racks/history

---

## Known Gaps vs Architecture Doc

### ⚠️ Not Yet Implemented (But Documented)

| Feature | Status | Notes |
|---------|--------|-------|
| WebSocket events for live updates | Implemented | Server broadcasts (filtered per-game — an earlier leak where every subscriber received every game's events, including other players' racks, is fixed), client subscribes, applies live updates, and works over `wss://` in the container deployment |
| Engine vs engine play | Tested | `engine_vs_engine_game_runs_to_completion` creates a two-`greedy-v1` game and asserts a single `/start` call drives it to `Finished` |
| Multiple engine implementations | Reference only | Only GreedyEngine exists; architecture supports plugging in more. `examples/repro_lexicon.rs` (see `crates/rules-shared/` notes) is intended as a shared benchmark harness for comparing future engines, not just the generator |
| Engine benchmarking | Not built | Engine trait supports it; CLI for benchmarking not implemented |
| Player identity (user accounts) | Implemented | Register/login/validate/change-password/update-details/forgot-password/reset-password all work end-to-end, with seat-ownership enforcement on every action-capable endpoint. Email addresses are still unverified (captured, never confirmed) — the only remaining gap in this row |
| Spectator mode | Not implemented | No spectator_id in schema or endpoints |
| Audit log | Not implemented | Schema exists (audit_log table), not used |
| Versioned engine contract | Not tested | Version strings exist in metadata, no migration tested |
| Engine turn timeout | Implemented | Engine runs via `spawn_blocking` raced against a 5s `tokio::time::timeout`; a timed-out engine auto-passes rather than stalling the game |
| Human turn timeout | Implemented | Move-time-limit auto-retirement (see MVP Checklist) — the human-facing equivalent of the engine timeout above, added this round of work |
| Save/load game state | Partial | Save works; load requires manual game ID (no UI) |
| CLI client | Not built | Architecture supports it; Dioxus CLI would compile it |
| Mobile client | Not built | Architecture supports Dioxus mobile target |
| Admin tooling | Implemented | `tile-lite-elite-admin` CLI + `/admin/*` endpoints |
| Client resilience to outages | Implemented | Connection-status tracking, background reconnect/reload, self-healing WebSocket |
| Container deployment | Implemented and live | See the "Deployment" entry in Architecture Alignment above |
| Structured logging | Implemented | `tracing` + `tracing-subscriber`, configurable via `RUST_LOG` (see `docs/3.5-production-support.md`'s "Logging" section). App-level events (auth, game lifecycle, invitations, admin actions, move-time-limit retirement) at `info`/`warn`; per-HTTP-request spans via `tower-http`'s `TraceLayer` at `debug`. Engine-decision diagnostics (why an engine chose a move) still don't exist — only that it timed out |
| Client API versioning | Implemented | `api::API_VERSION` (`Major.Minor`) checked against `/health` on first connect, in the shared bootstrap path (both web and desktop) — major mismatch blocks with an update message, minor mismatch is a soft notice (see `docs/4.1-configuration.md`'s "Versioning" section). Separate `Major.Minor.Patch[+build]` app version (from `Cargo.toml` + optional `TILE_LITE_ELITE_BUILD_ID`) is for display/logging only, not compatibility — server logs it at startup, desktop client shows it in the window title |

### ⚠️ Partially Implemented

**WebSocket Events**:
- Route `/games/{game_id}/events` exists, filtered per-game
- Client subscribes, applies live updates, retries forever (3s) instead of giving up after the first drop
- **Still missing**: the server doesn't replay missed events to a freshly (re)connected socket — a reconnecting client falls back to an explicit HTTP reload, which the client's background recovery loop does automatically

**Engine Execution**:
- EngineRegistry holds engines; GreedyEngine compiles and produces moves
- Server calls `run_engine_turns()` after game start, human moves, and suggest-move
- Engine moves flow through the same `apply_*` methods a human action goes through
- Runs via `tokio::task::spawn_blocking` raced against a 5s `tokio::time::timeout`; a timeout auto-passes the seat
- **Missing**: sandboxing, diagnostics/explanation output, a test that actually forces the timeout branch

**Authentication**:
- ✅ Schema: players table, sessions table, password_reset_tokens table
- ✅ POST /auth/register, /auth/login, /auth/validate, /auth/change-password, /auth/update-details, /auth/forgot-password, /auth/reset-password — all fully implemented
- ✅ GET /players/search?q= — case-insensitive display-name prefix search, backs the "invite by name" autocomplete
- ✅ Bearer-token auth threaded through the client for every action-submitting request
- ✅ Seat-ownership enforcement on every action-capable endpoint (`submit_action`, `start_game`, `preview_move`, `suggest_move`, seat reordering, and every roster-management endpoint) — an unclaimed human seat is rejected for everyone, not treated as open
- ✅ Login/Register/Change-password UI exists in `crates/ui` (web + desktop); Forgot-password/Reset-password UI exists too (a login-form toggle plus a standalone `/reset-password?token=...` landing page); "Edit user details" (display name + email, no password re-confirmation needed) replaced the old bare "Change password" trigger, with password-change kept as a nested sub-section
- ✅ Transactional email (welcome, named-invitation notification, email-invitation join link, password-reset) sends via Resend, live in production. Content lives in `crates/server-game/emails/*.txt` (currently placeholder wording). See `docs/2.5-authentication.md`'s status section.
- ⚠️ Email addresses are not verified in MVP (captured at registration, never confirmed) — unrelated to the transactional-email delivery above, which works regardless of whether an address has been verified.

**Game Invitations**:
- ✅ Schema includes game_invitations table, with `invited_player_id` nullable for open/stranger/unclaimed-email invitations and `invited_email` set only for an email invitation (excluded from the generic open-invitations query, so it isn't visible to every signed-in player the way a real `Open` seat is)
- ✅ POST /games/{game_id}/invite — invite a specific player by name, by email, or (both null) open the seat to any logged-in player; also the resend path for a declined seat
- ✅ POST /games/{game_id}/seats, POST /games/{game_id}/seats/{n}/{remove,withdraw}, POST /games/{game_id}/reorder-seats — creator-managed roster editing on a `Waiting` game, after it already exists (add a seat without sending its invitation yet, remove a claimed-or-not seat, let a seat's own holder withdraw, reorder turn order)
- ✅ POST /games/{game_id}/seats/{n}/force-resign — the `Active`-game counterpart, creator-only, for an unresponsive participant
- ✅ GET /invitations/{invitation_id}/preview — unauthenticated, backs the emailed join link's landing page (inviter name + status, before the visitor has necessarily registered or logged in)
- ✅ GET /players/{player_id}/invitations — list pending/responded named invitations
- ✅ GET /games — surfaces named and open pending invitations for the caller, tagged, alongside their actual games (email invitations are deliberately excluded — reachable only via their own link)
- ✅ POST /invitations/{invitation_id}/accept — binds `player_id` to the seat, race-safe for open/email invitations; returns the full updated `GameStateDto`, not just a status string
- ✅ POST /invitations/{invitation_id}/reject — named invitations only (an open/email invitation has no single invitee to reject on behalf of; not accepting is equivalent)

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
   - SeatKind, GameStatus, DirectionDto, SeatClaim, GameRelationship all properly typed
   - serde contracts enforce valid JSON shapes

3. **Database design**
   - Foreign keys and uniqueness constraints present
   - Schema supports future features (player_id, engine_id, spectator tracking)
   - Migrations auto-run on server startup (though see the "no real migration system" note above)

4. **Deployment readiness**
   - Single SQLite file simplifies backup/restore
   - No external service dependencies
   - Runs in a container with automatic HTTPS; also runs directly on any Linux/macOS/Windows system with Rust for local dev

### ⚠️ Areas for Attention

1. **Error handling**
   - ApiProblem type is defined but error messages could be more detailed
   - Invalid move rejections could log rule violation reason

2. **Testing**
   - Real coverage exists: 248 tests across the workspace (see the MVP Checklist above for the breakdown), including HTTP-level integration tests, the invitation/race-safety tests, and the security-relevant seat-ownership/uniqueness tests
   - Still missing: a test that forces the engine timeout branch, direct testing of the WebSocket events path itself (covered indirectly via live verification, not an automated test)

3. **Observability**
   - Structured logging via `tracing` now covers auth, game lifecycle, invitations, admin actions, and move-time-limit retirement (see `docs/3.5-production-support.md`)
   - Still missing: engine decisions aren't logged (only that an engine timed out, not what it considered or why it picked a move)
   - Performance metrics not tracked (though the move-generation performance fix was measured directly via `examples/repro_lexicon.rs`, not guessed at)

4. **Client-side validation**
   - Desktop/web clients fetch game state but don't pre-validate moves locally
   - Could use shared rules library to show illegal moves before submission

5. **Deployment process**
   - `scripts/deploy.sh` is a manual, on-demand push from a developer machine — no CI, no container registry. Appropriate for this project's actual deploy frequency today, but worth revisiting if that changes.
   - `crates/ui/Cargo.lock` is a stray, separately git-tracked lockfile distinct from the workspace-root one — a leftover from before `crates/ui` joined the workspace. Harmless but noted for eventual cleanup.

6. **No real schema migration system** — flagged before, and hit again this round (`game_invitations.invited_email`, added for email invitations, needed both the local dev DB and the production volume wiped, per the reset procedure in `docs/3.2-development.md`). `create table if not exists` only takes effect on a brand-new database; there's no `ALTER TABLE`/versioned-migration path, and the `schema_migrations` table exists but is dead scaffolding. Fine while there's no real user data to lose, but this is the second time it's cost a full data wipe for a column addition — worth solving for real (e.g. `sqlx migrate`) before it ever matters.

---

## Verification

Both clients (web via `dx serve`, desktop via `cargo run -p tile-lite-elite-ui --features desktop`) connect to the backend and support the full game loop: create a game (choosing the seat mix — creator/named-invite/open-invite/email-invite/engine — via presets or the seat-builder form), manage a `Waiting` game's roster after creation (add/remove/withdraw/resend a seat, reorder), discover and accept invitations (including via an emailed join link, with its own landing-page confirmation flow), place tiles (drag-and-drop or click/keyboard), pass, exchange, resign, force-resign an unresponsive opponent, play against the greedy engine, and log in/register/edit account details/change password with a persisted session. `cargo test --workspace` runs all 248 tests; see `docs/3.3-testing-and-staging.md` for exact commands.

The invitation flow, the move-time-limit auto-retirement, and the WebSocket event stream were all verified live end-to-end against the real deployed server (not just tests) — including a full production run against the live Oracle Cloud deployment: register → create → discover an open invitation → accept → start → live WebSocket updates over `wss://`, plus confirming SQLite data survives both a container restart and a full `docker compose down`/`up` recreation.

---

## Roadmap Alignment

### MVP (Current Target)

Status: **Core loop solid, player identity and matchmaking real (not just schema), deployed and live**

✅ Done:
- Server-owned game state, rule enforcement
- Human vs engine games, shared rules library, prefix-pruned move generation
- All four player actions (place, pass, exchange, resign)
- Persistence (SQLite), two client types (web + desktop)
- Live updates over WebSocket (server broadcasts per-game, client subscribes and applies)
- Real test coverage (248 tests, see above)
- Engine turn timeout (auto-pass on a slow engine rather than stalling)
- Player accounts: register/login/validate/change-password/update-details, argon2 password hashing, unique display names, a real login + "Edit user details" UI
- Seat-ownership enforcement on every action-capable endpoint
- Manual verification of seat-ownership cross-account behavior through the UI with two real accounts
- Standard end-of-game rules: going-out rack-penalty scoring, 6-scoreless-turn pass-out, and move-time-limit auto-retirement
- Engine-vs-engine test coverage
- Full invitation model: named, open/stranger, and email invitations; per-caller tagged games list; race-safe seat-claiming on accept
- Creator-managed roster editing on an already-created `Waiting` game (add/remove/withdraw/resend a seat, reorder), plus creator-only force-resign once `Active`
- Forgot-password and self-service password/details change, all fully wired with real transactional email via Resend
- Game creation via presets or a general seat-builder form (creator/named/open/email/engine rows + time limit)
- New games get a real random shuffle seed by default
- Move composer supports click/keyboard placement alongside drag-and-drop, with backspace/delete both behaving correctly and an Exchange flow
- Client survives server/network outages
- Admin CLI for operating the server (users, games), including inside the container deployment
- Container deployment: Docker + Caddy reverse proxy + automatic HTTPS, live on Oracle Cloud, with a scripted redeploy path
- Structured logging via `tracing`, configurable verbosity (`RUST_LOG`), covering auth/game-lifecycle/invitations/admin actions plus per-request HTTP tracing

❌ Not Yet:
- Email verification (addresses are captured and used for delivery, never confirmed as owned by the registrant)
- Engine decision diagnostics (why an engine chose a move, not just that it timed out)
- Multiple engine implementations, engine benchmarking CLI
- Client-side move pre-validation
- A repeatable (CI or registry-based) deploy pipeline — currently a manual script
- A real schema migration system — every schema change still costs a full data wipe (see Code Quality Notes above)

### v1 (Next Phase)

Expected next focus:
- Multiple engine implementations, using `examples/repro_lexicon.rs` as a shared benchmark harness
- Engine benchmarking CLI
- Email verification with short codes
- Full move validation test suite
- Client-side preview validation

---

## Conclusion

The architecture plan is **well-executed and now genuinely deployed**, not just running locally. The codebase correctly separates concerns, uses the right technologies (Rust, Axum, Dioxus, SQLite, Caddy), and has the foundation to support all documented features. Both clients work end-to-end today, including a real login/register/edit-details/change-password/forgot-password flow, seat-ownership protection on every action, a full invitation-based matchmaking model (named, open, and email invitations), and creator-managed roster editing after a game already exists. The live deployment on Oracle Cloud, behind automatic HTTPS at a real domain (tileliteelite.com), closes out what was previously a purely local/dev-only project; structured logging closed out the observability gap, and Resend-backed transactional email (welcome, named-invitation notification, email-invitation join link, password reset — all confirmed delivering for real) closes out what was the last piece of the auth story. The main remaining items are all optional/v1 scope or known, deliberately-deferred gaps: multiple engines, engine benchmarking, a less manual deploy pipeline, email verification, a real schema migration system (two schema changes now, two full data wipes), and replacing the placeholder email copy with the project owner's real wording.
