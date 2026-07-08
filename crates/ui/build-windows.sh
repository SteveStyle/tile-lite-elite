#!/usr/bin/env bash
set -euo pipefail

# Build helper for the current workspace layout.
# This script does not rewrite manifests.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "${REPO_DIR}"

echo "Building scrabble-ui desktop release..."
echo "Workspace: ${REPO_DIR}"

# If running on Windows, this produces target/release/scrabble-ui.exe.
# On Linux/macOS, this builds the local desktop binary for that platform.
cargo build -p scrabble-ui --release --features desktop

echo
echo "Build complete."
echo "Windows artifact path (when built on Windows): target/release/scrabble-ui.exe"