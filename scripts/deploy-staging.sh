#!/usr/bin/env bash
set -euo pipefail

# deploy-staging.sh — Build and run the staging stack locally (e.g. inside
# WSL), using the exact same Dockerfile/build process as scripts/deploy.sh,
# just without the ssh/scp hop to the real VM. See docs/3.3-testing-and-staging.md's
# "Staging Environment" section for why this exists and how to use it.
#
# Every mode builds from a committed HEAD, never a dirty working tree — see
# the "no uncommitted changes" check near the bottom. Commit first (a WIP
# commit is fine, nothing here gets pushed).
#
# Usage:
#   ./scripts/deploy-staging.sh              # build + (re)start the staging
#                                             # stack from the current HEAD
#   ./scripts/deploy-staging.sh down         # stop the staging stack, keep its data
#   ./scripts/deploy-staging.sh reset        # stop the staging stack and wipe its data
#   ./scripts/deploy-staging.sh at <git-ref> # wipe staging, then build + start
#                                             # from a specific commit/tag/branch —
#                                             # runs only the migrations that
#                                             # existed in the repo at that ref
#   ./scripts/deploy-staging.sh at prod      # same, but at whatever commit
#                                             # production is currently running
#                                             # (reads its /health endpoint)
#   ./scripts/deploy-staging.sh verify       # compare staging's live app_version
#                                             # against production's, without
#                                             # changing anything — run this
#                                             # before testing a new deployment,
#                                             # to confirm the starting point
#                                             # actually matches prod
#
# The staging data volume (tile-lite-elite-staging-data) persists across
# ordinary runs, same as production's — running this repeatedly against an
# already-seeded staging DB is what actually exercises "does a new
# migration apply to an existing database", not just a brand-new one.
#
# To test against a realistic copy of production data rather than whatever
# staging has accumulated on its own, restore a production backup
# (docs/3.5-production-support.md's Backups section) into
# tile-lite-elite-staging-data before running this.

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

COMPOSE=(docker compose -f docker-compose.staging.yml)
PROD_URL="${PROD_URL:-https://tileliteelite.com}"
STAGING_URL="http://localhost:8081"

# `/health`'s body is a small, fixed-shape JSON object (api::HealthDto) —
# grep/cut is enough to pull one field out of it without adding a jq
# dependency this script would otherwise have no use for.
fetch_app_version() {
  local url="$1" json version
  if ! json="$(curl -sf --max-time 5 "$url/health")"; then
    echo "error: couldn't reach $url/health" >&2
    return 1
  fi
  version="$(printf '%s' "$json" | grep -o '"app_version":"[^"]*"' | cut -d'"' -f4)"
  if [[ -z "$version" ]]; then
    echo "error: $url/health responded but had no app_version field — is it running code from before that field existed?" >&2
    return 1
  fi
  printf '%s\n' "$version"
}

MODE="${1:-up}"

case "$MODE" in
  down)
    "${COMPOSE[@]}" down
    exit 0
    ;;
  reset)
    "${COMPOSE[@]}" down -v
    exit 0
    ;;
  up)
    ;;
  at)
    REF="${2:-}"
    if [[ -z "$REF" ]]; then
      echo "Usage: $0 at <git-ref>|prod" >&2
      exit 1
    fi
    if [[ "$REF" == "prod" ]]; then
      echo "==> Checking production's live version ($PROD_URL/health)"
      PROD_VERSION="$(fetch_app_version "$PROD_URL")" || exit 1
      REF="${PROD_VERSION#*+}"
      if [[ "$REF" == "$PROD_VERSION" ]]; then
        echo "error: production's app_version ($PROD_VERSION) has no build id, so its exact commit isn't known — was it deployed via scripts/deploy.sh, which sets one automatically?" >&2
        exit 1
      fi
      echo "    Production is running $PROD_VERSION -> commit $REF"
    fi
    ;;
  verify)
    ;;
  *)
    echo "Usage: $0 [up|down|reset|at <git-ref>|at prod|verify]" >&2
    exit 1
    ;;
esac

if [[ "$MODE" == "verify" ]]; then
  echo "==> Checking production ($PROD_URL/health)"
  PROD_VERSION="$(fetch_app_version "$PROD_URL")" || exit 1
  echo "    production: $PROD_VERSION"
  echo "==> Checking staging ($STAGING_URL/health)"
  STAGING_VERSION="$(fetch_app_version "$STAGING_URL")" || exit 1
  echo "    staging:    $STAGING_VERSION"
  if [[ "$PROD_VERSION" == "$STAGING_VERSION" ]]; then
    echo "==> Match — staging is running the same version as production"
    exit 0
  else
    echo "==> Mismatch — staging is NOT running the same version as production" >&2
    echo "    Run './scripts/deploy-staging.sh at prod' to bring it in sync." >&2
    exit 1
  fi
fi

if [[ "$MODE" == "at" ]]; then
  # Fail fast with a clear message rather than a confusing worktree error
  # if the ref doesn't exist locally.
  if ! COMMIT="$(git rev-parse --verify "${REF}^{commit}" 2>/dev/null)"; then
    echo "error: '$REF' is not a valid local git ref (fetch it first if it's remote-only)" >&2
    exit 1
  fi
  SHORT_SHA="$(git rev-parse --short "$COMMIT")"

  echo "==> Wiping staging (about to redeploy at $REF ($SHORT_SHA) from scratch)"
  "${COMPOSE[@]}" down -v

  # A throwaway `git worktree` rather than checking out $REF in this working
  # copy — leaves the actual repo (branch, staged/uncommitted changes)
  # completely untouched. `docker build`'s context is a plain directory, so
  # this doesn't need docker-compose.yml/docker-compose.staging.yml to have
  # existed at that ref, only the Dockerfile and source under crates/.
  WORKTREE_DIR="$(mktemp -d /tmp/tile-lite-elite-staging-worktree-XXXXXX)"
  cleanup() {
    git worktree remove --force "$WORKTREE_DIR" 2>/dev/null || rm -rf "$WORKTREE_DIR"
  }
  trap cleanup EXIT
  # rmdir first: `git worktree add` refuses to target a directory mktemp
  # already created, even an empty one.
  rmdir "$WORKTREE_DIR"
  git worktree add --detach "$WORKTREE_DIR" "$COMMIT" >/dev/null

  echo "==> Building staging images from $REF ($SHORT_SHA)"
  docker build --target runtime-server \
    --build-arg TILE_LITE_ELITE_BUILD_ID="$SHORT_SHA" \
    -t tile-lite-elite-staging-server:latest "$WORKTREE_DIR"
  docker build --target runtime-web \
    --build-arg TILE_LITE_ELITE_BUILD_ID="$SHORT_SHA" \
    -t tile-lite-elite-staging-web:latest "$WORKTREE_DIR"

  echo "==> Starting staging stack"
  "${COMPOSE[@]}" up -d --no-build

  echo "==> Staging is up at http://localhost:8081, running $REF ($SHORT_SHA) against a fresh database"
  echo "    Back to the current working tree: ./scripts/deploy-staging.sh"
  echo "    Verify it matches production:     ./scripts/deploy-staging.sh verify"
  exit 0
fi

# Staging only ever deploys a real commit too, same reasoning as deploy.sh
# — otherwise "staging is running X" is only true by coincidence (whatever
# was on disk at build time), not something `at <ref>`/`verify` can actually
# trust later. Commit first (a WIP commit is fine — this is local, nothing
# is pushed) rather than testing uncommitted changes here.
if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree has uncommitted changes — deploy-staging.sh only builds from a committed HEAD. Commit (even a WIP one) or stash first:" >&2
  git status --short >&2
  exit 1
fi

# Same build-metadata wiring as scripts/deploy.sh — see
# docs/4.1-configuration.md's "Versioning" section.
export TILE_LITE_ELITE_BUILD_ID="$(git rev-parse --short HEAD)"

echo "==> Building staging images (build $TILE_LITE_ELITE_BUILD_ID)"
"${COMPOSE[@]}" build

echo "==> Starting staging stack"
"${COMPOSE[@]}" up -d

echo "==> Staging is up at http://localhost:8081"
echo "    Logs:    docker compose -f docker-compose.staging.yml logs -f server"
echo "    Down:    ./scripts/deploy-staging.sh down     (keeps data)"
echo "    Reset:   ./scripts/deploy-staging.sh reset    (wipes staging DB)"
echo "    Deploy a specific version: ./scripts/deploy-staging.sh at <git-ref>|prod"
