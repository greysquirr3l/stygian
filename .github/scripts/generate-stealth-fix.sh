#!/usr/bin/env bash
# generate-stealth-fix.sh <probe-report.json>
#
# Calls the GitHub Models API (gpt-4o) to produce a targeted fix for
# crates/stygian-browser/src/stealth.rs based on the failing DiagnosticReport
# checks.
#
# Outputs:
#   - Writes proposed fix directly to stealth.rs (backup at stealth.rs.bak)
#   - Writes PR body to /tmp/pr-body.md
#   - Writes issue body to /tmp/issue-body.md (fallback for when cargo check
#     fails)
#
# Requires:
#   - GITHUB_TOKEN env var with models:read permission
#   - jq (pre-installed on all GitHub Actions ubuntu runners)

set -euo pipefail

REPORT_FILE="${1:?usage: generate-stealth-fix.sh <probe-report.json>}"
STEALTH_RS="crates/stygian-browser/src/stealth.rs"
MODELS_ENDPOINT="https://models.inference.ai.azure.com/chat/completions"
MODEL="gpt-4o"

# ── Summarise failing checks ─────────────────────────────────────────────────

FAILED_SUMMARY=$(jq -r '
  [.[] | .failed_checks[] | "  - \(.id): \(.details)"] | join("\n")
' "$REPORT_FILE")

if [[ -z "$FAILED_SUMMARY" ]]; then
  echo "No failed checks in report — nothing to fix."
  exit 0
fi

echo "Failing checks:"
echo "$FAILED_SUMMARY"

STEALTH_SOURCE=$(cat "$STEALTH_RS")
DATE=$(date -u +%Y-%m-%d)

# ── Build the API request ────────────────────────────────────────────────────

SYSTEM_PROMPT='You are a Rust expert specialising in headless browser stealth
and bot-detection evasion.

You will receive:
1. A list of failing DiagnosticCheck IDs and their detail strings from
   stygian-browser'\''s verify_stealth() function
2. The complete current source of stealth.rs

Your task is to produce a minimal fix so every listed check passes.

Respond with a JSON object containing exactly two keys:
  "analysis"  — brief plain-text explanation of what you changed and why
  "stealth_rs" — the COMPLETE corrected Rust source for stealth.rs

Constraints:
  - Rust stable, edition 2024
  - Zero clippy::pedantic warnings
  - No .unwrap() or .expect() in library code — use ? or match
  - Do not change any public API signatures
  - Preserve all existing doc comments unchanged
  - Output only the JSON object, no markdown, no extra text'

USER_CONTENT="Failing checks:
${FAILED_SUMMARY}

Current stealth.rs:
${STEALTH_SOURCE}"

PAYLOAD=$(jq -n \
  --arg model "$MODEL" \
  --arg system "$SYSTEM_PROMPT" \
  --arg user "$USER_CONTENT" \
  '{
    "model": $model,
    "response_format": { "type": "json_object" },
    "messages": [
      { "role": "system", "content": $system },
      { "role": "user",   "content": $user   }
    ]
  }')

# ── Call GitHub Models API ───────────────────────────────────────────────────

echo "Calling GitHub Models (${MODEL})…"
RESPONSE=$(curl --silent --fail \
  -H "Authorization: Bearer ${GITHUB_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD" \
  "$MODELS_ENDPOINT")

CONTENT=$(echo "$RESPONSE" | jq -r '.choices[0].message.content // empty')

if [[ -z "$CONTENT" ]]; then
  echo "ERROR: empty response from GitHub Models" >&2
  echo "$RESPONSE" >&2
  exit 1
fi

ANALYSIS=$(echo "$CONTENT" | jq -r '.analysis // "No analysis provided."')
NEW_STEALTH=$(echo "$CONTENT" | jq -r '.stealth_rs // empty')

if [[ -z "$NEW_STEALTH" ]]; then
  echo "ERROR: LLM response missing stealth_rs field" >&2
  echo "$CONTENT" >&2
  exit 1
fi

echo "Analysis: $ANALYSIS"

# ── Write the proposed fix ───────────────────────────────────────────────────

cp "$STEALTH_RS" "${STEALTH_RS}.bak"
printf '%s\n' "$NEW_STEALTH" >"$STEALTH_RS"
echo "Wrote proposed fix to ${STEALTH_RS}"

# ── Write PR body ────────────────────────────────────────────────────────────

cat >/tmp/pr-body.md <<EOF
## Automated stealth regression fix

**Date**: ${DATE}
**Source**: stealth-canary workflow → GitHub Models (${MODEL})

> This PR was generated automatically. Review the diff carefully before
> merging. The fix passed \`cargo check --workspace --all-features\` but full
> integration tests must be run before merge.

### Analysis

${ANALYSIS}

### Failing checks (before fix)

\`\`\`
${FAILED_SUMMARY}
\`\`\`

### Full probe report

\`\`\`json
$(cat "$REPORT_FILE")
\`\`\`
EOF

# ── Write issue body (fallback — fix did not compile) ───────────────────────

cat >/tmp/issue-body.md <<EOF
## Stealth regression detected

**Date**: ${DATE}

The stealth canary detected failing fingerprint checks. An automated fix was
attempted but failed \`cargo check\` and was not committed.

### Failing checks

\`\`\`
${FAILED_SUMMARY}
\`\`\`

### Full probe report

\`\`\`json
$(cat "$REPORT_FILE")
\`\`\`

---

### For @github-copilot

Please investigate this stealth regression using the stygian MCP browser tools:

1. Use \`browser_acquire\` with \`stealth_level: "advanced"\` to start a session
2. Run \`browser_verify_stealth\` on \`about:blank\` to reproduce the failing checks
3. Read \`crates/stygian-browser/src/stealth.rs\` — all injection scripts live here
4. Use \`browser_eval\` to inspect specific JS signals interactively
5. Produce a targeted fix and open a PR against \`main\`

#### Check ID → stealth.rs function mapping

| Check ID | Function |
|---|---|
| \`webdriver_flag\` | instance + prototype patches in \`injection_script()\` |
| \`chrome_object\` | \`chrome_object_script()\` |
| \`headless_user_agent\` | \`NavigatorProfile\` UA strings + \`navigator_spoof_script()\` |
| \`plugin_count\` | \`navigator_spoof_script()\` plugins array |
| \`languages_present\` | \`navigator_spoof_script()\` languages array |
| \`canvas_consistency\` | \`canvas_noise_script()\` |
| \`web_gl_vendor\` | \`webgl_spoof_script()\` |
| \`automation_globals\` | CDP globals cleanup in \`injection_script()\` |
| \`outer_window_size\` | \`navigator_spoof_script()\` outerWidth/outerHeight |
| \`notification_permission\` | \`navigator_spoof_script()\` Notification override |

The \`user_agent_data\` consistency signal (not a built-in CheckId but detectable
via \`browser_eval\`) is handled by \`user_agent_data_script()\`.
EOF

echo "Done."
echo " → PR body:    /tmp/pr-body.md"
echo " → Issue body: /tmp/issue-body.md"
