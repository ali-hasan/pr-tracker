"""Point a report's line links at the PR's "Files changed" tab instead of the plain file view.

Usage: python3 -I link-to-pr-diff.py <report sidecar .json> [...]

For each sidecar this reads the PR's diff and rewrites every blob permalink in the matching .md
report whose line is shown in the diff, so clicking it opens the change itself with the line
highlighted. Lines the diff doesn't show keep their file link, since the PR view can't display
them. Local-branch reviews have no PR and are left alone.
"""

import hashlib
import json
import os
import re
import subprocess
import sys
from urllib.parse import unquote

HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def lines_shown_in_diff(repo, number):
    """Map each changed file to the new-side line ranges its diff hunks show."""
    diff = subprocess.run(
        ["gh", "pr", "diff", str(number), "--repo", repo],
        capture_output=True, text=True, check=True,
    ).stdout
    shown, path = {}, None
    for line in diff.splitlines():
        if line.startswith("+++ "):
            # A deleted file shows up as "+++ /dev/null" and has no new-side lines to link to.
            path = line[6:] if line.startswith("+++ b/") else None
        elif path and (match := HUNK.match(line)):
            start, length = int(match.group(1)), int(match.group(2) or 1)
            shown.setdefault(path, []).append((start, start + length - 1))
    return shown


def rewrite_report(sidecar_path):
    with open(sidecar_path) as f:
        sidecar = json.load(f)
    repo, number = sidecar.get("repo"), sidecar.get("number")
    report_path = os.path.splitext(sidecar_path)[0] + ".md"
    if not repo or not isinstance(number, int) or not os.path.exists(report_path):
        return 0

    shown = lines_shown_in_diff(repo, number)
    blob_link = re.compile(
        r"https://github\.com/" + re.escape(repo) + r"/blob/[0-9a-f]{7,40}/([^)#\s]+)#L(\d+)(?:-L\d+)?"
    )

    changed = 0

    def to_pr_link(match):
        nonlocal changed
        path, line = unquote(match.group(1)), int(match.group(2))
        if not any(start <= line <= end for start, end in shown.get(path, [])):
            return match.group(0)
        changed += 1
        file_anchor = hashlib.sha256(path.encode()).hexdigest()
        return f"https://github.com/{repo}/pull/{number}/files#diff-{file_anchor}R{line}"

    with open(report_path) as f:
        original = f.read()
    rewritten = blob_link.sub(to_pr_link, original)
    if rewritten != original:
        tmp = report_path + ".tmp"
        with open(tmp, "w") as f:
            f.write(rewritten)
        os.replace(tmp, report_path)
    return changed


def main():
    for sidecar_path in sys.argv[1:]:
        try:
            changed = rewrite_report(sidecar_path)
            print(f"link-to-pr-diff: {os.path.basename(sidecar_path)}: {changed} link(s) now open in the PR")
        except Exception as error:  # A failed rewrite must never fail the review itself.
            print(f"link-to-pr-diff: {os.path.basename(sidecar_path)}: skipped ({error})")


if __name__ == "__main__":
    main()
