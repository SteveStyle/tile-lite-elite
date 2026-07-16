#!/usr/bin/env bash
set -euo pipefail

# deploy.sh — Build the container images locally and ship them to the
# deployment VM.
#
# Why not build on the VM: the Always Free shape this project runs on
# (VM.Standard.E2.1.Micro) has 1GB RAM, which isn't enough to compile the
# Rust/wasm workspace. So this always builds locally (where there's room)
# and transfers the finished images instead of the source — `docker save`,
# scp, `docker load`, `docker compose up`. See docs/operations.md's
# "Container Deployment" section for the full story, including the Oracle
# Cloud networking setup this assumes is already in place.
#
# Usage:
#   ./scripts/deploy.sh
#
# Configure via environment variables (defaults match the current VM):
#   DEPLOY_HOST      Public IP or hostname of the VM (default: 129.151.69.246)
#   DEPLOY_USER      SSH user (default: ubuntu)
#   DEPLOY_SSH_KEY   Private key path (default: ~/.ssh/oracle_scrabble)
#   DEPLOY_REMOTE_DIR  Directory on the VM holding docker-compose.yml (default: tile-lite-elite)

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEPLOY_HOST="${DEPLOY_HOST:-129.151.69.246}"
DEPLOY_USER="${DEPLOY_USER:-ubuntu}"
DEPLOY_SSH_KEY="${DEPLOY_SSH_KEY:-$HOME/.ssh/oracle_scrabble}"
DEPLOY_REMOTE_DIR="${DEPLOY_REMOTE_DIR:-tile-lite-elite}"

SSH_OPTS=(-i "$DEPLOY_SSH_KEY" -o ConnectTimeout=10)
REMOTE="$DEPLOY_USER@$DEPLOY_HOST"

cd "$REPO_DIR"

echo "==> Building images locally (this is the slow step, ~2-3 min)"
docker compose build

echo "==> Exporting images"
TMP_TAR="$(mktemp /tmp/tile-lite-elite-images-XXXXXX.tar.gz)"
trap 'rm -f "$TMP_TAR"' EXIT
docker save tile-lite-elite-server:latest tile-lite-elite-web:latest | gzip > "$TMP_TAR"
echo "    $(du -h "$TMP_TAR" | cut -f1) compressed"

echo "==> Transferring to $DEPLOY_HOST"
ssh "${SSH_OPTS[@]}" "$REMOTE" "mkdir -p $DEPLOY_REMOTE_DIR"
scp "${SSH_OPTS[@]}" "$TMP_TAR" docker-compose.yml "$REMOTE:$DEPLOY_REMOTE_DIR/"

echo "==> Loading images and restarting the stack"
REMOTE_TAR_NAME="$(basename "$TMP_TAR")"
ssh "${SSH_OPTS[@]}" "$REMOTE" "
    set -e
    cd $DEPLOY_REMOTE_DIR
    gunzip -c $REMOTE_TAR_NAME | docker load
    rm -f $REMOTE_TAR_NAME
    docker compose up -d
    docker image prune -f > /dev/null
"

echo "==> Ensuring the 'sa' alias for tile-lite-elite-admin is set up on the VM"
ssh "${SSH_OPTS[@]}" "$REMOTE" "
    grep -qF 'alias sa=' ~/.bashrc 2>/dev/null || \
        echo \"alias sa='docker compose -f ~/$DEPLOY_REMOTE_DIR/docker-compose.yml exec server tile-lite-elite-admin'\" >> ~/.bashrc \
        || echo \"    (warning: could not set up the 'sa' alias for tile-lite-elite-admin)\"
"

echo "==> Done — https://$DEPLOY_HOST.sslip.io (or your configured hostname)"
