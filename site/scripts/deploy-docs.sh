#!/usr/bin/env bash
# Deploy a Starlight/Astro docs site into a GitHub user-pages repo and push.
#
# Workflow:
#   1. Build the site (`pnpm build`) if dist/ is stale or --rebuild is passed.
#   2. rsync the build into <deploy-dir>/<subdir>/ of the gh-pages working clone.
#   3. Stage, commit, push to the gh-pages repo.
#   4. Poll the live URL until Pages serves the content just pushed (or timeout).
#
# Step 4 compares the served body against the local build, not just the status
# code: a CDN serving the *previous* deploy answers 200 immediately, so a
# status-only check reports success while the site is still stale.
#
# Configuration:
#   Read from package.json's "deploy-docs" field, overridable by env vars and flags.
#
# Environment overrides:
#   DEPLOY_SLEEP     seconds between live-URL polls (default 8)
#   DEPLOY_ATTEMPTS  poll attempts before giving up (default 15)
#                     Lowered by scripts/deploy-docs.test.sh to keep it fast.
#
#   package.json:
#     "deploy-docs": {
#       "pagesRepo": "../greysquirr3l.github.io",  // path to the gh-pages working clone
#       "subdir": "protocol-droid",               // path inside the pages repo
#       "liveUrl": "https://greysquirr3l.github.io/protocol-droid/",
#       "sourceRepo": "greysquirr3l/protocol-droid" // (optional) for commit message
#     }
#
#   Flags override config: --pages-repo, --subdir, --live-url, --source-repo,
#   --rebuild, --message, --skip-build, --no-push, --no-poll.
#
# Exit codes:
#   0  success
#   1  unbuildable / bad config
#   2  gh-pages repo path is missing or not a git working tree
#   3  build failed
#   4  rsync failed
#   5  nothing to deploy (the build matches what is already deployed)
#   6  git push failed
#   7  live URL never served the pushed content within the poll window
#   8  Pages config on the SOURCE repo is shadowing the URL (404 "Site not found")
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SITE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$SITE_DIR"

# ---- read config from package.json -------------------------------------------------
# jq is part of the macOS developer toolchain; fall back to a node one-liner if missing.
# Bracket notation is required: ".deploy-docs" parses as ".deploy - docs" otherwise.
if command -v jq >/dev/null 2>&1; then
  cfg_get() { jq -r --arg k "$1" '.["deploy-docs"][$k] // empty' package.json 2>/dev/null || true; }
else
  cfg_get() { node -e "const p=require('./package.json'); const v=p['deploy-docs'] && p['deploy-docs'][process.argv[1]]; if (v !== undefined) process.stdout.write(String(v));" "$1" 2>/dev/null || true; }
fi

PAGES_REPO_DEFAULT="$(cfg_get pagesRepo)"
SUBDIR_DEFAULT="$(cfg_get subdir)"
LIVE_URL_DEFAULT="$(cfg_get liveUrl)"
SOURCE_REPO_DEFAULT="$(cfg_get sourceRepo)"

# ---- flags -------------------------------------------------------------------------
PAGES_REPO="${PAGES_REPO:-$PAGES_REPO_DEFAULT}"
SUBDIR="${SUBDIR:-$SUBDIR_DEFAULT}"
LIVE_URL="${LIVE_URL:-$LIVE_URL_DEFAULT}"
SOURCE_REPO="${SOURCE_REPO:-$SOURCE_REPO_DEFAULT}"

REBUILD=0
SKIP_BUILD=0
NO_PUSH=0
NO_POLL=0
COMMIT_MESSAGE=""

usage() {
  cat <<USAGE
Usage: $(basename "$0") [flags]

  --pages-repo PATH    path to the gh-pages working clone (default: $PAGES_REPO_DEFAULT)
  --subdir NAME        subdir inside the gh-pages repo (default: $SUBDIR_DEFAULT)
  --live-url URL       URL to poll for live verification (default: $LIVE_URL_DEFAULT)
  --source-repo OWNER/REPO
                       source repo, used in the deploy commit message
  --rebuild            force a fresh build even if dist/ is fresh
  --skip-build         skip the build step (use the existing dist/ as-is)
  --no-push            stage the changes but neither commit nor push; leaves the
                       clone dirty so a later real run deploys them
  --no-poll            do not poll the live URL after pushing
  --message MSG        custom commit message; default mentions the source commit
  -h, --help           show this help

Config is read from package.json's "deploy-docs" field. Flags and env vars
override config. See the top of this script for the field names.
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --pages-repo)    PAGES_REPO="$2"; shift 2 ;;
    --subdir)        SUBDIR="$2"; shift 2 ;;
    --live-url)      LIVE_URL="$2"; shift 2 ;;
    --source-repo)   SOURCE_REPO="$2"; shift 2 ;;
    --rebuild)       REBUILD=1; shift ;;
    --skip-build)    SKIP_BUILD=1; shift ;;
    --no-push)       NO_PUSH=1; shift ;;
    --no-poll)       NO_POLL=1; shift ;;
    --message)       COMMIT_MESSAGE="$2"; shift 2 ;;
    -h|--help)       usage; exit 0 ;;
    *)               echo "unknown flag: $1" >&2; usage; exit 1 ;;
  esac
done

# ---- validate config ---------------------------------------------------------------
[ -n "$PAGES_REPO" ] || { echo "error: --pages-repo not set and package.json 'deploy-docs.pagesRepo' missing" >&2; exit 1; }
[ -n "$SUBDIR" ]     || { echo "error: --subdir not set and package.json 'deploy-docs.subdir' missing" >&2; exit 1; }
[ -n "$LIVE_URL" ]   || { echo "error: --live-url not set and package.json 'deploy-docs.liveUrl' missing" >&2; exit 1; }

PAGES_REPO="$(cd "$PAGES_REPO" && pwd)" 2>/dev/null || {
  echo "error: pages repo path '$PAGES_REPO' is not accessible" >&2; exit 2; }
[ -d "$PAGES_REPO/.git" ] || {
  echo "error: '$PAGES_REPO' is not a git working tree" >&2; exit 2; }

TARGET_DIR="$PAGES_REPO/$SUBDIR"
mkdir -p "$TARGET_DIR"

# ---- build -------------------------------------------------------------------------
if [ "$SKIP_BUILD" = "1" ]; then
  echo "==> skipping build (using existing dist/)"
elif [ "$REBUILD" = "1" ] || [ ! -d dist ] || [ -z "$(ls -A dist 2>/dev/null)" ]; then
  echo "==> building..."
  pnpm build || exit 3
else
  echo "==> using existing dist/ (pass --rebuild to force a fresh build)"
fi

# ---- rsync -------------------------------------------------------------------------
echo "==> syncing dist/ into $TARGET_DIR"
# --checksum is not optional here. rsync's default quick check compares size and
# mtime, so a rebuilt page with unchanged byte length and a mtime that landed in
# the same second is skipped as "identical" — the deploy then commits a tree
# that still holds the previous deploy and reports success. The whole job is to
# publish exactly what was built, so pay for a content comparison.
rsync -av --delete --checksum --exclude='.git' \
  --exclude='.DS_Store' \
  "$SITE_DIR/dist/" "$TARGET_DIR/" || exit 4

# ---- commit + push ----------------------------------------------------------------
cd "$PAGES_REPO"

if [ -z "$COMMIT_MESSAGE" ]; then
  # Use the source repo's HEAD if available; otherwise the deploy-time date.
  if [ -n "$SOURCE_REPO" ] && [ -d "$SITE_DIR/.git" ]; then
    SRC_SHA=$(git -C "$SITE_DIR" rev-parse --short HEAD 2>/dev/null || echo "local")
    SRC_BRANCH=$(git -C "$SITE_DIR" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "main")
    COMMIT_MESSAGE="deploy: publish site from $SOURCE_REPO@$SRC_SHA ($SRC_BRANCH)"
  else
    COMMIT_MESSAGE="deploy: publish site ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
  fi
fi

# Stage only the subdir + any top-level housekeeping files we manage.
git add "$SUBDIR"
# .nojekyll: created on first run so Pages doesn't run Jekyll over _astro/.
[ -f "$PAGES_REPO/.nojekyll" ] || { touch "$PAGES_REPO/.nojekyll"; git add .nojekyll; }

if git diff --cached --quiet; then
  echo "==> nothing to deploy (the build matches what is already deployed)"
  exit 5
fi

# --no-push stages but does not commit. Committing here and skipping only the
# push would leave a commit in the clone, and the next real run would see a
# clean index, report "nothing to deploy", and exit 5 without ever pushing —
# so a preview silently became the thing that blocked the deploy. Leaving the
# work staged means the next run finds it and carries it through.
if [ "$NO_PUSH" = "1" ]; then
  echo "==> --no-push: staged, not committed (the next run commits and pushes these)"
  git --no-pager diff --cached --stat
  exit 0
fi

echo "==> committing"
git commit -m "$COMMIT_MESSAGE"

echo "==> pushing to origin"
git push origin HEAD || exit 6

# ---- live verification -------------------------------------------------------------
if [ "$NO_POLL" = "1" ]; then
  echo "==> --no-poll: skipping live URL check"
  exit 0
fi

echo "==> polling $LIVE_URL"

# Compare what Pages serves against what we just built. A CDN that has not yet
# picked up the new commit answers 200 with the *previous* deploy's bytes, so
# "200" alone proves reachability, not freshness. Digest the local build of the
# page the live URL points at, and require the served body to match it.
REL_PATH="${LIVE_URL#*://*/}"
REL_PATH="${REL_PATH%/}"

# `REL_PATH` is the *URL* path, so it carries the gh-pages subdirectory — for
# this project `protocol-droid`. The build, though, emits the site at the root
# of `dist/`; the subdirectory is applied when `dist/` is synced into the pages
# clone, not at build time. Looking for `dist/${REL_PATH}` therefore matches
# nothing whenever the site does not sit at the pages root, and the check
# degrades to the reachability-only poll it was written to replace — which is
# the exact failure it exists to catch, arriving through a different door.
#
# 0.4.3 added this body comparison and the changelog claimed it closed the
# stale-deploy hole. It had never run: the local page was never found, so
# `WANT_DIGEST` was empty and every run took the fallback branch.
DIST_REL="$REL_PATH"
case "$DIST_REL" in
    "$SUBDIR")   DIST_REL="" ;;
    "$SUBDIR"/*) DIST_REL="${DIST_REL#"$SUBDIR"/}" ;;
esac

if [ -n "$DIST_REL" ]; then
    CANDIDATES=(
        "$SITE_DIR/dist/$DIST_REL/index.html"
        "$SITE_DIR/dist/$DIST_REL.html"
        "$SITE_DIR/dist/$DIST_REL"
    )
else
    CANDIDATES=("$SITE_DIR/dist/index.html")
fi

LOCAL_PAGE=""
for candidate in "${CANDIDATES[@]}"; do
    [ -f "$candidate" ] && { LOCAL_PAGE="$candidate"; break; }
done

# cksum is in POSIX and on both macOS and Linux; md5sum/shasum differ by
# platform and flag spelling, which is not worth branching over for this.
digest_of_file() { cksum < "$1" | awk '{print $1 "-" $2}'; }
digest_of_stdin() { cksum | awk '{print $1 "-" $2}'; }

WANT_DIGEST=""
if [ -n "$LOCAL_PAGE" ]; then
  WANT_DIGEST="$(digest_of_file "$LOCAL_PAGE")"
  echo "  expecting the served body to match $(basename "$LOCAL_PAGE") (cksum $WANT_DIGEST)"
else
  echo "  ! no local build found for $REL_PATH — falling back to a status-code check"
  echo "    (this verifies reachability only; it cannot detect a stale deploy)"
fi

SLEEP_BETWEEN="${DEPLOY_SLEEP:-8}"
MAX_ATTEMPTS="${DEPLOY_ATTEMPTS:-15}"
for i in $(seq 1 $MAX_ATTEMPTS); do
  STATUS=$(curl -sS -o /dev/null -w '%{http_code}' "${LIVE_URL}?cb=$(date +%s%N)" 2>/dev/null || echo "000")
  if [ "$STATUS" = "200" ]; then
    if [ -z "$WANT_DIGEST" ]; then
      printf "  attempt %2d/%d: HTTP 200 (status only — cannot detect a stale deploy)\n" \
        "$i" "$MAX_ATTEMPTS"
      echo "==> live (reachability only; freshness unverified)"
      exit 0
    fi
    GOT_DIGEST="$(curl -sS "${LIVE_URL}?cb=$(date +%s%N)" 2>/dev/null | digest_of_stdin)"
    if [ "$GOT_DIGEST" = "$WANT_DIGEST" ]; then
      printf "  attempt %2d/%d: HTTP 200, body matches the build\n" "$i" "$MAX_ATTEMPTS"
      echo "==> live"
      exit 0
    fi
    printf "  attempt %2d/%d: HTTP 200 but the body is the previous deploy (cksum %s)\n" \
      "$i" "$MAX_ATTEMPTS" "$GOT_DIGEST"
  else
    printf "  attempt %2d/%d: HTTP %s\n" "$i" "$MAX_ATTEMPTS" "$STATUS"
  fi
  if [ "$STATUS" = "404" ] && [ "$i" -eq 4 ]; then
    echo "  still 404 after $i attempts — checking whether the SOURCE repo is shadowing this URL"
    if [ -n "$SOURCE_REPO" ] && command -v gh >/dev/null 2>&1; then
      if gh api "repos/$SOURCE_REPO" --jq '.has_pages // false' 2>/dev/null | grep -q true; then
        echo "  ! Pages is enabled on $SOURCE_REPO and is shadowing the URL."
        echo "    Fix: gh api -X DELETE repos/$SOURCE_REPO/pages"
        echo "    See site/README.md for context."
        exit 8
      fi
    fi
  fi
  sleep "$SLEEP_BETWEEN"
done

echo "error: $LIVE_URL never served the pushed content within $((SLEEP_BETWEEN * MAX_ATTEMPTS))s" >&2
exit 7
