#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"  # this repo's own root (pr-tracker/)

export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

if [[ ! -f "$SCRIPT_DIR/config.sh" ]]; then
  echo "run.sh: $SCRIPT_DIR/config.sh not found — copy config.sh.example to config.sh and fill in REPOS_DIR" >&2
  exit 1
fi
CLAUDE_BIN=""
REPOS_DIR=""
source "$SCRIPT_DIR/config.sh"
if [[ -z "$REPOS_DIR" ]]; then
  echo "run.sh: REPOS_DIR must be set in $SCRIPT_DIR/config.sh" >&2
  exit 1
fi
if [[ -z "$CLAUDE_BIN" ]]; then
  CLAUDE_BIN="$(command -v claude || true)"
fi
if [[ -z "$CLAUDE_BIN" ]]; then
  echo "run.sh: couldn't find claude on PATH — set CLAUDE_BIN explicitly in $SCRIPT_DIR/config.sh" >&2
  exit 1
fi

source "$SCRIPT_DIR/repos.sh"

PROMPT_FILE="$SCRIPT_DIR/prompt.md"
LOG_FILE="$SCRIPT_DIR/run.log"

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

# Built from repos.conf (via repos.sh) rather than hardcoded, so adding/removing a
# reviewed repo there takes effect on the very next scheduled run.
REPO_TABLE=""
REPO_SEARCH_ARGS=""
REPO_SKILL_LIST=""
for i in "${!REPO_NAMES[@]}"; do
  repo="${REPO_NAMES[$i]}"
  local_path="$REPOS_DIR/${REPO_LOCAL_DIRS[$i]}"
  REPO_TABLE+="| $repo | $local_path |"$'\n'
  REPO_SEARCH_ARGS+="--repo $repo "
  REPO_SKILL_LIST+="- $repo → $local_path/.github/skills/code-review/SKILL.md"$'\n'
done

# prompt.md is plain text passed as a prompt string, not a shell script — its
# placeholders need substituting here, the same way review-one.sh/review-local.sh
# already template their own prompts.
export RUN_BASE_DIR="$BASE_DIR" RUN_REPOS_DIR="$REPOS_DIR" RUN_PROMPT_FILE="$PROMPT_FILE" \
  RUN_REPO_TABLE="$REPO_TABLE" RUN_REPO_SEARCH_ARGS="$REPO_SEARCH_ARGS" RUN_REPO_SKILL_LIST="$REPO_SKILL_LIST"
PROMPT=$(python3 <<'PYEOF'
import os
with open(os.environ["RUN_PROMPT_FILE"]) as f:
    text = f.read()
text = text.replace("{{BASE_DIR}}", os.environ["RUN_BASE_DIR"])
text = text.replace("{{REPOS_DIR}}", os.environ["RUN_REPOS_DIR"])
text = text.replace("{{REPO_TABLE}}", os.environ["RUN_REPO_TABLE"].rstrip("\n"))
text = text.replace("{{REPO_SEARCH_ARGS}}", os.environ["RUN_REPO_SEARCH_ARGS"].rstrip())
text = text.replace("{{REPO_SKILL_LIST}}", os.environ["RUN_REPO_SKILL_LIST"].rstrip("\n"))
print(text)
PYEOF
)

GH_API_TOOLS=()
for org in $(repo_orgs); do
  GH_API_TOOLS+=("Bash(gh api repos/$org/*)")
done

"$CLAUDE_BIN" -p "$PROMPT" \
  --allowedTools "Bash(gh search prs:*)" "Bash(gh pr view:*)" "Bash(gh pr diff:*)" "${GH_API_TOOLS[@]}" "Bash(osascript:*)" "Bash(open vscode://*)" "Read" "Write" "mcp__youtrack__get_issue" "Skill" \
  --permission-prompts none \
  >> "$LOG_FILE" 2>&1

echo "=== Run finished: $(date), exit code $? ===" >> "$LOG_FILE"
