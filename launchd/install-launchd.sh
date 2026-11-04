#!/bin/bash
# Fills in the two plist templates with this machine's actual paths and loads them as login
# agents. Safe to re-run — it unloads an existing copy first, so re-running after an edit
# just reloads with the new version.
#
# Run this AFTER building and installing the tracker app (pr-tracker/package-app.sh) — the
# pr-tracker agent will load either way, but it won't have anything to launch until that
# app exists at ~/Applications/PR Tracker.app.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENTS_DIR="$HOME/Library/LaunchAgents"

mkdir -p "$AGENTS_DIR"

for name in com.ops1.pr-review-poll com.ops1.pr-tracker; do
  template="$SCRIPT_DIR/$name.plist.template"
  dest="$AGENTS_DIR/$name.plist"
  sed -e "s#__BASE_DIR__#$BASE_DIR#g" -e "s#__HOME__#$HOME#g" "$template" > "$dest"
  launchctl unload "$dest" 2>/dev/null || true
  launchctl load "$dest"
  echo "installed and loaded $name -> $dest"
done

echo
echo "com.ops1.pr-review-poll runs every 30 minutes as a fallback (RunAtLoad=false, so it"
echo "won't fire right now) — while PR Tracker.app is open it reviews new/updated PRs within"
echo "about 45 seconds on its own, this is just the backstop for when the app isn't running."
echo "com.ops1.pr-tracker launches now and on every login."
