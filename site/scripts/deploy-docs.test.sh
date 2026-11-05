#!/usr/bin/env bash
# Regression tests for deploy-docs.sh. Plain bash, no framework — run it directly:
#
#   bash site/scripts/deploy-docs.test.sh
#
# Two defects motivated these, both of which had shipped:
#
#   1. --no-push committed and skipped only the push, leaving a commit in the
#      pages clone. The next real run saw a clean index, reported "nothing to
#      deploy", exited 5, and never pushed — a preview silently became the thing
#      that blocked the deploy.
#
#   2. Live verification polled for HTTP 200. A CDN still serving the previous
#      deploy answers 200 immediately, so the first attempt passed while the
#      site was stale. The check now compares the served body against the build.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY="$SCRIPT_DIR/deploy-docs.sh"

PASS=0
FAIL=0

ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }
head2() { printf '\n\033[1m%s\033[0m\n' "$1"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/deploy-docs-test.XXXXXX")"
# shellcheck disable=SC2064
trap 'rm -rf "$WORK"; [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null' EXIT

git config --global user.email >/dev/null 2>&1 || true
GIT_ENV=(-c user.email=test@example.com -c user.name=deploy-docs-test -c commit.gpgsign=false)

# ---- fixture: a pages repo with one page, and a source site with a dist/ ------
setup_pages() {
  rm -rf "$WORK/pages" "$WORK/origin.git"
  git init -q --bare "$WORK/origin.git"
  git clone -q "$WORK/origin.git" "$WORK/pages" 2>/dev/null
  mkdir -p "$WORK/pages/protocol-droid"
  echo "<html><body>OLD</body></html>" > "$WORK/pages/protocol-droid/index.html"
  git -C "$WORK/pages" "${GIT_ENV[@]}" add -A
  git -C "$WORK/pages" "${GIT_ENV[@]}" commit -qm initial
  git -C "$WORK/pages" push -q origin HEAD
}

# Read the pushed content through a normal (non-bare) repo. `git` refuses to
# read a bare repo directly unless safe.bareRepository is relaxed, so fetch
# into the clone and read FETCH_HEAD — which is also the more honest question:
# did the remote actually receive it?
origin_content() {
  git -C "$WORK/pages" fetch -q origin 2>/dev/null
  git -C "$WORK/pages" show FETCH_HEAD:protocol-droid/index.html 2>/dev/null || echo '<missing>'
}

setup_site() {
  # The script derives SITE_DIR from its own location, so the test has to run a
  # *copy* of it inside a scratch site dir. Pointing the real script at a
  # fixture would mean testing against whatever site/dist happens to hold.
  rm -rf "$WORK/site"
  mkdir -p "$WORK/site/scripts" "$WORK/site/dist"
  cp "$DEPLOY" "$WORK/site/scripts/deploy-docs.sh"
  # The script reads its config from package.json; flags override it here, but
  # the file has to exist or the config read aborts the script.
  printf '{ "name": "fixture", "private": true }\n' > "$WORK/site/package.json"
  # dist/ holds site content at its root, exactly as a real Astro build does;
  # the deploy rsyncs it into <pages-repo>/<subdir>/. A dist/ that already
  # contained the subdir would nest it twice and hide the bug.
  echo "<html><body>NEW</body></html>" > "$WORK/site/dist/index.html"
  # Force the deployed file's mtime onto the built one. rsync's default quick
  # check is size+mtime, so without this the "same length, same second" case is
  # a coin flip — which is how the missing --checksum hid. NEW and OLD are the
  # same byte length on purpose; only the content differs.
  touch -r "$WORK/pages/protocol-droid/index.html" "$WORK/site/dist/index.html" 2>/dev/null || true
}

run_deploy() {
  # A default --live-url is passed first so config-only validation passes; the
  # flag parser takes the last occurrence, so callers override it by appending.
  ( cd "$WORK/site" && bash scripts/deploy-docs.sh \
      --pages-repo "$WORK/pages" \
      --subdir protocol-droid \
      --live-url http://127.0.0.1:1/ \
      --skip-build \
      "$@" )
}

# ---- 1. syntax ------------------------------------------------------------------
head2 "script parses"
if bash -n "$DEPLOY" 2>"$WORK/syntax.err"; then
  ok "bash -n"
else
  bad "bash -n: $(cat "$WORK/syntax.err")"
fi

# ---- 2. --no-push must not commit -------------------------------------------------
head2 "--no-push stages without committing"
setup_pages; setup_site

out="$(run_deploy --no-push --no-poll 2>&1)"; code=$?

ahead="$(git -C "$WORK/pages" rev-list --count origin/HEAD..HEAD 2>/dev/null || echo 0)"
staged="$(git -C "$WORK/pages" diff --cached --name-only | wc -l | tr -d ' ')"

[ "$code" -eq 0 ] && ok "exits 0" || bad "exits $code (expected 0)"
if [ "$ahead" -eq 0 ]; then
  ok "left no commit behind (the bug: it committed)"
else
  bad "left $ahead commit(s) behind — the next real run would report nothing to deploy"
fi
if [ "$staged" -gt 0 ]; then
  ok "left the change staged ($staged file(s))"
else
  bad "left nothing staged, so the preview is not carried into the next run"
fi

# ---- 3. a real run after a preview must still push --------------------------------
head2 "a real run after --no-push still deploys (the regression itself)"
out="$(run_deploy --no-poll 2>&1)"; code=$?

if [ "$code" -ne 5 ]; then
  ok "exits $code, not 5 (5 would mean the deploy was skipped)"
else
  bad "exited 5 — the preview blocked the deploy"
fi

pushed="$(origin_content)"
if echo "$pushed" | grep -q NEW; then
  ok "the pushed remote contains the new content"
else
  bad "origin still serves: $pushed"
fi
if git -C "$WORK/pages" diff --cached --quiet && [ -z "$(git -C "$WORK/pages" status --porcelain)" ]; then
  ok "clone left clean"
else
  bad "clone left dirty after a successful deploy"
fi

# ---- 4. a stale CDN must not read as live -----------------------------------------
head2 "a stale 200 is not success"
setup_pages; setup_site

# Serve the OLD bytes at the live URL while dist/ holds the NEW ones, which is
# exactly the state a CDN is in for the first minute after a push.
STALE="$WORK/stale"
mkdir -p "$STALE"
echo "<html><body>OLD</body></html>" > "$STALE/index.html"
( cd "$STALE" && exec python3 -m http.server 8731 --bind 127.0.0.1 ) >/dev/null 2>&1 &
SERVER_PID=$!
sleep 1

out="$(DEPLOY_SLEEP=1 DEPLOY_ATTEMPTS=3 run_deploy \
  --live-url http://127.0.0.1:8731/ 2>&1)"; code=$?

kill "$SERVER_PID" 2>/dev/null; SERVER_PID=""

if [ "$code" -eq 7 ]; then
  ok "exits 7 after the poll window (refused to call a stale site live)"
else
  bad "exits $code (expected 7) — it accepted a stale deploy as live"
fi
if echo "$out" | grep -q "previous deploy"; then
  ok "reports that the body is the previous deploy"
else
  bad "did not explain that the served body was stale"
fi

# ---- 5. a fresh serve is live ------------------------------------------------------
head2 "a matching body is live"
setup_pages; setup_site

FRESH="$WORK/fresh"
mkdir -p "$FRESH"
cp "$WORK/site/dist/index.html" "$FRESH/index.html"
( cd "$FRESH" && exec python3 -m http.server 8732 --bind 127.0.0.1 ) >/dev/null 2>&1 &
SERVER_PID=$!
sleep 1

out="$(DEPLOY_SLEEP=1 DEPLOY_ATTEMPTS=5 run_deploy \
  --live-url http://127.0.0.1:8732/ 2>&1)"; code=$?

kill "$SERVER_PID" 2>/dev/null; SERVER_PID=""

if [ "$code" -eq 0 ] && echo "$out" | grep -q "==> live"; then
  ok "exits 0 and reports live"
else
  bad "exits $code (expected 0); output: $(echo "$out" | tail -3 | tr '\n' ' ')"
fi

# ---- summary -----------------------------------------------------------------------
printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]