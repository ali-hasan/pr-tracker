#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"  # this repo's own root (pr-tracker/)

if [[ ! -f "$SCRIPT_DIR/config.sh" ]]; then
  echo "review-one.sh: $SCRIPT_DIR/config.sh not found — copy config.sh.example to config.sh and fill in CLAUDE_BIN + REPOS_DIR" >&2
  exit 1
fi
CLAUDE_BIN=""
REPOS_DIR=""
source "$SCRIPT_DIR/config.sh"
if [[ -z "$CLAUDE_BIN" || -z "$REPOS_DIR" ]]; then
  echo "review-one.sh: CLAUDE_BIN and REPOS_DIR must both be set in $SCRIPT_DIR/config.sh" >&2
  exit 1
fi

LOG_FILE="$SCRIPT_DIR/run.log"
RUNNING_FILE="$SCRIPT_DIR/RUNNING"

export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

REPO=""
NUMBER=""
LAYER1=true
LAYER2=true
EFFORT="low"
LAYER3=true
NOTES=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --number) NUMBER="$2"; shift 2 ;;
    --layer1) LAYER1="$2"; shift 2 ;;
    --layer2) LAYER2="$2"; shift 2 ;;
    --effort) EFFORT="$2"; shift 2 ;;
    --layer3) LAYER3="$2"; shift 2 ;;
    --notes) NOTES="$2"; shift 2 ;;
    *) echo "review-one.sh: unknown arg $1" >> "$LOG_FILE"; shift ;;
  esac
done

if [[ -z "$REPO" || -z "$NUMBER" ]]; then
  echo "review-one.sh: --repo and --number are required" >> "$LOG_FILE"
  exit 1
fi

if [[ -f "$RUNNING_FILE" ]]; then
  echo "=== Manual review for $REPO#$NUMBER skipped: another review is already running ===" >> "$LOG_FILE"
  exit 1
fi

case "$REPO" in
  cioplenu/backend) LOCAL_PATH="$REPOS_DIR/api" ;;
  cioplenu/frontend) LOCAL_PATH="$REPOS_DIR/frontend" ;;
  *) echo "review-one.sh: unknown repo $REPO" >> "$LOG_FILE"; exit 1 ;;
esac

echo "$REPO#$NUMBER" > "$RUNNING_FILE"
echo "=== Manual review started: $(date), $REPO#$NUMBER (layer1=$LAYER1 layer2=$LAYER2:$EFFORT layer3=$LAYER3 notes=${NOTES:-none}) ===" >> "$LOG_FILE"

# Built with printf (%s substitution, never shell re-interpreted) so backticks, apostrophes and
# whatever the user types into --notes can't break quoting or get executed as commands.
LAYERS=""
if [[ "$LAYER1" == true ]]; then
  LAYERS+=$(printf '\n**Layer 1 — repo approval-readiness rubric.** Read and apply %s/.github/skills/code-review/SKILL.md. This decides auto-approvable vs needs-human-review per that repos own stated categories. Keep the reasoning to one short line.\n' "$LOCAL_PATH")
fi
if [[ "$LAYER2" == true ]]; then
  LAYERS+=$(printf '\n**Layer 2 — the `code-review` skill.** Invoke it (`Skill` tool, skill name `code-review`) with args `%s <PR URL>`. Pass `%s` exactly as the effort level, do not change it.\n' "$EFFORT" "$EFFORT")
fi
if [[ "$LAYER3" == true ]]; then
  LAYERS+=$(printf '\n**Layer 3 — your own criteria.** Read %s/pr-review/review-criteria.md and apply anything written there on top of the layers above (it may be empty, skip if so).\n' "$BASE_DIR")
fi
if [[ -n "$NOTES" ]]; then
  LAYERS+=$(printf '\n**Layer 4 — this runs specific ask.** %s Weight this heavily, its what was specifically asked to be looked at on this re-run.\n' "$NOTES")
fi

TEMPLATE_FILE=$(mktemp)
# One trap covering both cleanup jobs — a second `trap ... EXIT` call doesn't add to the
# first, it silently replaces it, which is exactly what broke the RUNNING lock cleanup
# below when it was its own separate trap: this one only ran, the lock one never did.
trap 'rm -f "$RUNNING_FILE" "$TEMPLATE_FILE"' EXIT

# Single-quoted heredoc: nothing in here is expanded by bash, so the backticks and $ signs used
# for markdown code formatting stay literal instead of being run as commands.
cat <<'TEMPLATE_EOF' > "$TEMPLATE_FILE"
You are re-reviewing one specific pull request on request, triggered manually from the tracker app, not
on the regular timer. No one is watching this run interactively, don't ask questions, make the most
reasonable call on anything ambiguous. Never post anything to GitHub or YouTrack (no comments, no review
submissions, no status changes). This is a read-only, local-report-only workflow.

PR: {{REPO}}#{{NUMBER}}
Local repo path: {{LOCAL_PATH}}
State file: {{BASE_DIR}}/pr-review/state.json
Status file: {{BASE_DIR}}/pr-review/status.json
Report output dir: {{BASE_DIR}}/pr-reviews/

## Step 1 — gather context

```
gh pr view {{NUMBER}} --repo {{REPO}} --json title,body,headRefName,baseRefName,author,additions,deletions,changedFiles,files,commits,headRefOid
gh pr diff {{NUMBER}} --repo {{REPO}}
```

Read `{{BASE_DIR}}/pr-review/report-format.md` now — it has the mandatory rules for verifying
line numbers, building the permalink for every citation, and the exact voice and structure the report
must end up in (same rules the scheduled job follows, kept in one place so the two can't drift apart).
Apply those rules to everything below, including Step 2's report.

### Find the YouTrack ticket(s)

Search for `DEV-\d+` across the PR title, body, branch name, and every commit's headline + body. Call
`mcp__youtrack__get_issue` for each unique ticket ID found. If none found, say so explicitly. If a found
ticket's scope doesn't obviously match the diff, say so explicitly.

### Review the diff
{{LAYERS}}
Do NOT check out the branch, run a build, or run tests. Do not post findings anywhere, only write the
local report. Collapse every layer above into ONE flat list of comments — report-format.md covers the
voice and the exact anchoring.

## Step 2 — write the report

Follow `report-format.md` exactly: the line-number verification, the permalink for every citation, the
voice, and the report's structure are all defined there, not here. Write the file to
`{{BASE_DIR}}/pr-reviews/<repo-short-name>-{{NUMBER}}.md` (overwrite if it already exists —
this is a deliberate re-review, the old one is stale), and its JSON sidecar
(`<repo-short-name>-{{NUMBER}}.json`, same dir) per report-format.md's "Structured sidecar" section —
the tracker app's Publish feature reads that, not the `.md` file.

## Step 3 — update state, record status, notify, and open the report

1. Update state.json: set key `"{{REPO}}#{{NUMBER}}"` to this PR's `headRefOid`.
2. Merge an entry into status.json for key `"{{REPO}}#{{NUMBER}}"` (read the existing object, replace
   just this key, write the whole object back, don't lose other PRs' entries):
   ```json
   {
     "title": "<PR title>",
     "url": "<PR url>",
     "outcome": "approve" | "request-changes" | "needs-discussion",
     "reportPath": "{{BASE_DIR}}/pr-reviews/<repo-short-name>-{{NUMBER}}.md",
     "finishedAt": "<ISO 8601 UTC timestamp, from `date -u +%Y-%m-%dT%H:%M:%SZ`>"
   }
   ```
   Derive `outcome` per report-format.md's "Outcome field" section.
3. Fire a notification and open the report in VS Code:
   ```
   osascript -e 'display notification "<repo-short-name>#{{NUMBER}} re-reviewed" with title "Claude PR Review" sound name "Glass"'
   open "vscode://file/{{BASE_DIR}}/pr-reviews/<repo-short-name>-{{NUMBER}}.md"
   ```
   Never plain `open <path>` — that hands it to the OS's default handler for `.md` files (Xcode here).
   The `vscode://file/<path>` URL opens as a new tab in the VS Code window that's already open, with
   no new window and no extra process spawned.
TEMPLATE_EOF

export RO_REPO="$REPO" RO_NUMBER="$NUMBER" RO_LOCAL_PATH="$LOCAL_PATH" RO_BASE_DIR="$BASE_DIR" RO_LAYERS="$LAYERS" RO_TEMPLATE_FILE="$TEMPLATE_FILE"

PROMPT=$(python3 <<'PYEOF'
import os
with open(os.environ["RO_TEMPLATE_FILE"]) as f:
    text = f.read()
text = text.replace("{{REPO}}", os.environ["RO_REPO"])
text = text.replace("{{NUMBER}}", os.environ["RO_NUMBER"])
text = text.replace("{{LOCAL_PATH}}", os.environ["RO_LOCAL_PATH"])
text = text.replace("{{BASE_DIR}}", os.environ["RO_BASE_DIR"])
text = text.replace("{{LAYERS}}", os.environ["RO_LAYERS"])
print(text)
PYEOF
)

"$CLAUDE_BIN" -p "$PROMPT" \
  --allowedTools "Bash(gh pr view:*)" "Bash(gh pr diff:*)" "Bash(gh api repos/cioplenu/*)" "Bash(osascript:*)" "Bash(open vscode://*)" "Read" "Write" "mcp__youtrack__get_issue" "Skill" \
  --permission-prompts none \
  >> "$LOG_FILE" 2>&1

echo "=== Manual review finished: $(date), $REPO#$NUMBER ===" >> "$LOG_FILE"
