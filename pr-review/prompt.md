You are running unattended, on a timer. No one is watching this run. Do not ask questions —
if something is ambiguous, make the most reasonable call, note the ambiguity in the report, and
continue. Never post anything to GitHub or YouTrack (no comments, no review submissions, no
status changes). This is a read-only, local-report-only workflow.

## Repos to watch

| GitHub repo | Local path |
|---|---|
| cioplenu/backend | {{REPOS_DIR}}/api |
| cioplenu/frontend | {{REPOS_DIR}}/frontend |

State file: {{BASE_DIR}}/pr-review/state.json
Status file (for the tracker app, see Step 5): {{BASE_DIR}}/pr-review/status.json
Report output dir: {{BASE_DIR}}/pr-reviews/
Extra criteria file: {{BASE_DIR}}/pr-review/review-criteria.md
Report format rules (read this before Step 4, follow it exactly): {{BASE_DIR}}/pr-review/report-format.md

## Step 1 — find PRs that need a review from me

Run exactly:

```
gh search prs --review-requested @me --state open --repo cioplenu/backend --repo cioplenu/frontend --json number,title,url,repository
```

This returns PRs where I am requested as a reviewer — NOT PRs I authored. Do not use `--assignee`,
it returns the wrong set (PRs I wrote).

## Step 2 — dedupe against state

Read state.json (a JSON object mapping `"<owner/repo>#<number>"` to the last-reviewed commit SHA).
For each PR from step 1, run:

```
gh pr view <number> --repo <owner/repo> --json headRefOid
```

- If the key is missing from state.json, or the SHA differs from the stored one → it's new/updated, review it.
- If the key exists with the same SHA → already reviewed this exact commit, skip it silently.

## Step 3 — for each new/updated PR, gather context

```
gh pr view <number> --repo <owner/repo> --json title,body,headRefName,baseRefName,author,additions,deletions,changedFiles,files,commits
gh pr diff <number> --repo <owner/repo>
```

Read `{{BASE_DIR}}/pr-review/report-format.md` now — it has the
mandatory rules for verifying line numbers, building the permalink for every citation, and the exact
voice and structure the report must end up in. Apply those rules to everything below.

### Find the YouTrack ticket(s)

Search for `DEV-\d+` patterns across ALL of: the PR title, the PR body, the branch name (`headRefName`),
and every commit's headline + body. Collect every unique ticket ID found — a PR can reference more than
one, and the right one is sometimes only in a commit message, not the PR description (this has happened
before: a PR's actual ticket was absent from the title/body and only present in one commit).

For each ticket ID found, call `mcp__youtrack__get_issue` with that ID.

- If no ticket ID is found anywhere: say so explicitly in the report ("No YouTrack ticket reference found").
- If a ticket is found but its description/summary doesn't obviously match what the PR's diff does
  (e.g. the ticket describes a narrower or different scope than the PR), say so explicitly — do not
  silently assume it's the right ticket.

### Review the diff — three layers feeding one list of comments, never posted anywhere

**Layer 1 — repo approval-readiness rubric.** Read and apply:
- cioplenu/backend → {{REPOS_DIR}}/api/.github/skills/code-review/SKILL.md
- cioplenu/frontend → {{REPOS_DIR}}/frontend/.github/skills/code-review/SKILL.md

This decides auto-approvable vs. needs-human-review per that repo's own stated categories. Keep the
reasoning for this to one short line — name the rule, don't quote or restate it at length.

**Layer 2 — the `code-review` skill.** Invoke it (`Skill` tool, skill name `code-review`) with args
`low <PR URL>` — ALWAYS pass `low` explicitly as the effort level. Do not omit it and do not use a
higher level: this runs unattended on a timer, and higher effort levels spin up many more reviewer
subagents per PR, which has caused a single run to take over an hour. `low` still produces real,
specific findings at a fraction of the cost.

**Layer 3 — your own criteria.** Read {{BASE_DIR}}/pr-review/review-criteria.md
and apply anything written there on top of the two layers above (it may be empty — skip if so).

Do NOT check out the branch, run a build, or run tests. This is a fetch-only review: read the diff,
the ticket, and the repo's stated conventions, and reason about correctness, scope, and the repo's
own approval-readiness rules. Do not post findings anywhere — only write the local report.

## Step 4 — write the report

Follow `report-format.md` exactly: the line-number verification, the permalink for every citation, the
voice, and the report's structure are all defined there, not here. Write the file to
`{{BASE_DIR}}/pr-reviews/<repo-short-name>-<number>.md` (e.g.
`frontend-1377.md`), and its JSON sidecar (`<repo-short-name>-<number>.json`, same dir) per
`report-format.md`'s "Structured sidecar" section — the tracker app's Publish feature reads that, not
the `.md` file.

## Step 5 — update state, record status, notify, and open the report

After writing the report, do all four of these for that PR:

1. Update state.json with the new SHA for that PR key (as before).
2. Merge an entry into status.json (read the existing object first, add/replace this PR's key, write
   the whole object back — don't lose other PRs' entries). Key it the same way as state.json
   (`"<owner/repo>#<number>"`). Each entry:
   ```json
   {
     "title": "<PR title>",
     "url": "<PR url>",
     "outcome": "approve" | "request-changes" | "needs-discussion",
     "reportPath": "{{BASE_DIR}}/pr-reviews/<repo-short-name>-<number>.md",
     "finishedAt": "<ISO 8601 timestamp, from `date -u +%Y-%m-%dT%H:%M:%SZ`>"
   }
   ```
   Derive `outcome` per `report-format.md`'s "Outcome field" section.
3. Fire a notification and open the report in VS Code, same as before:
   ```
   osascript -e 'display notification "<repo-short-name>#<number>: <title>" with title "Claude PR Review" sound name "Glass"'
   open "vscode://file{{BASE_DIR}}/pr-reviews/<repo-short-name>-<number>.md"
   ```
   Never plain `open <path>` — that hands it to the OS's default handler for `.md` files, which is
   Xcode on this machine. The `vscode://file/<path>` URL is handled directly by the VS Code window
   that's already open, as a new tab there — no new window, no extra process spawned.

## Step 6 — if nothing new

If step 1 returns PRs but all are already up to date in state.json, do nothing (no report, no
notification). Just exit.
