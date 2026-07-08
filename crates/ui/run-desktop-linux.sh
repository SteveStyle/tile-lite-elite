#!/usr/bin/env bash
set -euo pipefail

# Launch helper for the current ui crate.
# Usage:
#   ./run-desktop-linux.sh            # native desktop app
#   ./run-desktop-linux.sh desktop    # same as default
#   ./run-desktop-linux.sh web        # web dev server on port 8080

MODE="${1:-desktop}"
UI_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "${UI_DIR}/../.." && pwd)"

# Backend lives on 3000 in this workspace, so web UI defaults to 8080.
WEB_PORT="${SCRABBLE_UI_PORT:-8080}"
API_BASE_URL="${SCRABBLE_PX_API_BASE_URL:-http://127.0.0.1:3000}"

# Prefer the cargo-installed Dioxus CLI to avoid conflict with apt's /usr/bin/dx.
DIOXUS_DX="${HOME}/.cargo/bin/dx"

run_desktop() {
    echo "Starting desktop UI (native window)..."
    cd "${REPO_DIR}"
    SCRABBLE_PX_API_BASE_URL="${API_BASE_URL}" cargo run -p scrabble-ui --features desktop
}

run_web() {
    if [[ ! -x "${DIOXUS_DX}" ]]; then
        echo "Dioxus CLI not found at ${DIOXUS_DX}."
        echo "Install with: cargo install dioxus-cli"
        exit 1
    fi

    echo "Starting web UI on http://127.0.0.1:${WEB_PORT} ..."
    cd "${UI_DIR}"
    SCRABBLE_PX_API_BASE_URL="${API_BASE_URL}" "${DIOXUS_DX}" serve --platform web --port "${WEB_PORT}"
}

case "${MODE}" in
    desktop)
        run_desktop
        ;;
    web)
        run_web
        ;;
    *)
        echo "Unknown mode: ${MODE}"
        echo "Usage: $0 [desktop|web]"
        exit 2
        ;;
esac