# Shared repo-config loader — sourced (not executed) by run.sh, review-one.sh, and
# review-local.sh. Reads repos.conf into two parallel arrays so a repo can be added,
# removed, or pointed at a different local folder by editing that one file — no script
# edits, no rebuild. Requires $SCRIPT_DIR already set by the caller.
#
# Plain indexed arrays, not `declare -A`: macOS ships bash 3.2, which has no
# associative arrays.

REPOS_CONF="$SCRIPT_DIR/repos.conf"
if [[ ! -f "$REPOS_CONF" ]]; then
  echo "$(basename "$0"): $REPOS_CONF not found — copy repos.conf.example to repos.conf and list the repos you want reviewed" >&2
  exit 1
fi

REPO_NAMES=()
REPO_LOCAL_DIRS=()
while IFS=: read -r conf_repo conf_dir; do
  conf_repo="$(echo "$conf_repo" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  [[ -z "$conf_repo" || "$conf_repo" == \#* ]] && continue
  conf_dir="$(echo "$conf_dir" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  REPO_NAMES+=("$conf_repo")
  REPO_LOCAL_DIRS+=("$conf_dir")
done < "$REPOS_CONF"

if [[ ${#REPO_NAMES[@]} -eq 0 ]]; then
  echo "$(basename "$0"): $REPOS_CONF has no repos listed" >&2
  exit 1
fi

# Looks up $1 (an "owner/repo" string) against REPO_NAMES and sets LOCAL_PATH to
# "$REPOS_DIR/<its configured local dir>". Returns 1 (LOCAL_PATH left unset) if $1
# isn't listed in repos.conf — the caller decides how to log/report that.
local_path_for_repo() {
  local target="$1" i
  for i in "${!REPO_NAMES[@]}"; do
    if [[ "${REPO_NAMES[$i]}" == "$target" ]]; then
      LOCAL_PATH="$REPOS_DIR/${REPO_LOCAL_DIRS[$i]}"
      return 0
    fi
  done
  return 1
}

# Space-separated list of distinct orgs (the part before "/") across every repo in
# repos.conf, for scoping `gh api repos/<org>/*` in --allowedTools without granting
# every org on GitHub.
repo_orgs() {
  local org seen=()
  for r in "${REPO_NAMES[@]}"; do
    org="${r%%/*}"
    local already=false
    for s in ${seen[@]+"${seen[@]}"}; do
      [[ "$s" == "$org" ]] && already=true && break
    done
    $already || seen+=("$org")
  done
  echo "${seen[@]}"
}
