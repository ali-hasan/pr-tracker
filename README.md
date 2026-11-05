# PR Review Automation + Tracker — architecture, and how to run this yourself

This system does three things:

1. **Reviews PRs for you automatically** — within seconds of being requested as a reviewer (or a
   PR you're already reviewing getting new commits), it reads the diff and the linked YouTrack
   ticket, runs a 3-layer review, and writes a short local report. It never posts anything to
   GitHub or YouTrack on its own.
2. **Tracks it all in a small menu-bar-style app** — a native Rust/egui app showing what's
   assigned, what's being reviewed right now, and the outcome of what's done — with buttons to
   re-run a review with different settings, review any PR by pasting its URL (not just ones
   assigned to you), or review a local git branch that has no PR yet.
3. **Lets you publish, when you choose to** — a Publish button turns a review's comments into a
   real pending (draft) GitHub review, placed inline on the right lines, which you then edit and
   submit yourself on GitHub. Nothing is ever posted without you clicking that button.

Nothing here requires a webhook or a server. It's a local polling loop plus a handful of plain
JSON files that the automation and the tracker app both read.

## 1. The moving parts

```
PR Tracker.app (launches at login, polls every 45s)
  ├─ gh search prs --review-requested @me --state open      (what's assigned, for the UI)
  ├─ diffs each PR's live head SHA against state.json        ──┐
  │  → new/changed and nothing else running? spawn:            │ this is what makes reviews
  │                                                              start within ~45s instead of
  └─ review-one.sh  /  review-local.sh  /  (from the UI)        waiting for a timer
       └─ claude -p "<prompt.md or a built one-off prompt>" --permission-prompts none
            ├─ gh pr view / gh pr diff  (or `git diff` for a local branch)
            ├─ mcp__youtrack__get_issue                      (ticket lookup)
            ├─ repo's own SKILL.md                           (layer 1)
            ├─ Skill: code-review                            (layer 2)
            ├─ review-criteria.md                            (layer 3)
            ├─ Skill: my-voice                                (writing style — see §2)
            └─ writes pr-reviews/<repo>-<n>.md + a .json sidecar
                 + updates state.json, status.json
                 + osascript notification + opens the report (vscode://file/... URL)

launchd (every 30 min, independent of the app)
  └─ run.sh — same discovery+review logic as above, as a fallback for whenever the
     tracker app itself isn't running to catch things within 45 seconds.

Publish… button (tracker app, user-initiated only)
  └─ gh api POST repos/<owner>/<repo>/pulls/<number>/reviews   (no `event` → stays a draft)
```

This whole repo is self-contained — it doesn't need to sit next to `api`/`frontend` or anywhere in
particular. All it needs is to be told, once, where those checkouts actually live (`REPOS_DIR`, see
§4) — they can be anywhere on disk, in a completely different parent folder from this repo.

All state lives in plain JSON files under `pr-review/` — no database, no server.

| File | Written by | Read by | Purpose |
|---|---|---|---|
| `prompt.md` | you (hand-edited) | `run.sh` | the scheduled-run instruction set |
| `report-format.md` | you (hand-edited) | every review | shared rules: line verification, link format, voice, report structure, JSON sidecar shape |
| `review-criteria.md` | you (hand-edited) | every review | layer 3, your own standing rules — empty by default |
| `config.sh` | you, from `config.sh.example` | `run.sh`, `review-one.sh`, `review-local.sh` | `REPOS_DIR` (required) and an optional `CLAUDE_BIN` override — **not committed** |
| `repos.conf` | you, from `repos.conf.example` | the Rust app, `repos.sh` | which repos to review and their local folder name — **not committed** |
| `repos.sh` | tracked | `run.sh`, `review-one.sh`, `review-local.sh` | shared loader for `repos.conf` |
| `run.sh` | tracked | `launchd` | wraps the scheduled `claude -p` call, logs, concurrency guard |
| `review-one.sh` | tracked | tracker app | one-off re-review of a specific PR with UI-selected options |
| `review-local.sh` | tracked | tracker app | reviews a local git branch against its base branch, no PR needed |
| `state.json` | automation | automation, tracker (Hide) | `{"owner/repo#N": "<sha last reviewed>"}` — dedup, PRs only |
| `status.json` | automation | tracker app | `{"owner/repo#N" or "owner/repo@branch": {title, url, outcome, reportPath, finishedAt}}` |
| `dismissed.json` | tracker app | tracker app | `{"key": "<sha hidden at>"}` — manual Hide, self-expiring for PRs |
| `RUNNING` | `run.sh` / `review-one.sh` / `review-local.sh` | tracker app | marker file; its content (if any) is the key being reviewed |
| `pr-reviews/<repo>-<n or branch>.md` | automation | you, opened in VS Code | the actual review report |
| `pr-reviews/<repo>-<n or branch>.json` | automation | tracker app's Publish panel | same findings, structured — never read by a human |

## 2. The skills, and why grepping the repo won't find them

`prompt.md` references three different things it calls "skills," and they live in three very
different places:

### Layer 1 — the repo's own rubric (**is** in the repo, shared via git)

```
api/.github/skills/code-review/SKILL.md
frontend/.github/skills/code-review/SKILL.md
```

Written for GitHub Copilot's PR review feature originally, and reusable here. Defines what counts
as auto-approvable (pure refactors, docs-only, lockfile bumps...) and what always needs a human (DB
migrations, new dependencies, telemetry changes...). Anyone with repo access already has these.

### Layer 2 — the built-in `code-review` skill (**not** in any repo — a Claude Code plugin)

Invoked as `Skill` tool, skill name `code-review`, args `low <PR URL>` (or, for a local branch, a
description of the diff already gathered instead of a URL). This resolves to an org-provisioned
plugin bundle (`~/.claude/plugins/synced/...`) tied to whichever Claude/claude.ai workspace this
machine is logged into — not something installed by a command in this repo. Runs a structured
multi-angle review (security, performance, correctness, maintainability) at a chosen effort level;
always pinned to `low` for unattended runs, since higher levels spin up many more reviewer
subagents per PR and once made a single run take over an hour.

**Practical effect:** anyone logged into Claude Code under the same org workspace should already
have this without installing anything. If someone's `Skill` tool can't find `code-review` at all,
that's an org-plugin-provisioning question, not something fixable by copying a file.

### Layer 3 — `review-criteria.md` (just a text file, not a real "skill")

Currently empty. Add bullets here any time you want a standing instruction applied to every review
without touching `prompt.md` itself.

### Writing style — `my-voice` (**personal — do not copy this file for someone else**)

```
~/.claude/skills/my-voice/SKILL.md
```

Lives in your home directory, not any repo. It's not a generic "sound professional" skill — it's
modeled on one specific person's actual writing habits (lowercase "i", comma splices, backticks
instead of quotes, specific approve/request-changes closing lines). `report-format.md` loads it so
every report reads like a comment that person would type themselves.

**If someone else is setting this up for themselves, they should either write their own version of
this skill (same idea — model it on how *they* actually write review comments) or delete the
"load my-voice" instruction from `report-format.md`'s Voice section and let reports come out in a
plainer, more generic register instead.** Either way, a first run without any `my-voice` skill at
all doesn't fail — the model just writes in its own default voice, which is a safe fallback but
worth knowing about going in.

## 3. Prerequisites (per person, not part of this repo)

- **macOS.** `osascript`, the `open`/`vscode://` URL handling, and the `.app`/`launchd` packaging
  are all macOS-specific.
- **Claude Code**, authenticated, logged into whichever org/workspace provisions the `code-review`
  plugin skill (ask around if you're not sure which one that is).
- **`gh` CLI**, authenticated (`gh auth status`) with access to the repos you want reviewed. No
  extra scope is needed for Publish — review creation uses whatever `gh` already has. Both the
  tracker app and the shell scripts auto-detect `gh` and `claude` on their own (common install
  locations, then your login shell's PATH as a fallback) — if both are already set up and
  authenticated the normal way, there's nothing to point at them explicitly.
- **YouTrack MCP connector** (or whatever ticket tracker's MCP server) connected in Claude Code,
  for the ticket-lookup step — if you don't use YouTrack, that one step in `prompt.md` /
  `review-one.sh` / `review-local.sh` just needs deleting.
- **Rust toolchain** (`rustup`/`cargo`) to build the tracker app.
- **VS Code**, installed as `/Applications/Visual Studio Code.app` — Report links open via a
  `vscode://file/...` URL, which that app registers as a URL handler on install.

## 4. Setting this up from a fresh clone

This repo can be cloned anywhere — `~/code/pr-tracker`, `~/tools/pr-tracker`, wherever. It has no
fixed relationship to where you keep your actual `api`/`frontend` checkouts; you tell it that
location explicitly in step 2 below.

1. Clone this repo.
2. ```
   cd pr-review
   cp config.sh.example config.sh
   cp repos.conf.example repos.conf
   ```
   Edit `config.sh`: set `REPOS_DIR` to the absolute path of the directory containing your repo
   checkouts — e.g. if you have `~/workspace/ops1/api` and `~/workspace/ops1/frontend`, that's
   `~/workspace/ops1`, regardless of where you cloned *this* repo. There's no way to infer this
   one — it has to be set. Leave `CLAUDE_BIN` blank unless auto-detection can't find yours (see §3).

   Edit `repos.conf` to list the repos you want reviewed, one per line as
   `<owner/repo>:<local-folder-name>` — the folder name is relative to `REPOS_DIR` and doesn't have
   to match the GitHub repo name. Any number of repos, across any number of orgs. Changing this
   file later (adding, removing, renaming) takes effect on the next poll or click — no rebuild, no
   restart, and nothing else to edit.
3. Decide what to do about `my-voice` — see §2. If you're skipping it, remove the "load the
   `my-voice` skill" line from `report-format.md`'s Voice section.
4. If you use a different ticket tracker than YouTrack, remove the `mcp__youtrack__get_issue`
   step from `prompt.md`, `review-one.sh`, and `review-local.sh`, and the `mcp__youtrack__get_issue`
   entry from each script's `--allowedTools` list.
5. Test the pipeline manually before trusting it unattended:
   ```
   bash pr-review/run.sh
   tail -f pr-review/run.log
   ```
   Confirm it either finds something to review and writes a report, or correctly says there's
   nothing new — before relying on the scheduled job or the tracker app's auto-trigger.
6. Build and install the tracker app:
   ```
   cp .cargo/config.toml.example .cargo/config.toml
   ```
   Edit `.cargo/config.toml`: set `PR_TRACKER_BASE_DIR` to *this repo's own* absolute root path
   (not `REPOS_DIR` — the app never touches `api`/`frontend` directly, only the scripts above do).
   This is baked into the binary at compile time — if it ever changes, edit this file and rebuild,
   there's no runtime config to update instead. Leave `PR_TRACKER_GH_PATH` commented out unless
   auto-detection can't find your `gh` (see §3). Then:
   ```
   ./package-app.sh
   ```
   This builds the release binary and installs it to `~/Applications/PR Tracker.app`. Launch it
   once manually (`open -a "PR Tracker.app"`) and confirm it shows your assigned PRs before
   wiring it into `launchd`.
7. Install both `launchd` agents (the scheduled 30-minute fallback, and the tracker app's
   login-launch):
   ```
   cd launchd
   ./install-launchd.sh
   ```
   This fills in your actual paths from the templates and runs `launchctl load` for both. Safe to
   re-run any time (e.g. after moving the repo) — it unloads the previous copy first.
8. From here on: the tracker app polls every 45 seconds and starts reviewing newly-assigned or
   newly-updated PRs on its own, with no further action from you. The 30-minute `launchd` job is
   purely a fallback for whenever the tracker app isn't running.

## 5. What never gets committed (see `.gitignore`)

- `pr-review/config.sh` / `pr-review/repos.conf` / `.cargo/config.toml` — per-machine paths and
  which repos you review, not secrets, but no reason to commit someone else's.
- `pr-review/state.json`, `status.json`, `dismissed.json`, `RUNNING`, `run.log*`, `launchd.log` —
  this machine's own review history and locks, regenerated from scratch by the pipeline.
- `pr-reviews/` — the actual review reports. Proprietary PR content, and stale the moment anyone
  else runs this.
- `target/`, `tracker.log` — build output and app logs.
- The `~/.claude/skills/my-voice/SKILL.md` personal skill isn't part of this repo at all (it lives
  in your home directory) — see §2 for why it should stay that way.

Nothing in this repo ever held an actual API token or credential — `gh` and Claude Code both
handle their own auth outside of it (`gh auth login`, Claude Code's own login), and the YouTrack
MCP connection is configured in Claude Code's own settings, not a file here.
