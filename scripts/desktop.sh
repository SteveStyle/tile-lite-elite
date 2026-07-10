#!/usr/bin/env bash
set -euo pipefail

# desktop.sh — Launch a desktop client instance
# Usage:
#   ./scripts/desktop.sh [server_url]
# 
# Default server_url: http://127.0.0.1:3000
# 
# The desktop client is a native window app that connects to the backend.
# It can be launched multiple times for multiple players.

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_URL="${1:-http://127.0.0.1:3000}"

echo "Launching desktop client..."
echo "Server: $SERVER_URL"
echo ""

cd "$REPO_DIR"
SCRABBLE_PX_API_BASE_URL="$SERVER_URL" cargo run -p scrabble-ui --features desktop
