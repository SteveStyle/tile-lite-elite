#!/usr/bin/env bash
set -euo pipefail

# e2e-clean.sh — remove Playwright E2E test data from the local dev database.
#
# Deletes every player whose display_name starts with the test prefix
# (default "e2e-"), plus the games they took part in and all dependent rows
# (moves, messages, participants, invitations, rating history, sessions,
# tokens, ratings). Safe to run any time — it only ever touches prefixed test
# data. Runs automatically as the Playwright global teardown; also runnable by
# hand (`./scripts/e2e-clean.sh`) or via `npm run clean` from e2e/.
#
# Note: this edits the DB directly. Any of these games still cached in a
# running server's memory are harmless leftovers — they clear on the next
# server restart and won't be re-saved unless touched.

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${E2E_PREFIX:-e2e-}"
DB="${E2E_DB_FILE:-$REPO_DIR/data/tile-lite-elite.sqlite3}"

if ! command -v sqlite3 >/dev/null 2>&1; then
    echo "e2e-clean: sqlite3 not found on PATH; skipping" >&2
    exit 0
fi
if [[ ! -f "$DB" ]]; then
    echo "e2e-clean: no database at $DB (nothing to do)"
    exit 0
fi

# Escape any single quotes in the prefix for safe inlining into the LIKE.
esc_prefix=${PREFIX//\'/\'\'}

n=$(sqlite3 "$DB" "select count(*) from players where display_name like '${esc_prefix}%';")

sqlite3 "$DB" <<SQL
begin;
create temp table _p as select id from players where display_name like '${esc_prefix}%';
create temp table _g as select distinct game_id from game_participants where player_id in (select id from _p);

delete from game_moves        where game_id in (select game_id from _g);
delete from game_messages     where game_id in (select game_id from _g);
delete from game_participants where game_id in (select game_id from _g);
delete from game_invitations  where game_id in (select game_id from _g);
delete from rating_history    where game_id in (select game_id from _g);
delete from games             where id      in (select game_id from _g);

delete from sessions              where player_id in (select id from _p);
delete from password_reset_tokens where player_id in (select id from _p);
delete from player_ratings    where subject_kind = 'player' and subject_id in (select id from _p);
delete from rating_history    where subject_kind = 'player' and subject_id in (select id from _p);
delete from players           where id in (select id from _p);
commit;
SQL

echo "e2e-clean: removed ${n} test player(s) matching '${PREFIX}%' and their games from ${DB#"$REPO_DIR"/}"
