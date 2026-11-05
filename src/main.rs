use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Both of these are baked in at compile time from pr-tracker/.cargo/config.toml (gitignored —
// copy config.toml.example and fill in your own paths before building). A fresh clone with no
// config.toml fails to compile with the message below instead of silently pointing at someone
// else's home directory. BASE_DIR itself is only read here to force that check — everywhere
// else re-reads the same env var through `concat!`, since `concat!` can't take a `const`.
#[allow(dead_code)]
const BASE_DIR: &str = env!(
    "PR_TRACKER_BASE_DIR",
    "copy pr-tracker/.cargo/config.toml.example to .cargo/config.toml and set PR_TRACKER_BASE_DIR"
);
// Unlike BASE_DIR, this is optional — most machines already have `gh` installed
// somewhere `gh_path()` below finds on its own, so there's nothing to configure.
// Set PR_TRACKER_GH_PATH in .cargo/config.toml only to pin a specific install.
const GH_PATH_OVERRIDE: Option<&str> = option_env!("PR_TRACKER_GH_PATH");
const STATUS_PATH: &str = concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/status.json");
const STATE_PATH: &str = concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/state.json");
const DISMISSED_PATH: &str = concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/dismissed.json");
const RUNNING_PATH: &str = concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/RUNNING");
const OPEN_PATH: &str = "/usr/bin/open";
const BASH_PATH: &str = "/bin/bash";
const REVIEW_ONE_SCRIPT: &str = concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/review-one.sh");
const REVIEW_LOCAL_SCRIPT: &str =
    concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/review-local.sh");
const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Wide enough that the table's columns never need a manual drag to appear.
const BASE_WINDOW_SIZE: [f32; 2] = [980.0, 480.0];
/// Added to the window height while a row's review-options panel is open, so
/// it's never hidden below the fold and the window grows back down after.
const REVIEW_PANEL_EXTRA_HEIGHT: f32 = 260.0;
/// Same idea for the Publish panel; its comment list scrolls internally past
/// this, so the extra height stays fixed no matter how many comments a
/// review produced.
const PUBLISH_PANEL_EXTRA_HEIGHT: f32 = 320.0;
const REPOS_CONF_PATH: &str = concat!(env!("PR_TRACKER_BASE_DIR"), "/pr-review/repos.conf");
const GH_POLL_INTERVAL: Duration = Duration::from_secs(45);
const FILE_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StatusEntry {
    title: String,
    url: String,
    outcome: String,
    #[serde(rename = "reportPath")]
    report_path: String,
    #[serde(rename = "finishedAt")]
    finished_at: String,
}

#[derive(Debug, Deserialize)]
struct GhPrState {
    state: String,
}

/// One finding from the sidecar JSON a review run writes next to its `.md`
/// report. `inline` says whether the line sits inside the PR's diff (so
/// GitHub can attach a real inline comment to it) or not (so it has to ride
/// along in the review's overall text instead) — see report-format.md.
#[derive(Debug, Clone, Deserialize)]
struct ReviewComment {
    path: String,
    line: u64,
    inline: bool,
    body: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ReviewSidecar {
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    #[serde(rename = "summaryLine")]
    summary_line: String,
    // Not sent when publishing (see publish_pending_review) — the overall review comment
    // is left for the user to write themselves, not auto-filled from the report.
    #[allow(dead_code)]
    #[serde(rename = "closingLine")]
    closing_line: String,
    #[serde(default)]
    comments: Vec<ReviewComment>,
}

/// What the user has decided to do with each sidecar comment before
/// publishing — `include` drops it from the draft entirely, `inline` can
/// override the sidecar's guess in either direction if that guess turns out
/// wrong (GitHub will reject the whole draft if even one "inline" comment
/// doesn't actually land on a diff line).
struct PublishDraft {
    sidecar: ReviewSidecar,
    include: Vec<bool>,
    inline: Vec<bool>,
    result: Option<Result<String, String>>,
    publishing: bool,
}

#[derive(Serialize)]
struct PublishComment<'a> {
    path: &'a str,
    line: u64,
    side: &'static str,
    body: &'a str,
}

#[derive(Serialize)]
struct PublishPayload<'a> {
    commit_id: &'a str,
    body: String,
    comments: Vec<PublishComment<'a>>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhPr {
    number: u64,
    title: String,
    url: String,
    repository: GhRepo,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRepo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Default)]
struct SharedState {
    assigned: Vec<GhPr>,
    /// status.json keys confirmed still open on GitHub, for PRs no longer in
    /// `assigned` (e.g. you already submitted your review). Anything that
    /// finished reviewing but isn't here and isn't in `assigned` has merged or
    /// closed, and build_rows drops it.
    finished_still_open: HashSet<String>,
    /// Keys removed via the Remove button, re-checked each poll against the live
    /// head commit: a key drops out of this set (reappears in the tracker) as
    /// soon as new commits land, since the posted comments no longer apply to
    /// the current code.
    dismissed: HashSet<String>,
    error: Option<String>,
    last_fetched: Option<Instant>,
}

struct Row {
    key: String,
    label: String,
    pr_url: String,
    title: String,
    stage: &'static str,
    outcome: Option<String>,
    action_label: &'static str,
    action_target: String,
}

/// Which review skills to run and what to tell them to focus on, for a manual
/// re-review of one specific PR triggered from the tracker UI.
struct ReviewOptions {
    repo_rubric: bool,
    code_review_skill: bool,
    effort: String,
    project_criteria: bool,
    notes: String,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self {
            repo_rubric: true,
            code_review_skill: true,
            effort: "low".to_string(),
            project_criteria: true,
            notes: String::new(),
        }
    }
}

struct PrTrackerApp {
    shared: Arc<Mutex<SharedState>>,
    last_file_refresh: Instant,
    status: HashMap<String, StatusEntry>,
    running: bool,
    /// Which PR key the running review marker names, if any — lets a specific
    /// row show "Reviewing" instead of the whole table lighting up at once.
    running_key: Option<String>,
    /// Key of the row whose review-options panel is open, if any.
    configuring: Option<String>,
    options: HashMap<String, ReviewOptions>,
    /// Key of the row whose Publish panel is open, if any. Mutually
    /// exclusive with `configuring` — opening one closes the other, so the
    /// window never has to size for both at once.
    publishing: Option<String>,
    publish_drafts: HashMap<String, PublishDraft>,
    /// Text typed into the "review a PR by URL" box.
    adhoc_url: String,
    /// Keys submitted through that box that don't have a row yet from the
    /// normal assigned/finished_still_open path — title and PR url fetched
    /// once up front so the row looks real immediately, not a placeholder.
    /// Entries prune themselves once the real path catches up with the same
    /// key (see the merge in `ui`), so this never duplicates a row.
    pending_adhoc: HashMap<String, (String, String)>,
    /// Repo selected in the "review a local branch" dropdown.
    local_branch_repo: String,
    /// Text typed into the "review a local branch" box.
    local_branch_name: String,
    /// Keys submitted through that box with no status.json entry yet. Unlike
    /// `pending_adhoc`, these never get picked up by assigned/finished_still_open
    /// (there's no GitHub PR), so build_rows' own status-only pass is what
    /// eventually takes over — this only needs to show something between the
    /// click and that pass, and its title/url are derivable straight from the
    /// key, so no fetch is needed up front.
    pending_local: HashSet<String>,
    status_message: Option<String>,
    /// Which PR key `status_message` was about, if it was a "review started
    /// for X" message — lets refresh_files expire it once the RUNNING file
    /// no longer names that key, instead of it sitting there forever
    /// claiming a review is in progress after it already finished.
    status_message_key: Option<String>,
    /// How much extra height the currently open panel (review options or
    /// publish) needs, so a resize command is only sent when that changes —
    /// not every frame, which would fight a manual drag-resize by the user.
    last_extra_height: f32,
    /// Repos from repos.conf, reloaded on every `refresh_files` tick (like
    /// status.json) so adding/removing one takes effect without restarting.
    repos: Vec<String>,
}

impl PrTrackerApp {
    fn new(shared: Arc<Mutex<SharedState>>) -> Self {
        let mut app = Self {
            shared,
            last_file_refresh: Instant::now() - FILE_POLL_INTERVAL * 2,
            status: HashMap::new(),
            running: false,
            running_key: None,
            configuring: None,
            options: HashMap::new(),
            publishing: None,
            publish_drafts: HashMap::new(),
            adhoc_url: String::new(),
            pending_adhoc: HashMap::new(),
            local_branch_repo: String::new(),
            local_branch_name: String::new(),
            pending_local: HashSet::new(),
            status_message: None,
            status_message_key: None,
            last_extra_height: 0.0,
            repos: Vec::new(),
        };
        app.refresh_files();
        app
    }

    fn refresh_files(&mut self) {
        self.repos = load_repos();
        // Keep the dropdown pointed at a real repo once one loads — matters on first
        // launch, when `new()` had nothing to default it to yet, and again if the
        // previously-selected repo gets removed from repos.conf out from under it.
        if !self.repos.contains(&self.local_branch_repo) {
            self.local_branch_repo = self.repos.first().cloned().unwrap_or_default();
        }
        self.status = std::fs::read_to_string(STATUS_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let running_content = std::fs::read_to_string(RUNNING_PATH).ok();
        self.running = running_content.is_some();
        self.running_key = running_content
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(key) = &self.status_message_key {
            if self.running_key.as_deref() != Some(key.as_str()) {
                self.status_message = None;
                self.status_message_key = None;
            }
        }
        self.last_file_refresh = Instant::now();
    }

    /// Handles the "review a PR by URL" box: resolves the pasted URL to a
    /// key, fetches its title once (also serving as a check that the URL is
    /// real and reachable), then opens the same review-options panel the
    /// table's own "Review…" button uses — everything past that point is
    /// identical to re-reviewing a PR that was already in the list.
    fn submit_adhoc_url(&mut self) {
        let Some((repo, number)) = parse_pr_url(&self.adhoc_url) else {
            self.status_message = Some(
                "couldn't find a PR URL in that (expected github.com/<owner>/<repo>/pull/<number>)"
                    .to_string(),
            );
            self.status_message_key = None;
            return;
        };
        if !self.repos.contains(&repo) {
            self.status_message = Some(format!(
                "{repo} isn't one of the repos this is set up for ({})",
                self.repos.join(", ")
            ));
            self.status_message_key = None;
            return;
        }
        let key = format!("{repo}#{number}");
        if !self.status.contains_key(&key) {
            match fetch_pr_title(&repo, number) {
                Ok((title, pr_url)) => {
                    self.pending_adhoc.insert(key.clone(), (title, pr_url));
                }
                Err(e) => {
                    self.status_message = Some(format!("couldn't look up {key}: {e}"));
                    self.status_message_key = None;
                    return;
                }
            }
        }
        self.adhoc_url.clear();
        self.configuring = Some(key);
        self.publishing = None;
    }

    /// Handles the "review a local branch" box — no PR exists to fetch or
    /// validate against here, so this just builds the `@`-keyed row and opens
    /// the same review-options panel; review-local.sh does the real work of
    /// checking the branch actually resolves once it runs.
    fn submit_local_branch(&mut self) {
        let branch = self.local_branch_name.trim().to_string();
        if branch.is_empty() {
            return;
        }
        let key = format!("{}@{branch}", self.local_branch_repo);
        if !self.status.contains_key(&key) {
            self.pending_local.insert(key.clone());
        }
        self.local_branch_name.clear();
        self.configuring = Some(key);
        self.publishing = None;
    }
}

fn short_repo(name_with_owner: &str) -> &str {
    name_with_owner.split('/').nth(1).unwrap_or(name_with_owner)
}

fn key_label(key: &str) -> String {
    if let Some((repo, num)) = key.split_once('#') {
        return format!("{} #{num}", short_repo(repo));
    }
    if let Some((repo, branch)) = key.split_once('@') {
        return format!("{} @ {branch}", short_repo(repo));
    }
    key.to_string()
}

/// A `#` key is a real GitHub PR; an `@` key is a local branch with no PR
/// yet (see review-local.sh). Several things only make sense for the former
/// — fetching a live head SHA, publishing a GitHub review.
fn is_pr_key(key: &str) -> bool {
    key.contains('#')
}

/// `open -a "Visual Studio Code" <path>` reuses whatever window VS Code
/// already had for that path, buffer and all, and showed stale content for a
/// file that had since been rewritten on disk — `--new-window` fixed that by
/// never reusing a buffer, but spawning a whole new window per click is
/// overkill. `--reuse-window` opens a tab in the existing window instead; no
/// `.vscode/settings.json` or global VS Code setting excludes `.claude/` from
/// its watcher (checked), so that earlier miss looks like a one-off rather
/// than something reproducible — if it recurs, Cmd+Shift+P "Revert File" forces
/// a reread.
/// `code --reuse-window` opens the right window, but the `code` CLI gets
/// there by briefly spinning up its own Electron process to relay the
/// command — that process is the extra dock icon. This URL scheme is handled
/// directly inside the already-running window instead, so nothing new spawns.
fn open_report(path: &str) {
    let encoded = path.replace(' ', "%20");
    let _ = Command::new(OPEN_PATH)
        .arg(format!("vscode://file/{encoded}"))
        .spawn();
}

/// Without egui-winit's "links" feature there's no built-in URL opener, so PR
/// links are handled the same way as the Report button: shell out directly.
fn open_url(url: &str) {
    let _ = Command::new(OPEN_PATH).arg(url).spawn();
}

fn spawn_review(key: &str, opts: &ReviewOptions) -> Result<(), String> {
    let (repo, number) = key
        .split_once('#')
        .ok_or_else(|| format!("malformed PR key: {key}"))?;
    Command::new(BASH_PATH)
        .arg(REVIEW_ONE_SCRIPT)
        .args([
            "--repo",
            repo,
            "--number",
            number,
            "--layer1",
            bool_str(opts.repo_rubric),
            "--layer2",
            bool_str(opts.code_review_skill),
            "--effort",
            &opts.effort,
            "--layer3",
            bool_str(opts.project_criteria),
            "--notes",
            &opts.notes,
        ])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to start review: {e}"))
}

/// Same idea as `spawn_review`, for an `@`-keyed local-branch review instead
/// of a `#`-keyed PR — see review-local.sh.
fn spawn_local_review(key: &str, opts: &ReviewOptions) -> Result<(), String> {
    let (repo, branch) = key
        .split_once('@')
        .ok_or_else(|| format!("malformed local-branch key: {key}"))?;
    Command::new(BASH_PATH)
        .arg(REVIEW_LOCAL_SCRIPT)
        .args([
            "--repo",
            repo,
            "--branch",
            branch,
            "--layer1",
            bool_str(opts.repo_rubric),
            "--layer2",
            bool_str(opts.code_review_skill),
            "--effort",
            &opts.effort,
            "--layer3",
            bool_str(opts.project_criteria),
            "--notes",
            &opts.notes,
        ])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to start review: {e}"))
}

/// Resolves the `gh` binary once and caches it for the rest of the run. An explicit
/// `PR_TRACKER_GH_PATH` override always wins; otherwise this checks the usual
/// Homebrew/system install locations, and only as a last resort asks the user's login
/// shell for its PATH — slower (spawns a shell) but catches a `gh` installed somewhere
/// a GUI-launched app's own minimal PATH wouldn't otherwise see.
fn gh_path() -> &'static str {
    static RESOLVED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            if let Some(path) = GH_PATH_OVERRIDE {
                return path.to_string();
            }
            for candidate in ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"] {
                if std::path::Path::new(candidate).is_file() {
                    return candidate.to_string();
                }
            }
            if let Ok(output) = Command::new("/bin/zsh").args(["-ilc", "command -v gh"]).output() {
                let found = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !found.is_empty() {
                    return found;
                }
            }
            "gh".to_string()
        })
        .as_str()
}

/// Reads the `<owner/repo>:<local-folder-name>` lines from repos.conf (same file and
/// format the shell scripts read via repos.sh) and returns just the `owner/repo` side —
/// the Rust app only ever needs the repo names themselves (for the GitHub search query,
/// URL validation, and the local-branch dropdown); the local folder name is only
/// meaningful to the shell scripts that actually `cd`/`git -C` into it. Missing file or
/// no valid lines both just come back as an empty list — the UI surfaces that plainly
/// rather than panicking on a fresh clone that hasn't copied repos.conf.example yet.
fn load_repos() -> Vec<String> {
    std::fs::read_to_string(REPOS_CONF_PATH)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(':').map(|(repo, _)| repo.trim().to_string()))
        .collect()
}

fn load_json_map(path: &str) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_json_map(path: &str, map: &HashMap<String, String>) {
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, json);
    }
}

/// Deletes a status.json entry along with its report and sidecar — by the time either
/// `remove()` or `prune_closed_prs()` calls this, there's nothing left to act on, so
/// there's no reason to keep the files around. Shared so the two call sites (one
/// user-initiated, one automatic) can't drift apart on what "cleaned up" means.
fn delete_report(key: &str) {
    let mut status: HashMap<String, StatusEntry> = std::fs::read_to_string(STATUS_PATH)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if let Some(entry) = status.remove(key) {
        let _ = std::fs::remove_file(&entry.report_path);
        let _ = std::fs::remove_file(sidecar_path(&entry.report_path));
        if let Ok(json) = serde_json::to_string_pretty(&status) {
            let _ = std::fs::write(STATUS_PATH, json);
        }
    }
}

/// Removes a reviewed PR by recording the commit it was reviewed at (state.json's
/// SHA for that key, the same one the report's comments apply to). The background poll
/// un-hides it again once the live head commit moves past that SHA — see
/// `prune_dismissed` in `main`'s polling loop. A local-branch key has no state.json
/// entry (those aren't deduped against a stored commit) — falling back to an empty
/// string there just means it stays removed for good, which is right: nothing
/// re-checks a local branch's SHA to know when to bring it back.
fn remove_reviewed(key: &str) {
    let sha = load_json_map(STATE_PATH).remove(key).unwrap_or_default();
    let mut dismissed = load_json_map(DISMISSED_PATH);
    dismissed.insert(key.to_string(), sha);
    save_json_map(DISMISSED_PATH, &dismissed);
    delete_report(key);
}

/// Cleans up PR-keyed status.json entries for PRs that have actually merged or
/// closed (not just "no longer requesting my review but still open" — that case is
/// `still_open` and stays visible via `finished_still_open`). Once a PR is gone for
/// good there's no row left to click Remove on — closed/merged PRs are never shown —
/// so without this, their reports would sit in pr-reviews/ forever with no way to
/// clear them. Local-branch keys (no `#`) are left alone; they have no GitHub PR to
/// check a merge/close state against.
fn prune_closed_prs(assigned_keys: &HashSet<String>, still_open: &HashSet<String>) {
    for key in load_status_keys() {
        if is_pr_key(&key) && !assigned_keys.contains(&key) && !still_open.contains(&key) {
            delete_report(&key);
        }
    }
}

/// The sidecar a review run writes sits next to its `.md` report under the
/// same name with a `.json` extension — see report-format.md.
fn sidecar_path(report_path: &str) -> String {
    report_path
        .strip_suffix(".md")
        .map(|stem| format!("{stem}.json"))
        .unwrap_or_else(|| format!("{report_path}.json"))
}

fn load_sidecar(report_path: &str) -> Result<ReviewSidecar, String> {
    let path = sidecar_path(report_path);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("couldn't parse {path}: {e}"))
}

/// Posts a draft (pending) review to GitHub: every included comment marked
/// `inline` lands on its exact diff line, everything else — comments marked
/// general, plus the review's own summary and closing line — goes in the
/// review's overall text instead. Leaving out the `event` field is what
/// keeps this a draft: nothing is submitted, approved, or requested-changed
/// until the user finishes it themselves in GitHub's own review screen.
fn publish_pending_review(key: &str, draft: &PublishDraft) -> Result<String, String> {
    let (repo, number) = key
        .split_once('#')
        .ok_or_else(|| format!("malformed PR key: {key}"))?;

    let mut inline_comments = Vec::new();
    let mut folded = Vec::new();
    for ((comment, &included), &inline) in draft
        .sidecar
        .comments
        .iter()
        .zip(draft.include.iter())
        .zip(draft.inline.iter())
    {
        if !included {
            continue;
        }
        if inline {
            inline_comments.push(PublishComment {
                path: &comment.path,
                line: comment.line,
                side: "RIGHT",
                body: &comment.body,
            });
        } else {
            folded.push(format!("- `{}:{}` — {}", comment.path, comment.line, comment.body));
        }
    }

    // No summary/closing line here on purpose — the overall review comment is something
    // to write yourself, not something to publish sight-unseen. The only thing that goes
    // in `body` is comments that couldn't be placed inline (GitHub requires *some* text
    // field to carry those; dropping them instead would silently lose findings).
    let body = if folded.is_empty() {
        String::new()
    } else {
        format!("Also flagged (not directly commentable on this diff):\n{}", folded.join("\n"))
    };

    let payload = PublishPayload {
        commit_id: &draft.sidecar.head_ref_oid,
        body,
        comments: inline_comments,
    };
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|e| format!("failed to build request body: {e}"))?;

    let mut child = Command::new(gh_path())
        .args([
            "api",
            "--method",
            "POST",
            &format!("repos/{repo}/pulls/{number}/reviews"),
            "--input",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start gh: {e}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(&payload_bytes)
        .map_err(|e| format!("failed to send request body to gh: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("gh didn't finish: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let html_url = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|v| v.get("html_url").and_then(|u| u.as_str().map(str::to_string)));
    Ok(html_url.unwrap_or_else(|| format!("https://github.com/{repo}/pull/{number}/files")))
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

fn stage_color(stage: &str) -> egui::Color32 {
    match stage {
        "Done" => egui::Color32::from_rgb(98, 209, 150),
        "Reviewing" => egui::Color32::from_rgb(240, 180, 80),
        _ => egui::Color32::from_rgb(150, 160, 180),
    }
}

struct Palette {
    background: egui::Color32,
    hyperlink: egui::Color32,
    selection: egui::Color32,
    noninteractive_bg: egui::Color32,
    inactive_bg: egui::Color32,
    hovered_bg: egui::Color32,
    active_bg: egui::Color32,
    card_bg: egui::Color32,
    card_border: egui::Color32,
    muted_text: egui::Color32,
}

const DARK: Palette = Palette {
    background: egui::Color32::from_rgb(24, 25, 29),
    hyperlink: egui::Color32::from_rgb(120, 170, 255),
    selection: egui::Color32::from_rgb(70, 100, 160),
    noninteractive_bg: egui::Color32::from_rgb(30, 31, 36),
    inactive_bg: egui::Color32::from_rgb(40, 42, 48),
    hovered_bg: egui::Color32::from_rgb(55, 58, 66),
    active_bg: egui::Color32::from_rgb(70, 74, 84),
    card_bg: egui::Color32::from_rgb(32, 33, 39),
    card_border: egui::Color32::from_rgb(46, 48, 55),
    muted_text: egui::Color32::from_rgb(150, 153, 163),
};

const LIGHT: Palette = Palette {
    background: egui::Color32::from_rgb(248, 248, 250),
    hyperlink: egui::Color32::from_rgb(10, 90, 220),
    selection: egui::Color32::from_rgb(195, 215, 245),
    noninteractive_bg: egui::Color32::from_rgb(241, 241, 244),
    inactive_bg: egui::Color32::from_rgb(230, 231, 235),
    hovered_bg: egui::Color32::from_rgb(219, 221, 227),
    active_bg: egui::Color32::from_rgb(205, 208, 216),
    card_bg: egui::Color32::from_rgb(255, 255, 255),
    card_border: egui::Color32::from_rgb(227, 228, 232),
    muted_text: egui::Color32::from_rgb(120, 122, 130),
};

fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

/// A rounded, tinted badge for a review outcome — distinct hue per outcome so the
/// list reads at a glance without parsing the text. Unknown/empty outcomes
/// (shouldn't happen, but `Row.outcome` is an `Option<String>`) fall back to gray
/// rather than panicking on an unrecognized string.
fn outcome_color(outcome: &str) -> egui::Color32 {
    match outcome {
        "approve" => egui::Color32::from_rgb(98, 209, 150),
        "request-changes" => egui::Color32::from_rgb(235, 120, 120),
        "needs-discussion" => egui::Color32::from_rgb(230, 180, 90),
        _ => egui::Color32::from_rgb(150, 160, 180),
    }
}

fn outcome_pill(ui: &mut egui::Ui, outcome: &str) {
    let color = outcome_color(outcome);
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(egui::CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.colored_label(color, outcome);
        });
}

/// Styles both the Dark and Light egui themes and leaves `Context`'s theme
/// preference at its default (`ThemePreference::System`), so the window
/// switches to match whichever one macOS is currently using — same approach
/// as `clippy`, which this was modeled on. Forcing one theme (the earlier
/// version of this function did) is exactly what made the app stick out
/// against the rest of the system when it didn't match.
fn apply_style(ctx: &egui::Context) {
    for (theme, base_visuals, palette) in [
        (egui::Theme::Dark, egui::Visuals::dark(), &DARK),
        (egui::Theme::Light, egui::Visuals::light(), &LIGHT),
    ] {
        ctx.style_mut_of(theme, |style| {
            style.visuals = base_visuals;
            let visuals = &mut style.visuals;
            visuals.panel_fill = palette.background;
            visuals.window_fill = palette.background;
            visuals.widgets.noninteractive.bg_fill = palette.noninteractive_bg;
            visuals.widgets.inactive.bg_fill = palette.inactive_bg;
            visuals.widgets.hovered.bg_fill = palette.hovered_bg;
            visuals.widgets.active.bg_fill = palette.active_bg;
            visuals.hyperlink_color = palette.hyperlink;
            visuals.selection.bg_fill = palette.selection;
            for corner_radius in [
                &mut visuals.widgets.noninteractive.corner_radius,
                &mut visuals.widgets.inactive.corner_radius,
                &mut visuals.widgets.hovered.corner_radius,
                &mut visuals.widgets.active.corner_radius,
            ] {
                *corner_radius = egui::CornerRadius::same(6);
            }

            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            for (text_style, font_id) in style.text_styles.iter_mut() {
                font_id.size = match text_style {
                    egui::TextStyle::Heading => 22.0,
                    egui::TextStyle::Body => 15.0,
                    egui::TextStyle::Button => 14.0,
                    egui::TextStyle::Small => 12.0,
                    _ => font_id.size,
                };
            }
        });
    }
}

/// "Reviewing" is shown only for the row the RUNNING marker actually names.
/// A bulk/scheduled run doesn't know its targets ahead of time and leaves the
/// marker empty, in which case every not-yet-done row shows "Reviewing" since
/// any of them could be next — same as before manual per-row triggers existed.
fn row_stage(key: &str, has_status: bool, running: bool, running_key: Option<&str>) -> &'static str {
    // Checked before `has_status`: a re-review of an already-Done PR always has a status
    // entry, so putting that check first meant "Reviewing" could never show for a
    // re-review — the row just sat on "Done" with the stale outcome the whole time.
    if running_key == Some(key) {
        return "Reviewing";
    }
    if has_status {
        return "Done";
    }
    match running_key {
        Some(_) => "Assigned",
        None if running => "Reviewing",
        None => "Assigned",
    }
}

/// A finished PR stays visible after GitHub drops it from the "review requested"
/// list (e.g. once you submit a review) for as long as it's still open, so the
/// outcome doesn't disappear the moment the review request is fulfilled. Once
/// it's merged or closed it's dropped entirely (see `finished_still_open`).
fn build_rows(
    assigned: &[GhPr],
    status: &HashMap<String, StatusEntry>,
    finished_still_open: &HashSet<String>,
    dismissed: &HashSet<String>,
    running: bool,
    running_key: Option<&str>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();

    for pr in assigned {
        let key = format!("{}#{}", pr.repository.name_with_owner, pr.number);
        seen.insert(key.clone());
        if dismissed.contains(&key) {
            continue;
        }
        let label = key_label(&key);

        if let Some(entry) = status.get(&key) {
            rows.push(Row {
                stage: row_stage(&key, true, running, running_key),
                key,
                label,
                pr_url: pr.url.clone(),
                title: pr.title.clone(),
                outcome: Some(entry.outcome.clone()),
                action_label: "Report",
                action_target: entry.report_path.clone(),
            });
        } else {
            rows.push(Row {
                stage: row_stage(&key, false, running, running_key),
                key,
                label,
                pr_url: pr.url.clone(),
                title: pr.title.clone(),
                outcome: None,
                action_label: "",
                action_target: String::new(),
            });
        }
    }

    let mut finished: Vec<_> = status
        .iter()
        .filter(|(key, _)| {
            !seen.contains(*key) && finished_still_open.contains(*key) && !dismissed.contains(*key)
        })
        .collect();
    finished.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (key, entry) in finished {
        rows.push(Row {
            key: key.clone(),
            label: key_label(key),
            pr_url: entry.url.clone(),
            title: entry.title.clone(),
            stage: row_stage(key, true, running, running_key),
            outcome: Some(entry.outcome.clone()),
            action_label: "Report",
            action_target: entry.report_path.clone(),
        });
    }

    // Local-branch reviews (`@`-keyed, see review-local.sh) have no GitHub PR behind them, so
    // they can never come from `assigned` or pass the `finished_still_open` "is it still open"
    // check above — both are GitHub-PR-only concepts. They're shown unconditionally instead,
    // for as long as they have a status.json entry and haven't been hidden.
    let mut local: Vec<_> = status
        .iter()
        .filter(|(key, _)| !seen.contains(*key) && !is_pr_key(key) && !dismissed.contains(*key))
        .collect();
    local.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (key, entry) in local {
        rows.push(Row {
            key: key.clone(),
            label: key_label(key),
            pr_url: entry.url.clone(),
            title: entry.title.clone(),
            stage: row_stage(key, true, running, running_key),
            outcome: Some(entry.outcome.clone()),
            action_label: "Report",
            action_target: entry.report_path.clone(),
        });
    }

    rows
}

impl eframe::App for PrTrackerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_file_refresh.elapsed() > FILE_POLL_INTERVAL {
            self.refresh_files();
        }
        let ctx = ui.ctx().clone();

        egui::CentralPanel::default().show(ui, |ui| {
            // Belt-and-suspenders: the window itself grows to fit the review panel (see
            // the resize below), but this keeps everything reachable even if a very long
            // title or a tall notes field ever pushes past that extra height.
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading("PR Review Tracker");
            ui.horizontal(|ui| {
                ui.label(if self.running {
                    "automation: reviewing now"
                } else {
                    "automation: idle"
                });
                let shared = self.shared.lock().unwrap();
                // A dot instead of a ticking "Ns ago" counter: its color only changes at
                // the fresh/stale threshold, not every second, so it doesn't read as a
                // running timer. Exact freshness is still there on hover for anyone who
                // wants it.
                let (color, tooltip) = match shared.last_fetched {
                    Some(t) if t.elapsed() < GH_POLL_INTERVAL * 2 => (
                        egui::Color32::from_rgb(98, 209, 150),
                        format!("gh refreshed {}s ago", t.elapsed().as_secs()),
                    ),
                    Some(t) => (
                        egui::Color32::from_rgb(240, 180, 80),
                        format!("gh hasn't refreshed in {}s, might be stuck", t.elapsed().as_secs()),
                    ),
                    None => (
                        egui::Color32::from_rgb(150, 160, 180),
                        "gh hasn't refreshed yet".to_string(),
                    ),
                };
                ui.colored_label(color, "●").on_hover_text(tooltip);
            });
            if let Some(msg) = &self.status_message {
                ui.colored_label(egui::Color32::from_rgb(120, 170, 255), msg);
            }
            if self.repos.is_empty() {
                ui.colored_label(
                    egui::Color32::RED,
                    "no repos configured — copy pr-review/repos.conf.example to \
                     pr-review/repos.conf and list the repos you want reviewed",
                );
            }
            ui.horizontal(|ui| {
                ui.label("Review a PR by URL:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.adhoc_url)
                        .desired_width(340.0)
                        .hint_text(format!(
                            "https://github.com/{}/pull/1234",
                            self.repos.first().map(String::as_str).unwrap_or("owner/repo")
                        )),
                );
                // Only enabled once the field actually parses to a PR URL on a
                // supported repo, so a stray click on an empty/placeholder field
                // can't fire a confusing "couldn't find a PR URL" message.
                let valid = parse_pr_url(&self.adhoc_url)
                    .is_some_and(|(repo, _)| self.repos.contains(&repo));
                if ui
                    .add_enabled(valid, egui::Button::new("Review…"))
                    .clicked()
                {
                    self.submit_adhoc_url();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Review a local branch:");
                egui::ComboBox::from_id_salt("local_branch_repo")
                    .selected_text(short_repo(&self.local_branch_repo))
                    .show_ui(ui, |ui| {
                        for repo in &self.repos {
                            ui.selectable_value(&mut self.local_branch_repo, repo.clone(), short_repo(repo));
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut self.local_branch_name)
                        .desired_width(260.0)
                        .hint_text("DEV-19297-variants-license"),
                );
                let valid = !self.local_branch_name.trim().is_empty();
                if ui
                    .add_enabled(valid, egui::Button::new("Review…"))
                    .clicked()
                {
                    self.submit_local_branch();
                }
            });
            ui.separator();

            let shared = self.shared.lock().unwrap();
            let gh_error = shared.error.clone();
            let mut rows = build_rows(
                &shared.assigned,
                &self.status,
                &shared.finished_still_open,
                &shared.dismissed,
                self.running,
                self.running_key.as_deref(),
            );
            drop(shared);

            // Rows for PRs submitted via the URL box that the normal assigned/
            // finished_still_open path doesn't know about yet. Once that path does
            // catch up with the same key (next gh poll), this prunes itself so the
            // key is never shown twice.
            let seen: HashSet<String> = rows.iter().map(|r| r.key.clone()).collect();
            for (key, (title, url)) in &self.pending_adhoc {
                if seen.contains(key) {
                    continue;
                }
                if let Some(entry) = self.status.get(key) {
                    rows.push(Row {
                        key: key.clone(),
                        label: key_label(key),
                        pr_url: entry.url.clone(),
                        title: entry.title.clone(),
                        stage: row_stage(key, true, self.running, self.running_key.as_deref()),
                        outcome: Some(entry.outcome.clone()),
                        action_label: "Report",
                        action_target: entry.report_path.clone(),
                    });
                } else {
                    rows.push(Row {
                        key: key.clone(),
                        label: key_label(key),
                        pr_url: url.clone(),
                        title: title.clone(),
                        stage: row_stage(key, false, self.running, self.running_key.as_deref()),
                        outcome: None,
                        action_label: "",
                        action_target: String::new(),
                    });
                }
            }
            self.pending_adhoc.retain(|k, _| !seen.contains(k));

            // Same idea for a submitted-but-not-yet-reviewed local branch — once status.json
            // has the entry, build_rows' own local-branch pass above already included it in
            // `rows`, so it's already in `seen` and this naturally stops showing the synthetic
            // version.
            for key in &self.pending_local {
                if seen.contains(key) {
                    continue;
                }
                let (repo, branch) = key.split_once('@').unwrap_or((key.as_str(), ""));
                rows.push(Row {
                    key: key.clone(),
                    label: key_label(key),
                    pr_url: format!("https://github.com/{repo}/tree/{branch}"),
                    title: branch.to_string(),
                    stage: row_stage(key, false, self.running, self.running_key.as_deref()),
                    outcome: None,
                    action_label: "",
                    action_target: String::new(),
                });
            }
            self.pending_local.retain(|k| !seen.contains(k));

            if let Some(err) = &gh_error {
                ui.colored_label(egui::Color32::RED, format!("gh error: {err}"));
                ui.separator();
            }

            if rows.is_empty() && gh_error.is_none() {
                ui.label("no assigned, in-progress, or completed reviews right now");
                return;
            }

            let mut clicked_report: Option<String> = None;
            let mut clicked_configure: Option<String> = None;
            let mut clicked_remove: Option<String> = None;
            let mut clicked_publish: Option<(String, String)> = None;

            let pal = palette(ui);
            for row in &rows {
                let card_id = ui.make_persistent_id(("pr_card", &row.key));
                let hovered = ui.ctx().data(|d| d.get_temp::<bool>(card_id)).unwrap_or(false);
                let bg_t = ui.ctx().animate_bool_with_time(card_id, hovered, 0.12);
                let bg = pal.card_bg.lerp_to_gamma(pal.hovered_bg, bg_t * 0.5);

                let frame_response = egui::Frame::new()
                    .fill(bg)
                    .stroke(egui::Stroke::new(1.0, pal.card_border))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(stage_color(row.stage), "●");
                            if ui.link(&row.label).clicked() {
                                open_url(&row.pr_url);
                            }
                            // Capped width so a long title can't push the outcome pill off
                            // the right edge of the window; full text on hover.
                            ui.scope(|ui| {
                                ui.set_max_width(380.0);
                                ui.add(egui::Label::new(&row.title).truncate())
                                    .on_hover_text(&row.title);
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if let Some(outcome) = &row.outcome {
                                    outcome_pill(ui, outcome);
                                }
                            });
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.colored_label(pal.muted_text, row.stage);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let reviewed = !row.action_label.is_empty();
                                if reviewed
                                    && ui
                                        .button("Remove")
                                        .on_hover_text(
                                            "Delete this report — comes back if new commits are pushed to this PR",
                                        )
                                        .clicked()
                                {
                                    clicked_remove = Some(row.key.clone());
                                }
                                // A local branch has no PR yet, so there's nothing to attach a
                                // GitHub review to.
                                if reviewed
                                    && is_pr_key(&row.key)
                                    && ui
                                        .button("Publish…")
                                        .on_hover_text(
                                            "Draft these comments as a pending GitHub review",
                                        )
                                        .clicked()
                                {
                                    clicked_publish =
                                        Some((row.key.clone(), row.action_target.clone()));
                                }
                                if ui.button("Review…").clicked() {
                                    clicked_configure = Some(row.key.clone());
                                }
                                if reviewed && ui.button(row.action_label).clicked() {
                                    clicked_report = Some(row.action_target.clone());
                                }
                            });
                        });
                    })
                    .response;

                let now_hovered = frame_response.hovered();
                if now_hovered != hovered {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(card_id, now_hovered));
                    ui.ctx().request_repaint();
                }
                ui.add_space(6.0);
            }

            if let Some(path) = clicked_report {
                open_report(&path);
            }
            if let Some(key) = clicked_remove {
                self.shared.lock().unwrap().dismissed.insert(key.clone());
                remove_reviewed(&key);
            }
            if let Some(key) = clicked_configure {
                self.configuring = Some(key);
                self.publishing = None;
            }
            if let Some((key, report_path)) = clicked_publish {
                match load_sidecar(&report_path) {
                    Ok(sidecar) => {
                        let include = vec![true; sidecar.comments.len()];
                        let inline = sidecar.comments.iter().map(|c| c.inline).collect();
                        self.publish_drafts.insert(
                            key.clone(),
                            PublishDraft {
                                sidecar,
                                include,
                                inline,
                                result: None,
                                publishing: false,
                            },
                        );
                        self.publishing = Some(key);
                        self.configuring = None;
                    }
                    Err(e) => {
                        self.status_message = Some(format!("couldn't load sidecar: {e}"));
                        self.status_message_key = None;
                    }
                }
            }

            if let Some(key) = self.configuring.clone() {
                let label = rows
                    .iter()
                    .find(|r| r.key == key)
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| key.clone());

                ui.separator();
                ui.group(|ui| {
                    ui.strong(format!("Re-review {label}"));
                    let opts = self.options.entry(key.clone()).or_default();
                    ui.checkbox(
                        &mut opts.repo_rubric,
                        "Repo rubric — .github/skills/code-review/SKILL.md",
                    );
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut opts.code_review_skill, "Built-in `code-review` skill");
                        ui.add_enabled_ui(opts.code_review_skill, |ui| {
                            egui::ComboBox::from_id_salt("effort")
                                .selected_text(&opts.effort)
                                .show_ui(ui, |ui| {
                                    for level in EFFORT_LEVELS {
                                        ui.selectable_value(
                                            &mut opts.effort,
                                            level.to_string(),
                                            level,
                                        );
                                    }
                                });
                        });
                    });
                    ui.checkbox(
                        &mut opts.project_criteria,
                        "Project rules — review-criteria.md",
                    );
                    ui.label("Additional notes for this run (optional):");
                    ui.text_edit_multiline(&mut opts.notes);

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!self.running, egui::Button::new("Run review now"))
                            .clicked()
                        {
                            let spawn_result = if is_pr_key(&key) {
                                spawn_review(&key, opts)
                            } else {
                                spawn_local_review(&key, opts)
                            };
                            match spawn_result {
                                Ok(()) => {
                                    self.status_message =
                                        Some(format!("review started for {label}"));
                                    self.status_message_key = Some(key.clone());
                                    self.configuring = None;
                                    // Optimistic: the RUNNING file write and the next 2s
                                    // file poll both lag slightly behind the process we
                                    // just spawned. Set this now so the row flips to
                                    // "Reviewing" on the next frame instead of sitting on
                                    // the stale "Done" for a couple of seconds.
                                    self.running = true;
                                    self.running_key = Some(key.clone());
                                }
                                Err(e) => {
                                    self.status_message = Some(format!("error: {e}"));
                                    self.status_message_key = None;
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.configuring = None;
                        }
                    });
                    if self.running {
                        ui.weak("a review is already in progress, wait for it to finish first");
                    }
                });
            }

            if let Some(key) = self.publishing.clone() {
                let label = rows
                    .iter()
                    .find(|r| r.key == key)
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| key.clone());

                ui.separator();
                ui.group(|ui| {
                    ui.strong(format!("Publish {label} as a draft GitHub review"));
                    ui.weak(
                        "Creates a pending review with these comments placed on GitHub — nothing \
                         is submitted. You edit, add to, or remove from it and hit Submit yourself \
                         on GitHub.",
                    );

                    if let Some(draft) = self.publish_drafts.get_mut(&key) {
                        ui.label(&draft.sidecar.summary_line);

                        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                            for (i, comment) in draft.sidecar.comments.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut draft.include[i], "");
                                    ui.add_enabled_ui(draft.include[i], |ui| {
                                        ui.checkbox(&mut draft.inline[i], "inline");
                                    });
                                    ui.add(
                                        egui::Label::new(format!(
                                            "{}:{} — {}",
                                            comment.path, comment.line, comment.body
                                        ))
                                        .truncate(),
                                    )
                                    .on_hover_text(&comment.body);
                                });
                            }
                            if draft.sidecar.comments.is_empty() {
                                ui.weak("no inline comments in this report");
                            }
                        });

                        if let Some(result) = &draft.result {
                            match result {
                                Ok(_) => {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(98, 209, 150),
                                        "draft review created, opened in your browser",
                                    );
                                }
                                Err(e) => {
                                    ui.colored_label(egui::Color32::RED, format!("publish failed: {e}"));
                                }
                            }
                        }

                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !draft.publishing,
                                    egui::Button::new("Create draft review"),
                                )
                                .clicked()
                            {
                                draft.publishing = true;
                                // Blocks the UI for the one GitHub round-trip — a deliberate,
                                // rare click, not worth a background thread and a result channel for.
                                let result = publish_pending_review(&key, draft);
                                if let Ok(url) = &result {
                                    open_url(url);
                                }
                                draft.result = Some(result);
                                draft.publishing = false;
                            }
                            if ui.button("Close").clicked() {
                                self.publishing = None;
                            }
                        });
                    }
                });
            }
            });
        });

        let target_extra_height = if self.configuring.is_some() {
            REVIEW_PANEL_EXTRA_HEIGHT
        } else if self.publishing.is_some() {
            PUBLISH_PANEL_EXTRA_HEIGHT
        } else {
            0.0
        };
        if (target_extra_height - self.last_extra_height).abs() > 0.5 {
            // Keep whatever width the window currently has (in case it was dragged wider
            // to read long titles) and only change height, so toggling a panel doesn't
            // clobber a manual resize.
            let width = ctx
                .input(|i| i.viewport().inner_rect)
                .map(|r| r.width())
                .unwrap_or(BASE_WINDOW_SIZE[0]);
            let height = BASE_WINDOW_SIZE[1] + target_extra_height;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
            self.last_extra_height = target_extra_height;
            // The resize command only takes effect once the OS actually reports the new
            // size back on a later frame. Left to the 1-second idle schedule below, that
            // readback (and the repaint that shows it) was visibly delayed; request one
            // right away instead, and keep requesting until the size settles.
            ctx.request_repaint();
        } else if self.last_extra_height > 0.0 {
            ctx.request_repaint();
        }

        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

fn fetch_assigned(repos: &[String]) -> Result<Vec<GhPr>, String> {
    if repos.is_empty() {
        // Rather than run an unscoped `gh search prs` across every repo the token can
        // see — never what's wanted — surface the same "go configure this" message the
        // UI already shows when repos.conf is empty or missing.
        return Err(
            "no repos configured — copy pr-review/repos.conf.example to repos.conf".to_string(),
        );
    }
    let mut args = vec!["search", "prs", "--review-requested", "@me", "--state", "open"];
    for repo in repos {
        args.push("--repo");
        args.push(repo);
    }
    args.push("--json");
    args.push("number,title,url,repository");

    let output = Command::new(gh_path())
        .args(&args)
        .output()
        .map_err(|e| format!("failed to spawn gh: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    serde_json::from_slice(&output.stdout).map_err(|e| format!("failed to parse gh output: {e}"))
}

fn is_pr_still_open(key: &str) -> bool {
    let Some((repo, number)) = key.split_once('#') else {
        return false;
    };
    let output = Command::new(gh_path())
        .args(["pr", "view", number, "--repo", repo, "--json", "state"])
        .output();

    match output {
        Ok(out) if out.status.success() => serde_json::from_slice::<GhPrState>(&out.stdout)
            .map(|s| s.state == "OPEN")
            .unwrap_or(false),
        _ => false,
    }
}

fn load_status_keys() -> Vec<String> {
    std::fs::read_to_string(STATUS_PATH)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, StatusEntry>>(&s).ok())
        .map(|m| m.into_keys().collect())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct GhPrHead {
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
}

/// Pulls `<owner>/<repo>` and the PR number out of a pasted GitHub PR URL —
/// tolerates a missing scheme and trailing segments like `/files` so a
/// straight copy-paste from the browser's address bar just works.
fn parse_pr_url(input: &str) -> Option<(String, u64)> {
    let trimmed = input
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let rest = trimmed.strip_prefix("github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?.parse().ok()?;
    Some((format!("{owner}/{repo}"), number))
}

#[derive(Debug, Deserialize)]
struct GhPrTitleUrl {
    title: String,
    url: String,
}

/// A one-off lookup so a freshly pasted PR shows its real title and link
/// right away instead of a placeholder — also doubles as validation that the
/// URL actually points at a PR you can see before a review panel opens for it.
fn fetch_pr_title(repo: &str, number: u64) -> Result<(String, String), String> {
    let output = Command::new(gh_path())
        .args(["pr", "view", &number.to_string(), "--repo", repo, "--json", "title,url"])
        .output()
        .map_err(|e| format!("failed to run gh: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    serde_json::from_slice::<GhPrTitleUrl>(&output.stdout)
        .map(|p| (p.title, p.url))
        .map_err(|e| format!("couldn't parse gh output: {e}"))
}

fn fetch_head_sha(key: &str) -> Option<String> {
    let (repo, number) = key.split_once('#')?;
    let output = Command::new(gh_path())
        .args(["pr", "view", number, "--repo", repo, "--json", "headRefOid"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<GhPrHead>(&output.stdout)
        .ok()
        .map(|h| h.head_ref_oid)
}

/// Starts a review the moment a poll notices a PR is new or has moved past
/// the commit `state.json` last reviewed, instead of waiting for the
/// half-hour scheduled job to notice. Only ever starts one per call: if a
/// review is already running, everything here just waits for the next poll
/// once the lock frees — as close to "queued" as this gets without risking
/// two reviews writing `state.json`/`status.json` at the same time.
fn maybe_auto_review(prs: &[GhPr]) {
    if std::path::Path::new(RUNNING_PATH).exists() {
        return;
    }
    let state = load_json_map(STATE_PATH);
    for pr in prs {
        let key = format!("{}#{}", pr.repository.name_with_owner, pr.number);
        let Some(live_sha) = fetch_head_sha(&key) else {
            continue;
        };
        if state.get(&key) == Some(&live_sha) {
            continue;
        }
        let _ = spawn_review(&key, &ReviewOptions::default());
        return;
    }
}

/// Drops a hidden PR's key once its live head commit has moved past the SHA
/// it was hidden at — the posted comments no longer apply, so it should come
/// back into view. Keeps anything it can't check (a transient `gh` failure)
/// hidden rather than flicker it back in and out.
fn prune_dismissed() -> HashSet<String> {
    let dismissed = load_json_map(DISMISSED_PATH);
    let mut still_hidden = HashMap::new();
    for (key, hidden_at_sha) in &dismissed {
        match fetch_head_sha(key) {
            Some(live_sha) if live_sha != *hidden_at_sha => {}
            _ => {
                still_hidden.insert(key.clone(), hidden_at_sha.clone());
            }
        }
    }
    if still_hidden.len() != dismissed.len() {
        save_json_map(DISMISSED_PATH, &still_hidden);
    }
    still_hidden.into_keys().collect()
}

fn main() -> eframe::Result<()> {
    let shared = Arc::new(Mutex::new(SharedState::default()));

    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || loop {
            let result = fetch_assigned(&load_repos());

            if let Ok(prs) = &result {
                maybe_auto_review(prs);
            }

            let assigned_keys: HashSet<String> = match &result {
                Ok(prs) => prs
                    .iter()
                    .map(|pr| format!("{}#{}", pr.repository.name_with_owner, pr.number))
                    .collect(),
                Err(_) => HashSet::new(),
            };
            let still_open: HashSet<String> = load_status_keys()
                .into_iter()
                .filter(|key| !assigned_keys.contains(key))
                .filter(|key| is_pr_still_open(key))
                .collect();
            // Only once the search itself actually succeeded — on a transient `gh` failure
            // `assigned_keys` comes back empty, which would otherwise look exactly like
            // every tracked PR merging at once and wipe their reports.
            if result.is_ok() {
                prune_closed_prs(&assigned_keys, &still_open);
            }
            let dismissed = prune_dismissed();

            let mut s = shared.lock().unwrap();
            match result {
                Ok(prs) => {
                    s.assigned = prs;
                    s.error = None;
                }
                Err(e) => {
                    s.error = Some(e);
                }
            }
            s.finished_still_open = still_open;
            s.dismissed = dismissed;
            s.last_fetched = Some(Instant::now());
            drop(s);
            std::thread::sleep(GH_POLL_INTERVAL);
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(BASE_WINDOW_SIZE)
            .with_min_inner_size(BASE_WINDOW_SIZE),
        ..Default::default()
    };

    eframe::run_native(
        "PR Review Tracker",
        options,
        Box::new(|cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(PrTrackerApp::new(shared)))
        }),
    )
}
