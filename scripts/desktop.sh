#!/usr/bin/env bash
set -euo pipefail

# desktop.sh — Launch a desktop client instance
# Usage:
#   ./scripts/desktop.sh                       # compiled-in default (see crates/ui/src/config.rs)
#   ./scripts/desktop.sh <server_url>           # shorthand for --server-url <url>
#   ./scripts/desktop.sh --server-url <url>     # connect to an exact URL
#   ./scripts/desktop.sh --env <name>           # connect to a named environment (e.g. local, prod)
#
# The desktop client is a native window app that connects to the backend.
# It can be launched multiple times for multiple players.

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

if [[ $# -eq 0 ]]; then
    echo "Launching desktop client (compiled-in default server)..."
    cargo run -p tile-lite-elite-ui --features desktop
elif [[ "$1" == --* ]]; then
    echo "Launching desktop client..."
    cargo run -p tile-lite-elite-ui --features desktop -- "$@"
else
    echo "Launching desktop client..."
    echo "Server: $1"
    cargo run -p tile-lite-elite-ui --features desktop -- --server-url "$1"
fi
