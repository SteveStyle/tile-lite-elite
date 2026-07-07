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
Tracks applied migrations.

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

### `game_participants`
Players or engines assigned to seats in a game.

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
Every move or turn action in order.

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

### `game_state_snapshots`
Optional saved snapshots for replay, restore, or debugging.

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
- `display_name` text not null
- `email` text not null
- `email_verification_code_hash` text null
- `email_verification_sent_at` text null
- `email_verified_at` text null
- `recovery_secret_hash` text not null
- `created_at` text not null
- `updated_at` text not null
- `last_seen_at` text null
- `preferences_json` text null

### `saved_games`
Optional saved-match records if we want explicit save/load beyond the live `games` table.

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
- `expires_at` text null

## Suggested Relationships

- One game has many participants.
- One game has many moves.
- One game may have many snapshots.
- One game may have many saved-game records.
- One session may point to a player and optionally a game.
- One player may have many sessions over time.
- Engine metadata is separate from game state so engines can be managed independently.

## Recommended MVP Shape

For the first version, the minimum useful schema is:

- `schema_migrations`
- `players`
- `games`
- `game_participants`
- `game_moves`
- `engine_profiles`
- `sessions`

The snapshot and saved-game tables can be added when replay, restore, or manual save/load become first-class features.

## Notes

The exact representation of `board_json`, `bag_json`, and `racks_json` can evolve.

That data can stay compact and versioned so older games can still be replayed even if the live game model changes later.
