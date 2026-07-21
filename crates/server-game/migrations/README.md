# Migrations

Real, versioned schema migrations, run automatically at server startup via
sqlx's `Migrator` (`persistence::migrate`, which just wraps
`sqlx::migrate!("migrations")`). This replaced an ad-hoc `create table if not
exists` scheme that looked fine but silently failed to apply to an existing
database three separate times — see `docs/schema.md`'s migration section for
the full history.

## Rules

1. **Never edit a migration file once it has been applied anywhere** (your
   local dev DB, production — anywhere). sqlx checksums every migration's
   content when it's applied. If the file changes afterward, the next server
   start **fails to boot** (`MigrateError::VersionMismatch`) rather than
   silently doing nothing. This is enforced, not just a convention.

2. **To change the schema, add a new file**: `NNNN_description.sql`, with
   `NNNN` the next sequential 4-digit number (`0002_...`, `0003_...`, ...).
   Use ordinary DDL for the actual change — `alter table ... add column
   ...`, a plain `create table ...` for a genuinely new table, etc. Do
   **not** use `if not exists`/`create table if not exists` for a real
   change — that pattern is only safe in `0001_baseline.sql`, where every
   target database is known in advance to already have (or not have) the
   exact same shape. A real migration should apply exactly once and fail
   loudly if it can't.

3. **Keep changes additive/backward-compatible** where practical — the same
   caution already used for `PersistedGame`/`PersistedVariantRules`'s
   `#[serde(default = ...)]` fields elsewhere in `persistence.rs`. Don't
   drop or rename a column a currently-running deployment still reads
   without a coordinated two-step change (ship the new shape read-only
   first, then remove the old column in a later migration once nothing
   depends on it).

4. **New migration files need a rebuild to be picked up.** `build.rs` tells
   Cargo to rerun the build when anything under `migrations/` changes, so a
   plain `cargo build`/`cargo run` after adding a file is enough — no extra
   step needed.

5. **No locking concerns on SQLite.** sqlx's cross-connection migration lock
   is a no-op for the SQLite driver (there's no advisory-lock primitive to
   use), so there's nothing to worry about with the many parallel test
   runs in `crates/server-game/src/app.rs` — each test already gets its own
   fresh on-disk database file anyway.
