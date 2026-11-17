# PR Tracker

A local PR review assistant for macOS. When you're requested as a reviewer on GitHub, it reviews
the PR with Claude Code and writes a short report on your machine. A small native app shows what's
assigned and what's been reviewed, and lets you turn a review into a draft GitHub review when you
choose to.

- **Automatic reviews.** New PRs and new commits on PRs you're reviewing are picked up within about
  45 seconds. Each review reads the diff and the linked ticket and writes a report to `pr-reviews/`.
- **Tracker app.** Shows assigned PRs and review outcomes. You can re-run a review with other
  options, review any PR by URL, or review a local branch that has no PR yet.
- **Nothing is posted on its own.** The **Publish…** button creates a *pending* GitHub review with
  the comments you pick. You edit and submit it on GitHub yourself.

No server or webhook is involved: a polling loop, a few shell scripts and some JSON files.

## How it works

```
PR Tracker.app            polls GitHub every 45s, starts a review for anything new or updated
launchd, every 30 min     run.sh, a fallback for when the app isn't running
                          (checks with plain `gh` first, only starts Claude if there's work)

each review:  claude -p … --permission-prompts none
              ├─ gh pr view / gh pr diff (or git diff for a local branch)
              ├─ ticket lookup via the YouTrack MCP connector
              ├─ layer 1: <your repo>/.github/skills/code-review/SKILL.md
              ├─ layer 2: the `code-review` Claude Code skill
              ├─ layer 3: pr-review/review-criteria.md
              └─ writes pr-reviews/<repo>-<n>.md and a .json sidecar, then notifies you
              then link-to-pr-diff.py points the report's line links into the PR's diff

Publish…      gh api POST repos/<owner>/<repo>/pulls/<n>/reviews   (no event, so it stays a draft)
```

Reviews run with a fixed `--allowedTools` list. Only one review runs at a time, and reports for
merged or closed PRs are cleaned up automatically.

## Requirements

- macOS, with VS Code installed (report links open via `vscode://file/...`)
- [Claude Code](https://claude.com/claude-code), logged in to a workspace that provides the
  `code-review` plugin skill (layer 2). It isn't installed from this repo.
- [`gh`](https://cli.github.com), logged in with access to the repos you want reviewed
- The YouTrack MCP connector in Claude Code, for ticket lookups (optional, see below)
- Rust (`rustup`) to build the app

`gh` and `claude` are found automatically (common install paths, then your login shell's `PATH`).
No tokens are stored in this repo; both tools use their own logins.

## Setup

1. Clone the repo anywhere, then create your local config:
   ```bash
   cd pr-review
   cp config.sh.example config.sh
   cp repos.conf.example repos.conf
   ```
2. In `config.sh`, set `REPOS_DIR` to the folder that holds your repo checkouts (not this repo).
3. In `repos.conf`, list the repos to review, one per line as `owner/repo:local-folder`. The folder
   is relative to `REPOS_DIR` and doesn't need to match the GitHub name. Changes apply on the next
   poll, no rebuild needed.
4. Run one review by hand to check everything works:
   ```bash
   bash pr-review/run.sh && tail pr-review/run.log
   ```
5. Build and install the app:
   ```bash
   cp .cargo/config.toml.example .cargo/config.toml   # set PR_TRACKER_BASE_DIR to this repo's path
   ./package-app.sh                                   # installs ~/Applications/PR Tracker.app
   ```
   The path is compiled in, so rebuild if you move the repo.
6. Start the app at login and enable the 30-minute fallback:
   ```bash
   launchd/install-launchd.sh
   ```
   Safe to re-run, for example after moving the repo.

## Customising

- **Model.** `REVIEW_MODEL` in `config.sh` (`opus`, `sonnet`, `fable` or a full model name) is used
  for every automatic review. The **Default model** dropdown in the app edits the same line. The
  **Review…** panel can pick a different model for a single run. Leave it empty to use your Claude
  Code default. Each run logs its model in `pr-review/run.log`.
- **Your own rules.** Add bullets to `pr-review/review-criteria.md` and every review applies them.
- **Writing style.** `report-format.md` loads a personal `my-voice` skill from
  `~/.claude/skills/`. Write your own, or delete that line to get Claude's default voice. A missing
  skill doesn't break anything.
- **No YouTrack?** Remove the `mcp__youtrack__get_issue` step and its `--allowedTools` entry from
  `prompt.md`, `review-one.sh` and `review-local.sh`.
- **Repo rubric.** Layer 1 reads `.github/skills/code-review/SKILL.md` from each reviewed repo's
  local checkout. It decides whether a PR is safe to approve quickly or needs a closer look.

## Files

| File | Purpose |
|---|---|
| `pr-review/prompt.md` | Instructions for the scheduled run |
| `pr-review/report-format.md` | Report structure, link format, voice, JSON sidecar shape |
| `pr-review/review-criteria.md` | Your standing review rules (empty by default) |
| `pr-review/run.sh` | Scheduled run: checks for new work, then starts Claude |
| `pr-review/review-one.sh` | One PR, with options chosen in the app |
| `pr-review/review-local.sh` | A local branch against its base branch |
| `pr-review/repos.sh` | Reads `repos.conf` for the scripts |
| `pr-review/link-to-pr-diff.py` | Makes a report's line links open in the PR's "Files changed" tab |
| `src/main.rs` | The tracker app (Rust, egui) |

Created at runtime and gitignored: `config.sh`, `repos.conf`, `.cargo/config.toml`, the reports in
`pr-reviews/`, logs, and these state files in `pr-review/`:

| File | Contents |
|---|---|
| `state.json` | Last reviewed commit per PR, so nothing is reviewed twice |
| `status.json` | Outcome and report path per PR or branch, shown in the app |
| `dismissed.json` | Rows you removed in the app; a PR comes back if it gets new commits |
| `RUNNING` | Lock file while a review is in progress |
