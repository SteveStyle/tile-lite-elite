#!/usr/bin/env bash
set -euo pipefail

# admin.sh — Build (release) and run the admin CLI (`tile-lite-elite-admin`,
# crates/admin-cli) against the server on THIS machine.
#
# This only works against a server running on the same machine — the
# server's /admin/* endpoints reject anything that isn't a loopback
# connection, by design (see docs/3.4-production-environment.md's "Admin CLI" section).
# That means this script can't reach the Oracle VM's server; use
# `docker compose exec server tile-lite-elite-admin ...` there instead (same doc
# section covers it).
#
# Usage:
#   ./scripts/admin.sh users list
#   ./scripts/admin.sh games list --status waiting
#   ./scripts/admin.sh games force-end <game_id>

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

cargo build --release -p admin-cli
exec ./target/release/tile-lite-elite-admin "$@"
