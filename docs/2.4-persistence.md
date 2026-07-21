# Persistence

## SQLite File Layout

The project should use one primary SQLite database file per environment.

Typical examples:

- local development: `./data/tile-lite-elite.sqlite3`
- self-hosted instance: `/var/lib/tile-lite-elite/tile-lite-elite.sqlite3`
- tests: temporary throwaway database files

SQLite may also create sidecar files when write-ahead logging is enabled:

- `tile-lite-elite.sqlite3-wal`
- `tile-lite-elite.sqlite3-shm`

Those files are part of normal SQLite operation and do not represent separate databases.

## What Is Persisted

The database should store durable game and platform data such as:

- active games and finished games
- board state snapshots or move history
- tile bag state and player racks if needed for recovery
- player or session records
- engine metadata and configuration
- match results and audit history
- saved game metadata
- migration version state

## What Stays In Memory

The following should generally stay in memory or be recomputed:

- current UI state
- transient preview calculations
- temporary engine search state
- cached move evaluations that can be rebuilt

## Persistence Model

The server remains authoritative and writes the canonical state to SQLite.

Clients and engine proxies may use the shared rules library for previews, but they do not own persistence.

Good persistence behavior for this project means:

- a match can be resumed after a restart
- finished games remain available for history and replay
- the database can be backed up by copying the single main file, ideally after a clean shutdown or with WAL-aware backup procedures
- the design stays simple enough to run locally or on a free tier

## Recommended Approach

Use migrations to evolve the schema over time.

Prefer simple, explicit tables over complex embedded blobs unless a blob is the clearest way to capture a replay snapshot.

Keep the write path small and predictable so the project stays easy to self-host.