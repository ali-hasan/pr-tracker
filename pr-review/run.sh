#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"  # this repo's own root (pr-tracker/)

if [[ ! -f "$SCRIPT_DIR/config.sh" ]]; then
  echo "run.sh: $SCRIPT_DIR/config.sh not found — copy config.sh.example to config.sh and fill in CLAUDE_BIN + REPOS_DIR" >&2
  exit 1
fi
CLAUDE_BIN=""
REPOS_DIR=""
source "$SCRIPT_DIR/config.sh"
if [[ -z "$CLAUDE_BIN" || -z "$REPOS_DIR" ]]; then
  echo "run.sh: CLAUDE_BIN and REPOS_DIR must both be set in $SCRIPT_DIR/config.sh" >&2
  exit 1
fi

PROMPT_FILE="$SCRIPT_DIR/prompt.md"
LOG_FILE="$SCRIPT_DIR/run.log"

export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

if [[ -f "$SCRIPT_DIR/RUNNING" ]]; then
  echo "=== Run skipped: $(date), a review (manual or scheduled) is already in progress ===" >> "$LOG_FILE"
  exit 0
fi

echo "=== Run started: $(date) ===" >> "$LOG_FILE"

touch "$SCRIPT_DIR/RUNNING"
# A plain trailing `rm -f` only runs if claude exits normally — if it's killed or crashes
# partway through, the lock is left behind and every following scheduled run skips itself
# forever. This happened once already. A trap fires on any exit, crash included.
trap 'rm -f "$SCRIPT_DIR/RUNNING"' EXIT

# prompt.md is plain text passed as a prompt string, not a shell script — its {{BASE_DIR}}
# and {{REPOS_DIR}} placeholders need substituting here, the same way review-one.sh/
# review-local.sh already template their own prompts.
export RUN_BASE_DIR="$BASE_DIR" RUN_REPOS_DIR="$REPOS_DIR" RUN_PROMPT_FILE="$PROMPT_FILE"
PROMPT=$(python3 <<'PYEOF'
import os
with open(os.environ["RUN_PROMPT_FILE"]) as f:
    text = f.read()
text = text.replace("{{BASE_DIR}}", os.environ["RUN_BASE_DIR"])
text = text.replace("{{REPOS_DIR}}", os.environ["RUN_REPOS_DIR"])
print(text)
PYEOF
)

"$CLAUDE_BIN" -p "$PROMPT" \
  --allowedTools "Bash(gh search prs:*)" "Bash(gh pr view:*)" "Bash(gh pr diff:*)" "Bash(gh api repos/cioplenu/*)" "Bash(osascript:*)" "Bash(open vscode://*)" "Read" "Write" "mcp__youtrack__get_issue" "Skill" \
  --permission-prompts none \
  >> "$LOG_FILE" 2>&1

echo "=== Run finished: $(date), exit code $? ===" >> "$LOG_FILE"
