# Schema

This is a first-pass SQLite schema for the project.

The goal is to keep the database simple enough for local use and free or near-free self-hosting, while still supporting game history, reconnects, engine-vs-engine play, and saved matches.

## Design Principles

- Keep the server authoritative.
- Store durable state in SQLite.
- Keep transient previews and search state out of the database.
- Prefer a few explicit tables over a large nested blob when the data is queried often.
- Use JSON only where the structure is naturally variable or best treated as a snapshot.

## Core Tables

### `schema_migrations`
Created by `persistence::migrate()`, but dead scaffolding in practice — nothing currently inserts into or reads from it. There is no real migration runner; schema evolution is entirely via additive `create table if not exists`/JSON-field-default changes, which is exactly why this table doing nothing hasn't blocked anything yet (see the migration-limitation note at the bottom of this doc).

Fields:

- `version` integer primary key
- `applied_at` text not null

### `games`
One row per game.

Fields:

- `id` text primary key
- `created_at` text not null
- `started_at` text null
- `ended_at` text null
- `status` text not null
- `variant` text not null
- `language` text not null
- `board_layout` text not null
- `turn_number` integer not null
- `current_seat` integer not null
- `winner_seat` integer null
- `random_seed` integer null
- `notes` text null
- `snapshot_json` text not null — the actual authoritative game state (full board, racks, bag, move history, per-seat `resigned`/`removed_by_player`/`invited_email`, etc.), deserialized as `PersistedGame`. Every other column on this table, plus `game_participants`/`game_moves` below, is a denormalized read-optimization derived from this blob, not a second source of truth.

### `game_participants`
Players or engines assigned to seats in a game — a queryable, denormalized mirror of the participant rows embedded in `games.snapshot_json` (see that column below), rewritten wholesale on every `save_game` call. `snapshot_json` is the actual source of truth for game state (including per-seat fields that aren't mirrored here, like `resigned`, `removed_by_player`, and `invited_email`); this table exists so the server can query across games (last-activity lookups, etc.) without deserializing every snapshot.

Fields:

- `id` text primary key
- `game_id` text not null foreign key to `games.id`
- `seat_number` integer not null
- `kind` text not null, for example `human` or `engine`
- `display_name` text not null
- `player_id` text null
- `engine_id` text null
- `score` integer not null default 0
- `joined_at` text not null
- `left_at` text null

Constraints:

- unique `game_id` + `seat_number`

### `game_moves`
Every move or turn action in order — same denormalized-mirror relationship to `snapshot_json` as `game_participants` above (the authoritative `MoveRecord` list lives in the snapshot; this table exists for querying).

Fields:

- `id` text primary key
- `game_id` text not null foreign key to `games.id`
- `move_number` integer not null
- `seat_number` integer not null
- `move_type` text not null, for example `place`, `pass`, `exchange`, `resign`
- `tiles_json` text null
- `payload_json` text not null
- `score_delta` integer not null default 0
- `created_at` text not null
- `is_validated` integer not null default 1

Constraints:

- unique `game_id` + `move_number`

### `game_messages`
In-game chat, one row per message.

Fields:

- `id` text primary key
- `game_id` text not null foreign key to `games.id`
- `player_id` text not null
- `display_name` text not null (the sender's display name at send time — not re-derived from `players` on read, so a later display-name change doesn't rewrite chat history)
- `body` text not null
- `created_at` text not null

### `game_invitations`
One row per invitation ever sent for a seat (a seat's full history — send, decline, resend — isn't overwritten, just appended). See `authentication-and-invitations.md` for the full seat-claim/invitation model this backs.

Fields:

- `id` text primary key
- `game_id` text not null foreign key to `games.id`
- `invited_player_id` text null — the specific invitee for a `Named` invitation; `null` for an `Open` invitation (any signed-in player may accept) or an `Email` invitation before it's been claimed
- `inviting_player_id` text not null
- `seat_number` integer not null
- `status` text not null — `pending`, `accepted`, `rejected`, or `cancelled`
- `created_at` text not null
- `responded_at` text null
- `invited_email` text null — set only for an `Email`-claim invitation (the address its join link was sent to). Distinguishes it from a plain `Open` invitation, which also has `invited_player_id is null` until claimed: `get_open_invitations` (the query behind the games list's generic "open invitations" section, visible to every signed-in player) explicitly excludes rows where this is set, since an email invite is only supposed to be reachable via its mailed link, not general browsing.

### `password_reset_tokens`
Single-use "forgot password" tokens — mirrors `sessions`'s hashed-secret shape rather than `game_invitations`'s plain-id shape, since a reset token is an unguessable secret, never used as a REST resource id.

Fields:

- `id` text primary key
- `player_id` text not null
- `token_hash` text not null
- `created_at` text not null
- `expires_at` text not null (1 hour after creation)
- `consumed_at` text null

### `game_state_snapshots` — not implemented
Not created by `persistence::migrate()`; described here only as a possible future addition (optional saved snapshots for replay, restore, or debugging) if `snapshot_json`'s current live-state-only approach ever needs point-in-time history. Read this section as a proposal, not a description of the current schema.

Fields:

- `id` text primary key
- `game_id` text not null foreign key to `games.id`
- `move_number` integer not null
- `board_json` text not null
- `bag_json` text not null
- `racks_json` text not null
- `created_at` text not null

Constraints:

- unique `game_id` + `move_number`

### `engine_profiles`
Registered engine metadata.

Fields:

- `id` text primary key
- `name` text not null
- `version` text not null
- `author` text null
- `description` text null
- `capabilities_json` text not null
- `created_at` text not null
- `updated_at` text not null

### `players`
Persistent player identity records.

Fields:

- `id` text primary key
- `display_name` text not null, **unique** (enforced at the DB level — two players cannot register the same display name)
- `email` text not null
- `email_verification_code_hash` text null
- `email_verification_sent_at` text null
- `email_verified_at` text null
- `password_hash` text not null (argon2; see `authentication.md`)
- `created_at` text not null
- `updated_at` text not null
- `last_seen_at` text null
- `preferences_json` text null

### `saved_games` — not implemented
Not created by `persistence::migrate()` — like `game_state_snapshots` above, a proposal for optional saved-match records beyond the live `games` table, not part of the current schema.

Fields:

- `id` text primary key
- `game_id` text not null foreign key to `games.id`
- `label` text null
- `saved_at` text not null
- `snapshot_json` text not null

### `sessions`
Lightweight reconnect and presence records.

Fields:

- `id` text primary key
- `player_id` text not null foreign key to `players.id`
- `game_id` text null
- `token_hash` text null
- `created_at` text not null
- `last_seen_at` text not null
- `expires_at` text null — set at login/register time: 7 days out if `stay_logged_in` was `false`, `null` (no expiry) if `true`. Expired rows are deleted lazily whenever `GET /games` runs (`persistence::delete_expired_sessions`) — this is what actually bounds the table's size, since most logins don't check "stay logged in" and would otherwise leave a permanent row per login.
- `stay_logged_in` integer not null default 0 — captured explicitly from the request rather than only inferred from `expires_at is null`, so it reads as a record of intent, not a reconstruction.

## Suggested Relationships

- One game has many participants and many moves (mirrored in `game_participants`/`game_moves`; authoritative in `games.snapshot_json`).
- One game has many chat messages (`game_messages`) and many invitations across its lifetime (`game_invitations`).
- One game may have many snapshots or saved-game records — not implemented, see those two tables' entries above.
- One session may point to a player and optionally a game.
- One player may have many sessions, and many password-reset tokens, over time.
- Engine metadata is separate from game state so engines can be managed independently.

## Actual Current Shape

The schema is created by `crates/server-game/migrations/0001_baseline.sql`, run automatically at server startup via sqlx's `Migrator` (`persistence::migrate`, a thin wrapper around `sqlx::migrate!("./migrations").run(pool)`). It creates, in order: `players`, `engine_profiles`, `games`, `game_participants`, `game_moves`, `game_messages`, `sessions`, `game_invitations`, `password_reset_tokens`. `game_state_snapshots` and `saved_games` remain proposals, not implemented. The old `schema_migrations` table (dead scaffolding — nothing ever inserted into or read from it) is dropped by this same migration and superseded by sqlx's own tracking table, `_sqlx_migrations`.

Beyond the automatic indexes SQLite creates for every `primary key`/`unique` column above, the baseline migration also explicitly creates a handful more, on columns that are looked up often enough to matter once row counts grow past what's convenient to full-scan: `sessions(token_hash)` (checked on every authenticated request), `sessions(expires_at)` and `games(status, ended_at)` (both scanned by the lazy cleanup jobs on every `GET /games`), and `game_invitations(game_id)` / `game_invitations(invited_player_id)` / `game_messages(game_id)` (per-game and per-player lookups).

## Notes

The exact representation of `board_json`, `bag_json`, and `racks_json` can evolve.

That data can stay compact and versioned so older games can still be replayed even if the live game model changes later.

**Schema migrations**: real, versioned migrations live in `crates/server-game/migrations/` (see that directory's `README.md` for the rules) and run automatically at server startup. This replaced an ad-hoc `create table if not exists` scheme that only took effect on a *freshly created* database file — an existing on-disk database silently kept its old schema, which bit production three times, most recently when `sessions.stay_logged_in` was added but the column never actually reached the already-running production database until a later, unrelated deploy broke every authenticated request. sqlx checksums every applied migration: if an already-applied migration file's content changes, the next server start **fails to boot** (`MigrateError::VersionMismatch`) instead of silently doing nothing — a schema change now either applies correctly or the server refuses to start, never a silent no-op.
