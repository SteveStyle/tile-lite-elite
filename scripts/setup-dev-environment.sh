#!/usr/bin/env bash
set -euo pipefail

# setup-dev-environment.sh — Bootstrap a fresh Ubuntu (WSL2 or otherwise)
# checkout of this repo into a working dev + deploy environment: Rust
# toolchain, wasm target, the exact-matched dioxus-cli/wasm-bindgen-cli
# versions this project needs, sccache, and Docker Engine.
#
# Run this from inside a clone of the repo (it reads Cargo.lock to pick the
# right wasm-bindgen-cli version). Safe to re-run — every step checks
# whether it's already done first.
#
# What this does NOT do, on purpose:
#   - Restore the Oracle deploy SSH key. That's a secret; copy it back in
#     by hand (see docs/3.1-setup.md's "Development Environment Setup").
#   - Enable systemd in WSL. That's a Windows-side /etc/wsl.conf edit
#     needing a `wsl --shutdown`, which a script running inside the distro
#     can't safely trigger on itself. This script checks and warns instead.

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

echo "==> Checking WSL/systemd (Docker needs systemd to manage its service)"
if [ -f /proc/version ] && grep -qi microsoft /proc/version; then
    if [ "$(ps -p 1 -o comm=)" != "systemd" ]; then
        cat <<'EOF'
    WARNING: this looks like WSL without systemd enabled (PID 1 isn't
    systemd). Docker's `systemctl enable --now docker` step will fail.
    Fix: add to /etc/wsl.conf (as root):

        [boot]
        systemd=true

    then from Windows PowerShell: wsl --shutdown
    ...and restart this shell. Continuing anyway in case Docker isn't
    needed on this machine.
EOF
    else
        echo "    systemd OK"
    fi
fi

echo "==> System packages (build tools + dioxus-desktop's webview deps)"
sudo apt-get update -qq
sudo apt-get install -y -qq \
    build-essential pkg-config curl ca-certificates git \
    libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev

echo "==> Rust toolchain"
if ! command -v rustc >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
else
    echo "    already installed: $(rustc --version)"
fi
rustup target add wasm32-unknown-unknown

echo "==> dioxus-cli (must match crates/ui's dioxus/dioxus-web version)"
DIOXUS_VERSION="0.6.3"
if ! command -v dx >/dev/null 2>&1 || [[ "$(dx --version 2>&1)" != *"$DIOXUS_VERSION"* ]]; then
    cargo install dioxus-cli --version "$DIOXUS_VERSION" --locked
else
    echo "    already installed: $(dx --version)"
fi

echo "==> wasm-bindgen-cli (must exactly match the wasm-bindgen crate version in Cargo.lock)"
WASM_BINDGEN_VERSION="$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | sed -E 's/.*"(.*)"/\1/')"
if [ -z "$WASM_BINDGEN_VERSION" ]; then
    echo "    could not read wasm-bindgen version from Cargo.lock — skipping, install manually" >&2
elif ! command -v wasm-bindgen >/dev/null 2>&1 || [[ "$(wasm-bindgen --version 2>&1)" != *"$WASM_BINDGEN_VERSION"* ]]; then
    cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION" --locked
else
    echo "    already installed: $(wasm-bindgen --version)"
fi

echo "==> sccache (speeds up local rebuilds; .cargo/config.toml expects it at ~/.cargo/bin/sccache)"
if ! command -v sccache >/dev/null 2>&1; then
    cargo install sccache --locked
else
    echo "    already installed: $(sccache --version)"
fi

echo "==> Docker Engine"
if ! command -v docker >/dev/null 2>&1; then
    sudo install -m 0755 -d /etc/apt/keyrings
    sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
    sudo chmod a+r /etc/apt/keyrings/docker.asc
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" \
        | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null
    sudo apt-get update -qq
    sudo apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
    sudo systemctl enable --now docker || echo "    (systemctl failed — see the systemd warning above)"
    sudo usermod -aG docker "$USER"
    echo "    installed — you'll need a new shell session (or 'newgrp docker') for group membership to apply"
else
    echo "    already installed: $(docker --version)"
fi

cat <<EOF

==> Tooling setup done. Two manual steps left (see docs/3.1-setup.md):
    1. Copy your Oracle deploy SSH key back in:
         ~/.ssh/oracle_tile_lite_elite (private) and .pub — from your Windows backup.
    2. Start a new shell (for the docker group to take effect), then verify:
         cargo test --workspace
         dx --version
         docker compose version
         ssh -i ~/.ssh/oracle_tile_lite_elite ubuntu@129.151.69.246 echo ok
EOF
