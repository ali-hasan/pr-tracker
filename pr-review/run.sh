#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"  # this repo's own root (pr-tracker/)
# Launched from the app or launchd we start in `/`. Work from this repo instead, so nothing
# the review does from its working folder wanders into Desktop or Documents and makes macOS
# ask PR Tracker for folder access.
cd "$BASE_DIR"

export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

if [[ ! -f "$SCRIPT_DIR/config.sh" ]]; then
  echo "run.sh: $SCRIPT_DIR/config.sh not found — copy config.sh.example to config.sh and fill in REPOS_DIR" >&2
  exit 1
fi
CLAUDE_BIN=""
REPOS_DIR=""
REVIEW_MODEL=""
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

# Most of these half-hourly runs find nothing new, so the check itself is done here in plain
# shell (the same search and commit comparison as prompt.md's Steps 1–2) and Claude only
# starts when at least one PR is new or has new commits since its last review.
SEARCH_REPO_ARGS=()
for repo in "${REPO_NAMES[@]}"; do
  SEARCH_REPO_ARGS+=(--repo "$repo")
done
if ! OPEN_PRS=$(gh search prs --review-requested @me --state open "${SEARCH_REPO_ARGS[@]}" \
    --json number,repository --jq '.[] | "\(.repository.nameWithOwner) \(.number)"' 2>> "$LOG_FILE"); then
  echo "=== Run finished: $(date), couldn't list PRs from GitHub, trying again next run ===" >> "$LOG_FILE"
  exit 0
fi
NEEDS_REVIEW=()
CHECKED=0
while read -r repo number; do
  [[ -z "$repo" ]] && continue
  CHECKED=$((CHECKED + 1))
  if ! head_sha=$(gh pr view "$number" --repo "$repo" --json headRefOid -q .headRefOid 2>> "$LOG_FILE"); then
    echo "couldn't read the head commit of $repo#$number, checking it again next run" >> "$LOG_FILE"
    continue
  fi
  reviewed_sha=$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))' \
    "$SCRIPT_DIR/state.json" "$repo#$number" 2>/dev/null || true)
  if [[ "$head_sha" != "$reviewed_sha" ]]; then
    NEEDS_REVIEW+=("$repo#$number")
  fi
done <<< "$OPEN_PRS"
if [[ ${#NEEDS_REVIEW[@]} -eq 0 ]]; then
  echo "=== Run finished: $(date), nothing new across $CHECKED open PR(s), Claude not started ===" >> "$LOG_FILE"
  exit 0
fi
echo "New or updated, starting Claude (model=${REVIEW_MODEL:-claude-default}): ${NEEDS_REVIEW[*]}" >> "$LOG_FILE"

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

# Read-only git commands in each checkout, so a review can read files and search code at the
# PR's own commit without touching your working tree.
GIT_READ_TOOLS=()
for dir in "${REPO_LOCAL_DIRS[@]}"; do
  for sub in fetch show grep log; do
    GIT_READ_TOOLS+=("Bash(git -C $REPOS_DIR/$dir $sub:*)")
  done
done

# Reviews may only write their reports and the two state files, nothing else on disk.
WRITE_TOOLS=("Edit(/$BASE_DIR/pr-reviews/**)" "Edit(/$BASE_DIR/pr-review/state.json)" "Edit(/$BASE_DIR/pr-review/status.json)")

MODEL_ARGS=()
if [[ -n "$REVIEW_MODEL" ]]; then
  MODEL_ARGS=(--model "$REVIEW_MODEL")
fi

RUN_STARTED="$(mktemp -t pr-review-run)"
"$CLAUDE_BIN" -p "$PROMPT" ${MODEL_ARGS[@]+"${MODEL_ARGS[@]}"} \
  --allowedTools "Bash(gh search prs:*)" "Bash(gh pr view:*)" "Bash(gh pr diff:*)" "${GH_API_TOOLS[@]}" "${GIT_READ_TOOLS[@]}" "Bash(osascript:*)" "Bash(open vscode://*)" "Read" "Grep" "Glob" "${WRITE_TOOLS[@]}" "mcp__youtrack__get_issue" "Skill" \
  --permission-prompts none \
  >> "$LOG_FILE" 2>&1
CLAUDE_EXIT=$?

# Point each new report's line links into the PR's diff (only reports written by this run).
find "$BASE_DIR/pr-reviews" -name '*.json' -newer "$RUN_STARTED" -print0 2>/dev/null \
  | xargs -0 python3 -I "$SCRIPT_DIR/link-to-pr-diff.py" >> "$LOG_FILE" 2>&1
rm -f "$RUN_STARTED"

echo "=== Run finished: $(date), exit code $CLAUDE_EXIT ===" >> "$LOG_FILE"
