#!/usr/bin/env bash
set -euo pipefail

# deploy.sh — Build the container images locally and ship them to the
# deployment VM.
#
# Why not build on the VM: the Always Free shape this project runs on
# (VM.Standard.E2.1.Micro) has 1GB RAM, which isn't enough to compile the
# Rust/wasm workspace. So this always builds locally (where there's room)
# and transfers the finished images instead of the source — `docker save`,
# scp, `docker load`, `docker compose up`. See docs/3.3-testing-ci-and-release.md's
# "Container Deployment" section for the full story, including the Oracle
# Cloud networking setup this assumes is already in place.
#
# Refuses to run unless: the working tree is clean, HEAD has been pushed
# to its upstream, and local staging is currently running that exact
# commit. See the checks below for why each one exists.
#
# Usage:
#   ./scripts/deploy.sh
#
# Configure via environment variables (defaults match the current VM):
#   DEPLOY_HOST      Public IP or hostname of the VM (default: 129.151.69.246)
#   DEPLOY_USER      SSH user (default: ubuntu)
#   DEPLOY_SSH_KEY   Private key path (default: ~/.ssh/oracle_tile_lite_elite)
#   DEPLOY_REMOTE_DIR  Directory on the VM holding docker-compose.yml (default: tile-lite-elite)

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEPLOY_HOST="${DEPLOY_HOST:-129.151.69.246}"
DEPLOY_USER="${DEPLOY_USER:-ubuntu}"
DEPLOY_SSH_KEY="${DEPLOY_SSH_KEY:-$HOME/.ssh/oracle_tile_lite_elite}"
DEPLOY_REMOTE_DIR="${DEPLOY_REMOTE_DIR:-tile-lite-elite}"
STAGING_URL="${STAGING_URL:-http://localhost:8081}"

SSH_OPTS=(-i "$DEPLOY_SSH_KEY" -o ConnectTimeout=10)
REMOTE="$DEPLOY_USER@$DEPLOY_HOST"

cd "$REPO_DIR"

# Production only ever deploys a real commit, never whatever happens to be
# sitting in the working tree — otherwise the build-ID label baked into the
# image (below) would describe an older commit than what's actually running,
# exactly the mismatch that motivated adding it in the first place. Commit
# first (or stash) rather than deploying uncommitted changes.
if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree has uncommitted changes — deploy.sh only builds from a committed HEAD. Commit or stash first:" >&2
  git status --short >&2
  exit 1
fi

HEAD_SHA="$(git rev-parse --short HEAD)"

# Refuses to ship a commit that only exists on this machine — if this
# machine were lost before anyone/anything else had a copy, "what's
# actually running in production" would stop being reproducible from
# source control at all. Fetches first so a stale local view of the
# remote can't produce a false pass.
CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
UPSTREAM="$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null || true)"
if [[ -z "$UPSTREAM" ]]; then
  echo "error: '$CURRENT_BRANCH' has no upstream tracking branch — push it first: git push -u origin $CURRENT_BRANCH" >&2
  exit 1
fi
git fetch --quiet "${UPSTREAM%%/*}"
if ! git merge-base --is-ancestor HEAD "$UPSTREAM"; then
  echo "error: HEAD ($HEAD_SHA) hasn't been pushed to $UPSTREAM — push first: git push" >&2
  exit 1
fi
echo "==> HEAD ($HEAD_SHA) confirmed pushed to $UPSTREAM"

# Refuses to ship a commit that was never actually exercised in staging —
# a passing `cargo test` only proves the code compiles and unit-tests
# clean, not that it boots/migrates cleanly in a real container. Without
# this check, testing commit A in staging and then committing a "quick
# fix" B before deploying would silently ship B untested — easy to do
# without noticing, since deploy.sh has no other way to know staging
# wasn't re-run. See docs/3.3-testing-ci-and-release.md.
STAGING_HEALTH="$(curl -sf --max-time 5 "$STAGING_URL/health" 2>/dev/null || true)"
STAGING_VERSION="$(printf '%s' "$STAGING_HEALTH" | grep -o '"app_version":"[^"]*"' | cut -d'"' -f4)"
STAGING_SHA="${STAGING_VERSION#*+}"
if [[ -z "$STAGING_VERSION" ]]; then
  echo "error: local staging ($STAGING_URL) isn't reachable — test this commit there first: ./scripts/deploy-staging.sh" >&2
  exit 1
elif [[ "$STAGING_SHA" == "$STAGING_VERSION" ]]; then
  echo "error: staging is running $STAGING_VERSION, which has no commit id — was it deployed via deploy-staging.sh?" >&2
  exit 1
elif [[ "$STAGING_SHA" != "$HEAD_SHA" ]]; then
  echo "error: staging is running commit $STAGING_SHA, not HEAD ($HEAD_SHA) — test HEAD in staging first: ./scripts/deploy-staging.sh" >&2
  exit 1
fi
echo "==> Staging confirmed running this commit ($HEAD_SHA) — proceeding"

# Baked into both binaries as SemVer build metadata (e.g. `0.2.0+a1c9f02`) —
# see docs/4.1-configuration.md's "Versioning" section.
export TILE_LITE_ELITE_BUILD_ID="$HEAD_SHA"

echo "==> Building images locally (this is the slow step, ~2-3 min) [build $TILE_LITE_ELITE_BUILD_ID]"
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

echo "==> Ensuring the 'sa' alias for tile-lite-elite-admin is set up and current on the VM"
ssh "${SSH_OPTS[@]}" "$REMOTE" "
    # Drop any existing 'alias sa=' line first, then re-append the current
    # definition. A stale line (e.g. an old deploy dir, or the pre-rename
    # admin binary name) used to survive because the previous logic only
    # appended when *no* 'alias sa=' line existed at all — so an out-of-date
    # value was never refreshed. Delete-then-append converges to exactly one
    # correct line whether the alias was absent, current, or stale.
    sed -i '/alias sa=/d' ~/.bashrc 2>/dev/null || true
    echo \"alias sa='docker compose -f ~/$DEPLOY_REMOTE_DIR/docker-compose.yml exec server tile-lite-elite-admin'\" >> ~/.bashrc \
        || echo \"    (warning: could not set up the 'sa' alias for tile-lite-elite-admin)\"
"

echo "==> Done — https://$DEPLOY_HOST.sslip.io (or your configured hostname)"
