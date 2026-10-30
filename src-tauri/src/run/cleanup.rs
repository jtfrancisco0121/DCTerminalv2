//! Automatic cleanup of merged runs (plan 2026-10-27, AFK runs). A run at
//! `stage: done` whose PRs have a `prUrl` is asked about now and then; for
//! each PR GitHub reports `MERGED` with the run's own `dcv2/<runId>-<pr>`
//! head: the janitor removes the worktree (its rules: verified, clean, no
//! `--force`), `git branch -d` (never `-D`), and the remote branch is deleted
//! only when its tip is the PR's `headRefOid`, pushed to the remote URL
//! recorded when the worktree was made (`.guard.json` `remotes`), without
//! hooks and never forced. Open PRs are asked again later; closed, local-only
//! and foreign PRs are left alone. Every final outcome is recorded in
//! `<run>/app-cleanup.json` (app-written) and logged once.
//!
//! status.json and `.guard.json` are written by the LLM-driven skill
//! (through state.sh), so every path, branch and URL from them is checked
//! against the run's contract before anything is removed.

use super::folder::{append_app_log, read_json_value};
use super::git::{canonical, git};
use super::janitor;
use super::status::{parse_status, valid_pr_id, PrView};
use crate::store::write_json_atomic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CLEANUP_FILE: &str = "app-cleanup.json";
/// Runs last changed longer ago than this are not asked about any more.
pub const MAX_AGE_DAYS: i64 = 30;
/// How often one run is asked about (its open PRs may be merged later).
pub const ASK_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long one `gh` or network `git` call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// What the app did for one PR of the run (final: never asked again).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRecord {
    pub pr_id: String,
    pub pr_url: String,
    /// `cleaned` (merged; worktree and remote branch gone), `kept` (merged;
    /// something was kept, see the parts), `untouched` (not merged, or not
    /// this run's PR).
    pub outcome: String,
    /// Why a PR was left alone (outcome `untouched`).
    #[serde(default)]
    pub note: String,
    /// Not final: asked again later (a dirty worktree, a network failure),
    /// within the run's 30 days.
    #[serde(default)]
    pub retry: bool,
    pub worktree: String,
    pub branch: String,
    pub remote: String,
    pub at: String,
}

impl CleanupRecord {
    pub fn line(&self) -> String {
        let parts: Vec<&str> = [&self.note, &self.worktree, &self.branch, &self.remote]
            .into_iter()
            .map(String::as_str)
            .filter(|p| !p.is_empty())
            .collect();
        format!("{}: {}", self.pr_id, parts.join("; "))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupFile {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub prs: BTreeMap<String, CleanupRecord>,
}

pub fn read_cleanup(run_dir: &Path) -> CleanupFile {
    read_json_value(&run_dir.join(CLEANUP_FILE))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// What `gh pr view --json state,mergeCommit,headRefName,headRefOid,baseRefName` said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrInfo {
    pub state: String,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    /// The branch the PR targets (a merged PR's own base is what a PR stacked on it is retargeted to).
    pub base_ref_name: String,
}

/// An open PR whose base is some branch (a PR stacked on a merged run branch).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StackedPr {
    pub url: String,
    pub base_ref_name: String,
}

/// Asks GitHub about one PR (`gh` in the app; a fake in tests).
pub trait PrLookup {
    fn view(&self, url: &str, repo: &str) -> Result<PrInfo, String>;
    /// The open PRs of `repo` whose base is `branch`.
    fn stacked_on(&self, branch: &str, repo: &str) -> Result<Vec<StackedPr>, String>;
    /// Points the PR at `base` and checks, with a fresh `view`, that it took.
    fn retarget(&self, url: &str, base: &str, repo: &str) -> Result<(), String>;
}

/// Runs `command`, killed after `timeout`; (success, stdout, first stderr line).
pub fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("dcv2-call-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (out, err) = (dir.join("out"), dir.join("err"));
    let result = (|| {
        let stdout = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        let stderr = std::fs::File::create(&err).map_err(|e| e.to_string())?;
        let mut child = command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timed out after {} s", timeout.as_secs()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(e.to_string()),
            }
        };
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        if status.success() {
            Ok(text)
        } else {
            let e = std::fs::read_to_string(&err).unwrap_or_default();
            Err(e.lines().next().unwrap_or("failed").trim().to_string())
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// `gh pr view <url> -R <repo> --json …`, no prompts, with a timeout. The only
/// writes are in [`PrLookup::retarget`]: `gh pr edit --base`, for a PR stacked
/// on a merged branch (deleting that branch would close it).
pub struct GhCli;

fn gh_command(args: &[&str]) -> Command {
    let mut cmd = Command::new("gh");
    cmd.args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
    cmd
}

impl PrLookup for GhCli {
    fn view(&self, url: &str, repo: &str) -> Result<PrInfo, String> {
        let cmd = gh_command(&[
            "pr",
            "view",
            url,
            "-R",
            repo,
            "--json",
            "state,mergeCommit,headRefName,headRefOid,baseRefName",
        ]);
        let text = run_with_timeout(cmd, CALL_TIMEOUT).map_err(|e| format!("gh pr view: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| "gh pr view printed no JSON".to_string())?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Ok(PrInfo {
            state: s("state"),
            head_ref_name: s("headRefName"),
            head_ref_oid: s("headRefOid"),
            base_ref_name: s("baseRefName"),
        })
    }

    fn stacked_on(&self, branch: &str, repo: &str) -> Result<Vec<StackedPr>, String> {
        let cmd = gh_command(&[
            "pr",
            "list",
            "--base",
            branch,
            "--state",
            "open",
            "-R",
            repo,
            "--json",
            "url,baseRefName",
            "--limit",
            "100",
        ]);
        let text = run_with_timeout(cmd, CALL_TIMEOUT).map_err(|e| format!("gh pr list: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| "gh pr list printed no JSON".to_string())?;
        let rows = v.as_array().ok_or("gh pr list printed no list")?;
        Ok(rows
            .iter()
            .map(|r| StackedPr {
                url: r.get("url").and_then(|x| x.as_str()).unwrap_or("").into(),
                base_ref_name: r
                    .get("baseRefName")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .into(),
            })
            .collect())
    }

    fn retarget(&self, url: &str, base: &str, repo: &str) -> Result<(), String> {
        let cmd = gh_command(&["pr", "edit", url, "--base", base, "-R", repo]);
        run_with_timeout(cmd, CALL_TIMEOUT).map_err(|e| format!("gh pr edit --base: {e}"))?;
        let now = self.view(url, repo)?;
        if now.base_ref_name == base {
            Ok(())
        } else {
            Err(format!(
                "{url} is on {} after the retarget, not {base}",
                now.base_ref_name
            ))
        }
    }
}

/// A remote URL the app may talk to: an owner/name https or ssh URL, or an
/// existing absolute folder (a local mirror). Never an option (`-…`), a
/// helper transport (`ext::…`) or anything with spaces or control characters.
pub fn safe_remote_url(url: &str) -> bool {
    static SLUG: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let slug = SLUG.get_or_init(|| {
        regex::Regex::new(
            r"^(?:https://[^/\s@]+/|ssh://(?:[\w.-]+@)?[^/\s@]+/|[\w.-]+@[^:/\s]+:)[\w.-]+/[\w.-]+?(?:\.git)?/?$",
        )
        .expect("slug regex")
    });
    if url.is_empty()
        || url.starts_with('-')
        || url.contains("::")
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    // A local bare repo runs its own hooks on a push: only the tests use one.
    slug.is_match(url) || (cfg!(test) && url.starts_with('/') && Path::new(url).is_dir())
}

/// The git command for a network call: only the https / ssh (and, in tests,
/// local) transports, no hooks, no prompts. The system config stays: it is
/// where e.g. Homebrew git keeps the credential helper (osxkeychain) that
/// https remotes need.
fn net_command(args: &[&str], dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "protocol.ssh.allow=always",
        "-c",
        "protocol.file.allow=always",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
    ])
    .args(args)
    .current_dir(dir)
    .env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    cmd
}

/// A network git call to `url`, from a fresh temp folder (an empty repo when
/// `in_repo`) so no repo's config applies, with [`net_command`]'s
/// transports and a timeout. The URL
/// goes after `--end-of-options` in `args`.
fn net_git(url: &str, args: &[&str], in_repo: bool) -> Result<String, String> {
    if url.starts_with("http://") {
        return Err("refused: http:// remotes are not used (use https://)".into());
    }
    if !safe_remote_url(url) {
        return Err(REFUSED_URL.into());
    }
    let tmp = std::env::temp_dir().join(format!("dcv2-net-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        if in_repo {
            git(&tmp, &["init", "-q"])?;
        }
        let cmd = net_command(args, &tmp);
        run_with_timeout(cmd, CALL_TIMEOUT)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

const REFUSED_URL: &str = "refused: not a safe remote URL";

/// The remote branch's tip (`None`: not there).
fn remote_tip(url: &str, branch: &str) -> Result<Option<String>, String> {
    let refname = format!("refs/heads/{branch}");
    net_git(
        url,
        &["ls-remote", "--heads", "--end-of-options", url, &refname],
        false,
    )
    .map(|out| out.split_whitespace().next().map(str::to_string))
}

/// Deletes the remote branch only while it is still at `oid` (a lease:
/// never a plain force, so a commit pushed since is never lost).
pub fn delete_at(url: &str, branch: &str, oid: &str) -> Result<String, String> {
    let lease = format!("--force-with-lease=refs/heads/{branch}:{oid}");
    let target = format!(":refs/heads/{branch}");
    net_git(
        url,
        &["push", &lease, "--end-of-options", url, &target],
        true,
    )
}

/// `https://<host>/<owner>/<name>/pull/<n>`: (host, owner/name).
fn pr_url_parts(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let parts: Vec<&str> = rest.split('/').collect();
    let ok = parts.len() == 5
        && parts[3] == "pull"
        && !parts[4].is_empty()
        && parts[4].chars().all(|c| c.is_ascii_digit())
        && parts[..3].iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
        });
    ok.then(|| {
        (
            parts[0].to_ascii_lowercase(),
            format!("{}/{}", parts[1], parts[2]),
        )
    })
}

/// The host of a remote URL (`https://h/…`, `ssh://git@h/…`, `git@h:…`).
fn remote_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .unwrap_or(url);
    let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty() && url.contains(['/', ':'])).then(|| host.to_ascii_lowercase())
}

/// `.guard.json` `remotes.<PR>`: the push URL and `owner/name` recorded
/// when the worktree was first made.
fn recorded_remote(run_dir: &Path, pr_id: &str) -> Option<(String, Option<String>)> {
    let g = read_json_value(&run_dir.join(".guard.json"))?;
    let r = g.get("remotes")?.get(pr_id)?;
    let url = r.get("url")?.as_str()?.to_string();
    let repo = r.get("repo").and_then(|x| x.as_str()).map(str::to_string);
    (!url.is_empty()).then_some((url, repo))
}

/// The main checkout of a PR: status.json's `repoPath`, only when
/// `.guard.json` lists it as one of the run's repos.
fn recorded_repo(run_dir: &Path, pr: &PrView) -> Option<PathBuf> {
    let want = canonical(Path::new(pr.repo_path.as_deref()?));
    let g = read_json_value(&run_dir.join(".guard.json"))?;
    g.get("repos")?
        .as_array()?
        .iter()
        .filter_map(|r| r.as_str())
        .map(|r| canonical(Path::new(r)))
        .find(|r| *r == want)
}

fn run_id_of(run_dir: &Path) -> String {
    run_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn now_text() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The PRs of a run status.json lists with a `prUrl`.
pub(super) fn prs_with_url(run_dir: &Path) -> (Option<String>, Option<String>, Vec<PrView>) {
    let Some(value) = read_json_value(&run_dir.join("status.json")) else {
        return (None, None, Vec::new());
    };
    let Ok(view) = parse_status(&value) else {
        return (None, None, Vec::new());
    };
    let prs = view
        .prs
        .into_iter()
        .filter(|pr| valid_pr_id(&pr.id) && pr.pr_url.as_deref().is_some_and(|u| !u.is_empty()))
        .collect();
    (view.stage, view.updated_at, prs)
}

/// The run should be asked about: `done`, changed within [`MAX_AGE_DAYS`],
/// and a PR with a `prUrl` has no final record yet.
pub fn wants_cleanup(run_dir: &Path, now: chrono::DateTime<chrono::Utc>) -> bool {
    let (stage, updated, prs) = prs_with_url(run_dir);
    if stage.as_deref() != Some("done") || prs.is_empty() {
        return false;
    }
    let recent = updated
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| now.signed_duration_since(t) <= chrono::Duration::days(MAX_AGE_DAYS));
    if !recent {
        return false;
    }
    let done = read_cleanup(run_dir);
    prs.iter()
        .any(|pr| done.prs.get(&pr.id).is_none_or(|rec| rec.retry))
}

fn untouched(pr: &PrView, url: &str, why: String) -> CleanupRecord {
    CleanupRecord {
        pr_id: pr.id.clone(),
        pr_url: url.to_string(),
        outcome: "untouched".into(),
        note: why,
        at: now_text(),
        ..CleanupRecord::default()
    }
}

/// The recorded remote URL and `owner/name` a PR of the run may be asked
/// about: the PR URL must be `https://<github.com or the remote's host>/<owner/name>/pull/<n>`
/// for the repo `.guard.json` `remotes` recorded. `Err` is the one-line reason it is not.
pub(super) fn pr_target(
    run_dir: &Path,
    pr_id: &str,
    url: &str,
) -> Result<(String, String), String> {
    let Some((remote_url, Some(slug))) = recorded_remote(run_dir, pr_id) else {
        return Err("untouched: no recorded GitHub remote".into());
    };
    let mut hosts = vec!["github.com".to_string()];
    hosts.extend(remote_host(&remote_url));
    match pr_url_parts(url) {
        Some((host, repo)) if repo.eq_ignore_ascii_case(&slug) && hosts.contains(&host) => {
            Ok((remote_url, slug))
        }
        _ => Err(format!("untouched: not a pull request of {slug}")),
    }
}

/// A branch name the app may pass to `gh pr edit --base`: no option, no `..`, no odd characters.
fn safe_base(base: &str) -> bool {
    static REF: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = REF.get_or_init(|| regex::Regex::new(r"^[\w][\w./-]{0,200}$").expect("ref regex"));
    re.is_match(base) && !base.contains("..") && !base.ends_with('/') && !base.ends_with(".lock")
}

/// Before the merged PR's branch goes: every open PR that uses it as its base
/// (a PR stacked on it) is retargeted to the merged PR's own base, and each
/// retarget is checked (deleting the base of an open PR closes it on GitHub).
/// `Ok` when none is left on the branch. `Err` is the one-line reason it is not
/// safe to delete; the caller keeps the branch and asks again later.
fn retarget_stacked(
    lookup: &dyn PrLookup,
    run_dir: &Path,
    pr_id: &str,
    branch: &str,
    slug: &str,
    new_base: &str,
) -> Result<(), String> {
    let open = lookup.stacked_on(branch, slug)?;
    if open.is_empty() {
        return Ok(());
    }
    if !safe_base(new_base) {
        return Err(format!(
            "no usable base to retarget the PRs stacked on it to ({new_base:?})"
        ));
    }
    for stacked in open {
        // The same URL checks as the PR's own: a PR of the recorded repo only.
        pr_target(run_dir, pr_id, &stacked.url)
            .map_err(|_| format!("{} is not a pull request of {slug}", stacked.url))?;
        lookup
            .retarget(&stacked.url, new_base, slug)
            .map_err(|e| format!("could not retarget {} to {new_base}: {e}", stacked.url))?;
    }
    Ok(())
}

/// `prs[].stackedOn` of each PR, read from the raw status.json (a PR with none is not stacked).
fn stacked_on_map(run_dir: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(value) = read_json_value(&run_dir.join("status.json")) else {
        return out;
    };
    for pr in value
        .get("prs")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
    {
        if let (Some(id), Some(parent)) = (
            pr.get("id").and_then(|x| x.as_str()),
            pr.get("stackedOn").and_then(|x| x.as_str()),
        ) {
            if valid_pr_id(id) && valid_pr_id(parent) {
                out.insert(id.to_string(), parent.to_string());
            }
        }
    }
    out
}

/// How far down a stack a PR is (0 = not stacked); a loop stops counting.
fn stack_depth(map: &std::collections::HashMap<String, String>, id: &str) -> usize {
    let mut depth = 0;
    let mut cur = id;
    while let Some(parent) = map.get(cur) {
        depth += 1;
        cur = parent;
        if depth > map.len() {
            break;
        }
    }
    depth
}

/// One PR: `None` when it should be asked about again (open, or GitHub
/// could not be asked), else the final record.
fn clean_pr(run_dir: &Path, pr: &PrView, lookup: &dyn PrLookup) -> Option<CleanupRecord> {
    let url = pr.pr_url.clone()?;
    let run_id = run_id_of(run_dir);
    let branch = janitor::expected_branch(&run_id, &pr.id);
    let (remote_url, slug) = match pr_target(run_dir, &pr.id, &url) {
        Ok(target) => target,
        Err(why) => return Some(untouched(pr, &url, why)),
    };
    let info = match lookup.view(&url, &slug) {
        Ok(info) => info,
        Err(err) => {
            eprintln!("DCTerminalv2: cleanup {run_id} {}: {err}", pr.id);
            return None;
        }
    };
    if info.head_ref_name != branch {
        return Some(untouched(
            pr,
            &url,
            format!(
                "untouched: the PR's head is {}, not {branch}",
                info.head_ref_name
            ),
        ));
    }
    match info.state.as_str() {
        "MERGED" => {}
        "CLOSED" => {
            return Some(untouched(
                pr,
                &url,
                "untouched: closed without merging".into(),
            ))
        }
        _ => return None,
    }
    let oid_ok = info.head_ref_oid.len() == 40
        && info
            .head_ref_oid
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
    let mut rec = CleanupRecord {
        pr_id: pr.id.clone(),
        pr_url: url.clone(),
        at: now_text(),
        ..CleanupRecord::default()
    };
    // The worktree and the local branch: the janitor's own rules.
    let item = janitor::list_worktrees(run_dir)
        .into_iter()
        .find(|item| item.pr_id == pr.id);
    let mut worktree_gone = false;
    match item {
        Some(item) if item.exists => {
            let result = janitor::remove_clean(run_dir, std::slice::from_ref(&pr.id))
                .into_iter()
                .next();
            match result {
                Some(r) if r.removed => {
                    worktree_gone = true;
                    rec.worktree = "worktree removed".into();
                    rec.branch = if r.branch_deleted {
                        format!("branch {branch} deleted")
                    } else {
                        format!("branch {branch} kept (git branch -d: not merged here)")
                    };
                }
                Some(r) => {
                    // Changes or the user's ignored files: asked again later
                    // (they may be cleaned up by hand). A contract problem is final.
                    rec.retry = item.problem.is_none()
                        || item
                            .problem
                            .as_deref()
                            .is_some_and(|p| p.starts_with("has ignored files"));
                    rec.worktree = format!("worktree kept ({})", r.message);
                }
                None => rec.worktree = "worktree kept".into(),
            }
        }
        _ => {
            worktree_gone = true;
            rec.worktree = "worktree already gone".into();
            if let Some(repo) = recorded_repo(run_dir, pr) {
                let exists = git(
                    &repo,
                    &[
                        "show-ref",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{branch}"),
                    ],
                )
                .is_ok();
                if exists {
                    rec.branch = match git(&repo, &["branch", "-d", "--", &branch]) {
                        Ok(_) => format!("branch {branch} deleted"),
                        Err(_) => format!("branch {branch} kept (git branch -d: not merged here)"),
                    };
                }
            }
        }
    }
    // The remote branch: only the run's own, at the merged head, at the
    // recorded URL.
    let mut remote_gone = false;
    rec.remote = if !oid_ok {
        "remote branch kept (GitHub gave no head commit)".into()
    } else {
        match remote_tip(&remote_url, &branch) {
            Err(err) if err.starts_with("refused:") => format!("remote branch kept ({err})"),
            Err(err) => {
                rec.retry = true;
                format!("remote branch kept for now (ls-remote: {err})")
            }
            Ok(None) => {
                remote_gone = true;
                "remote branch already deleted".into()
            }
            Ok(Some(tip)) if tip != info.head_ref_oid => {
                "remote branch kept (its tip is not the merged PR's head)".into()
            }
            Ok(Some(_)) => {
                match retarget_stacked(lookup, run_dir, &pr.id, &branch, &slug, &info.base_ref_name)
                {
                    Err(why) => {
                        // A PR is still based on this branch: deleting it would close that PR.
                        rec.retry = true;
                        format!("remote branch kept for now (not deleted: {why})")
                    }
                    Ok(()) => match delete_at(&remote_url, &branch, &info.head_ref_oid) {
                        Ok(_) => {
                            remote_gone = true;
                            "remote branch deleted".into()
                        }
                        Err(err) if err.contains("remote ref does not exist") => {
                            remote_gone = true;
                            "remote branch already deleted".into()
                        }
                        Err(err) if err.contains("stale info") || err.contains("rejected") => {
                            "remote branch kept (it moved since the merge)".into()
                        }
                        Err(err) => {
                            rec.retry = true;
                            format!("remote branch kept for now: {err}")
                        }
                    },
                }
            }
        }
    };
    rec.outcome = if worktree_gone && remote_gone {
        "cleaned"
    } else {
        "kept"
    }
    .into();
    Some(rec)
}

/// One process-wide lock, so a run is never cleaned twice at once (the
/// sweep and a typed request) and only one `gh` call of the cleanup runs.
pub(super) static CLEANUP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Asks about each PR of the run with a `prUrl` and no final record; saves
/// and logs the new records. Returns them.
pub fn cleanup_run(run_dir: &Path, lookup: &dyn PrLookup) -> Vec<CleanupRecord> {
    let _lock = CLEANUP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (_, _, mut prs) = prs_with_url(run_dir);
    // Parents of a stack first, so a child is retargeted before its parent's branch goes;
    // a run with no `stackedOn` keeps the order of its PRs (the sort is stable).
    let stacks = stacked_on_map(run_dir);
    prs.sort_by_key(|pr| stack_depth(&stacks, &pr.id));
    let mut file = read_cleanup(run_dir);
    let mut new = Vec::new();
    let mut changed = false;
    for pr in &prs {
        let old = file.prs.get(&pr.id).cloned();
        if old.as_ref().is_some_and(|rec| !rec.retry) {
            continue;
        }
        if let Some(mut rec) = clean_pr(run_dir, pr, lookup) {
            changed = true;
            // What an earlier ask already did stays said.
            if let Some(o) = &old {
                if rec.remote == "remote branch already deleted"
                    && o.remote == "remote branch deleted"
                {
                    rec.remote = o.remote.clone();
                }
            }
            file.prs.insert(rec.pr_id.clone(), rec.clone());
            // A retry that ends the same way is saved, not logged again.
            if old.is_none_or(|o| o.line() != rec.line() || o.retry != rec.retry) {
                new.push(rec);
            }
        }
    }
    if changed {
        file.schema = 1;
        if let Err(err) = write_json_atomic(&run_dir.join(CLEANUP_FILE), &file) {
            eprintln!("DCTerminalv2: {CLEANUP_FILE}: {err}");
        }
    }
    if !new.is_empty() {
        let lines: Vec<String> = new.iter().map(CleanupRecord::line).collect();
        let _ = append_app_log(run_dir, &format!("cleanup {}", lines.join(" | ")));
    }
    new
}

/// One pass over the app's own runs (adopted ones are the dev app's): each
/// run that [`wants_cleanup`] and was not asked about within `ask_every`.
/// Nothing at all when the setting is off.
pub fn sweep(
    data: &Path,
    enabled: bool,
    lookup: &dyn PrLookup,
    asked: &mut std::collections::HashMap<PathBuf, Instant>,
    ask_every: Duration,
) -> Vec<CleanupRecord> {
    if !enabled {
        return Vec::new();
    }
    let now = chrono::Utc::now();
    let mut out = Vec::new();
    for run in super::folder::list_runs(data) {
        if run.adopted || run.stage.as_deref() != Some("done") {
            continue;
        }
        let dir = PathBuf::from(&run.run_dir);
        if asked.get(&dir).is_some_and(|at| at.elapsed() < ask_every) {
            continue;
        }
        if !wants_cleanup(&dir, now) {
            continue;
        }
        asked.insert(dir.clone(), Instant::now());
        out.extend(cleanup_run(&dir, lookup));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::git::testing::{repo_at, run};
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const RUN: &str = "r-20261027-0900-abcd";

    #[derive(Default)]
    struct Fake {
        prs: HashMap<String, Result<PrInfo, String>>,
        calls: RefCell<Vec<String>>,
        /// Open PRs whose base is a branch (branch -> their URLs).
        stacked: RefCell<HashMap<String, Vec<String>>>,
        /// `retarget` fails with this.
        retarget_fails: RefCell<Option<String>>,
        /// (url, new base) of each retarget that worked.
        retargeted: RefCell<Vec<(String, String)>>,
    }

    impl PrLookup for Fake {
        fn view(&self, url: &str, repo: &str) -> Result<PrInfo, String> {
            self.calls.borrow_mut().push(format!("{repo} {url}"));
            self.prs
                .get(url)
                .cloned()
                .unwrap_or_else(|| Err("no such PR".into()))
        }
        fn stacked_on(&self, branch: &str, repo: &str) -> Result<Vec<StackedPr>, String> {
            self.calls
                .borrow_mut()
                .push(format!("{repo} list-base {branch}"));
            Ok(self
                .stacked
                .borrow()
                .get(branch)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|url| StackedPr {
                    url,
                    base_ref_name: branch.to_string(),
                })
                .collect())
        }
        fn retarget(&self, url: &str, base: &str, repo: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("{repo} retarget {url} {base}"));
            if let Some(err) = self.retarget_fails.borrow().clone() {
                return Err(err);
            }
            self.retargeted.borrow_mut().push((url.into(), base.into()));
            for urls in self.stacked.borrow_mut().values_mut() {
                urls.retain(|u| u != url);
            }
            Ok(())
        }
    }

    fn head(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).unwrap()
    }

    fn url(n: u32) -> String {
        format!("https://github.com/o/r/pull/{n}")
    }

    fn info(state: &str, n: u32, oid: &str) -> Result<PrInfo, String> {
        Ok(PrInfo {
            state: state.into(),
            head_ref_name: format!("dcv2/{RUN}-pr{n}"),
            head_ref_oid: oid.into(),
            base_ref_name: "main".into(),
        })
    }

    struct Setup {
        _dir: TempDir,
        data: PathBuf,
        run_dir: PathBuf,
        repo: PathBuf,
        origin: PathBuf,
        tips: HashMap<u32, String>,
    }

    impl Setup {
        fn wt(&self, n: u32) -> PathBuf {
            self.repo
                .parent()
                .unwrap()
                .join("r-worktrees")
                .join(format!("dcv2-{RUN}-pr{n}"))
        }
        fn on_remote(&self, n: u32) -> bool {
            git(
                &self.origin,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/dcv2/{RUN}-pr{n}"),
                ],
            )
            .is_ok()
        }
        fn branch(&self, n: u32) -> bool {
            git(
                &self.repo,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/dcv2/{RUN}-pr{n}"),
                ],
            )
            .is_ok()
        }
    }

    /// A repo with a local bare remote and one worktree per PR id (each
    /// with a commit, pushed); status.json at `stage` with every PR opened.
    fn setup(stage: &str, ids: &[u32], local_only: &[u32]) -> Setup {
        let dir = TempDir::new("cleanup");
        let data = dir.join("data");
        let repo = dir.join("ws/r");
        repo_at(&repo);
        std::fs::write(repo.join(".gitignore"), "*.local\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "ignore"]);
        let repo = canonical(&repo);
        let origin = dir.join("origin.git");
        run(
            dir.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        let origin = canonical(&origin);
        let run_dir = data.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut tips = HashMap::new();
        let mut prs = Vec::new();
        let mut worktrees = serde_json::Map::new();
        let mut remotes = serde_json::Map::new();
        for &n in ids {
            let branch = format!("dcv2/{RUN}-pr{n}");
            let wt = repo
                .parent()
                .unwrap()
                .join("r-worktrees")
                .join(format!("dcv2-{RUN}-pr{n}"));
            run(
                &repo,
                &["worktree", "add", "-q", "-b", &branch, wt.to_str().unwrap()],
            );
            std::fs::write(wt.join(format!("f{n}.txt")), "x").unwrap();
            run(&wt, &["add", "."]);
            run(&wt, &["commit", "-q", "-m", "S"]);
            run(&wt, &["push", "-q", origin.to_str().unwrap(), &branch]);
            tips.insert(n, head(&wt));
            let id = format!("PR{n}");
            let pr_url = if local_only.contains(&n) {
                None
            } else {
                Some(url(n))
            };
            prs.push(json!({"id": id, "repoPath": repo, "branch": branch, "worktree": wt,
                            "state": if pr_url.is_some() { "opened" } else { "local-only" }, "prUrl": pr_url}));
            worktrees.insert(id.clone(), json!(canonical(&wt)));
            remotes.insert(id, json!({"name": "origin", "url": origin, "repo": "o/r"}));
        }
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "runId": RUN, "stage": stage, "updatedAt": now_text(), "prs": prs})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            run_dir.join(".guard.json"),
            json!({"schema": 1, "runDir": run_dir, "repos": [repo], "worktrees": worktrees, "remotes": remotes}).to_string(),
        )
        .unwrap();
        std::fs::write(run_dir.join("request.md"), "folder: /x\n").unwrap();
        Setup {
            _dir: dir,
            data,
            run_dir,
            repo,
            origin,
            tips,
        }
    }

    fn by<'a>(recs: &'a [CleanupRecord], id: &str) -> &'a CleanupRecord {
        recs.iter().find(|r| r.pr_id == id).unwrap()
    }

    #[test]
    fn two_prs_of_one_repo_are_cleaned_one_at_a_time() {
        // PR1 and PR2 share a repo: the merged one goes, the open one is untouched.
        let s = setup("done", &[1, 2], &[]);
        run(
            &s.repo,
            &["merge", "-q", "--no-edit", &format!("dcv2/{RUN}-pr1")],
        );
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        fake.prs.insert(url(2), info("OPEN", 2, &s.tips[&2]));
        let recs = cleanup_run(&s.run_dir, &fake);
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(by(&recs, "PR1").outcome, "cleaned");
        assert!(!s.wt(1).exists() && !s.branch(1) && !s.on_remote(1));
        assert!(s.wt(2).exists() && s.branch(2) && s.on_remote(2));
    }

    #[test]
    fn a_merged_pr_is_cleaned_and_others_are_left_alone() {
        // PR1 merged (and its merge pulled into main), PR2 open, PR3 closed,
        // PR4 merged but the remote tip moved, PR5 merged with a dirty
        // worktree, PR6 local-only, PR7 merged but GitHub's head is another branch.
        let s = setup("done", &[1, 2, 3, 4, 5, 6, 7], &[6]);
        run(
            &s.repo,
            &["merge", "-q", "--no-edit", &format!("dcv2/{RUN}-pr1")],
        );
        std::fs::write(s.wt(5).join("f5.txt"), "wip").unwrap();
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        fake.prs.insert(url(2), info("OPEN", 2, &s.tips[&2]));
        fake.prs.insert(url(3), info("CLOSED", 3, &s.tips[&3]));
        fake.prs.insert(url(4), info("MERGED", 4, &"0".repeat(40)));
        fake.prs.insert(url(5), info("MERGED", 5, &s.tips[&5]));
        fake.prs.insert(
            url(7),
            Ok(PrInfo {
                state: "MERGED".into(),
                head_ref_name: "feature/other".into(),
                head_ref_oid: s.tips[&7].clone(),
                base_ref_name: "main".into(),
            }),
        );
        assert!(wants_cleanup(&s.run_dir, chrono::Utc::now()));
        let recs = cleanup_run(&s.run_dir, &fake);
        let ids: Vec<&str> = recs.iter().map(|r| r.pr_id.as_str()).collect();
        assert_eq!(ids, ["PR1", "PR3", "PR4", "PR5", "PR7"], "{recs:?}");

        let r1 = by(&recs, "PR1");
        assert_eq!(r1.outcome, "cleaned", "{r1:?}");
        assert_eq!(r1.worktree, "worktree removed");
        assert!(r1.branch.contains("deleted"), "{r1:?}");
        assert_eq!(r1.remote, "remote branch deleted");
        assert!(!s.wt(1).exists() && !s.branch(1) && !s.on_remote(1));

        // Open: no record, asked again later; nothing touched.
        assert!(s.wt(2).exists() && s.branch(2) && s.on_remote(2));
        let r3 = by(&recs, "PR3");
        assert_eq!(r3.outcome, "untouched");
        assert!(s.wt(3).exists() && s.on_remote(3));

        // Tip mismatch: the worktree (clean) goes, the remote branch stays;
        // the branch is not merged here, so `git branch -d` keeps it.
        let r4 = by(&recs, "PR4");
        assert_eq!(r4.outcome, "kept");
        assert!(r4.remote.contains("tip is not"), "{r4:?}");
        assert!(r4.branch.contains("kept"), "{r4:?}");
        assert!(s.on_remote(4) && s.branch(4) && !s.wt(4).exists());

        // Dirty: kept for now (asked again), but the merged remote branch goes.
        let r5 = by(&recs, "PR5");
        assert_eq!(r5.outcome, "kept");
        assert!(r5.retry, "{r5:?}");
        assert!(r5.worktree.contains("kept"), "{r5:?}");
        assert!(s.wt(5).join("f5.txt").exists() && !s.on_remote(5));
        assert!(!by(&recs, "PR4").retry, "a moved tip is final");

        // Not this run's head: untouched, never deleted.
        assert_eq!(by(&recs, "PR7").outcome, "untouched");
        assert!(s.wt(7).exists() && s.on_remote(7));
        // Local-only: never asked about.
        assert!(!fake.calls.borrow().iter().any(|c| c.contains("/pull/6")));
        assert!(s.wt(6).exists());

        // Recorded and logged once; the open PR and the dirty one are asked
        // again, and a retry that ends the same way is not logged again.
        let file = read_cleanup(&s.run_dir);
        assert_eq!(file.prs.len(), 5);
        let log = std::fs::read_to_string(s.run_dir.join("log.md")).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(
            log.contains(" done app: cleanup PR1: worktree removed"),
            "{log}"
        );
        fake.calls.borrow_mut().clear();
        assert!(cleanup_run(&s.run_dir, &fake).is_empty());
        assert_eq!(
            fake.calls.borrow().as_slice(),
            [format!("o/r {}", url(2)), format!("o/r {}", url(5))]
        );
        assert_eq!(
            std::fs::read_to_string(s.run_dir.join("log.md"))
                .unwrap()
                .lines()
                .count(),
            1
        );

        // The user cleaned PR5's worktree, and PR2 was merged: both go on
        // the next ask; then the run is finished.
        run(&s.wt(5), &["checkout", "--", "f5.txt"]);
        fake.prs.insert(url(2), info("MERGED", 2, &s.tips[&2]));
        let later = cleanup_run(&s.run_dir, &fake);
        assert_eq!(later.len(), 2, "{later:?}");
        assert!(!s.wt(2).exists() && !s.on_remote(2) && !s.wt(5).exists());
        assert_eq!(by(&later, "PR5").outcome, "cleaned");
        assert!(!wants_cleanup(&s.run_dir, chrono::Utc::now()));
    }

    #[test]
    fn forged_urls_never_run_and_network_failures_are_asked_again() {
        let s = setup("done", &[1, 2], &[]);
        let pwn = s.run_dir.join("PWN");
        let guard_path = s.run_dir.join(".guard.json");
        let mut g: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&guard_path).unwrap()).unwrap();
        // PR1: an option as the URL; PR2: a folder that is not a repo (ls-remote fails).
        g["remotes"]["PR1"]["url"] = json!(format!("--upload-pack=touch {};false", pwn.display()));
        let not_repo = s.run_dir.join("not-a-repo");
        std::fs::create_dir_all(&not_repo).unwrap();
        g["remotes"]["PR2"]["url"] = json!(not_repo);
        std::fs::write(&guard_path, g.to_string()).unwrap();
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        fake.prs.insert(url(2), info("MERGED", 2, &s.tips[&2]));
        let recs = cleanup_run(&s.run_dir, &fake);
        assert!(!pwn.exists(), "the URL never ran as an option");
        let r1 = by(&recs, "PR1");
        assert!(r1.remote.contains(REFUSED_URL) && !r1.retry, "{r1:?}");
        let r2 = by(&recs, "PR2");
        assert!(r2.retry && r2.remote.contains("for now"), "{r2:?}");
        assert!(s.on_remote(1) && s.on_remote(2));
        assert!(
            wants_cleanup(&s.run_dir, chrono::Utc::now()),
            "PR2 again later"
        );
        for bad in [
            "--upload-pack=x",
            "ext::sh -c x",
            "https://github.com/o/r x",
            "file:///tmp/x",
            "relative/path",
            "https://github.com/o",
        ] {
            assert!(!safe_remote_url(bad), "{bad}");
        }
        assert!(remote_tip("http://github.com/o/r.git", "x")
            .unwrap_err()
            .contains("http:// remotes are not used"));
        for good in [
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@ghe.example/o/r",
        ] {
            assert!(safe_remote_url(good), "{good}");
        }
    }

    #[test]
    fn network_git_keeps_the_system_config_and_allows_only_safe_transports() {
        // The system config holds Homebrew git's credential helper
        // (osxkeychain): https pushes fail without it.
        let cmd = net_command(
            &["ls-remote", "--end-of-options", "https://github.com/o/r"],
            Path::new("/"),
        );
        let envs: Vec<String> = cmd
            .get_envs()
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();
        assert!(
            !envs.iter().any(|k| k.starts_with("GIT_CONFIG")),
            "{envs:?}"
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"protocol.allow=never".to_string()));
        assert!(!args
            .iter()
            .any(|a| a.contains("protocol.http.allow") || a.contains("protocol.ext")));
    }

    #[test]
    fn the_remote_delete_is_a_lease_on_the_merged_head() {
        let s = setup("done", &[1], &[]);
        let origin = s.origin.to_str().unwrap();
        let branch = format!("dcv2/{RUN}-pr1");
        // Someone pushed after the merge: the delete at the old head is refused.
        assert!(delete_at(origin, &branch, &"1".repeat(40)).is_err());
        assert!(s.on_remote(1));
        assert!(delete_at(origin, &branch, &s.tips[&1]).is_ok());
        assert!(!s.on_remote(1));
    }

    #[test]
    fn github_errors_ask_again_and_foreign_urls_or_remotes_are_untouched() {
        let s = setup("done", &[1, 2], &[]);
        // PR1: gh fails (network): no record. PR2: the URL is another repo's.
        let status = s.run_dir.join("status.json");
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&status).unwrap()).unwrap();
        v["prs"][1]["prUrl"] = json!("https://github.com/evil/r/pull/2");
        std::fs::write(&status, v.to_string()).unwrap();
        let fake = Fake::default();
        let recs = cleanup_run(&s.run_dir, &fake);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].pr_id, "PR2");
        assert_eq!(recs[0].outcome, "untouched");
        assert!(recs[0].note.contains("not a pull request of o/r"));
        assert_eq!(fake.calls.borrow().len(), 1, "only PR1 is asked");
        assert!(s.wt(1).exists() && s.wt(2).exists() && s.on_remote(2));
        assert!(
            wants_cleanup(&s.run_dir, chrono::Utc::now()),
            "PR1 again later"
        );
    }

    #[test]
    fn the_sweep_skips_unfinished_old_and_switched_off_runs() {
        let s = setup("shipping", &[1], &[]);
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        let mut asked = HashMap::new();
        // Not done yet.
        assert!(sweep(&s.data, true, &fake, &mut asked, ASK_EVERY).is_empty());
        let status = s.run_dir.join("status.json");
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&status).unwrap()).unwrap();
        v["stage"] = json!("done");
        v["updatedAt"] = json!("2026-01-01T00:00:00Z");
        std::fs::write(&status, v.to_string()).unwrap();
        // Done but older than 30 days.
        assert!(sweep(&s.data, true, &fake, &mut asked, ASK_EVERY).is_empty());
        v["updatedAt"] = json!(now_text());
        std::fs::write(&status, v.to_string()).unwrap();
        // The setting off: nothing, not even a gh call.
        assert!(sweep(&s.data, false, &fake, &mut asked, ASK_EVERY).is_empty());
        assert!(fake.calls.borrow().is_empty());
        assert!(s.wt(1).exists());
        // On: cleaned, then never asked again.
        let recs = sweep(&s.data, true, &fake, &mut asked, Duration::ZERO);
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert!(!s.wt(1).exists() && !s.on_remote(1));
        assert!(sweep(&s.data, true, &fake, &mut asked, Duration::ZERO).is_empty());
        // One `gh pr view`, and the one `gh pr list --base` that precedes the branch delete.
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls[0].contains("/pull/1") && calls[1].contains("list-base"),
            "{calls:?}"
        );
    }

    #[test]
    fn a_kept_ignored_file_keeps_the_worktree_and_urls_parse_strictly() {
        let s = setup("done", &[1], &[]);
        std::fs::write(s.wt(1).join("notes.local"), "mine").unwrap();
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        let recs = cleanup_run(&s.run_dir, &fake);
        assert_eq!(recs[0].outcome, "kept");
        assert!(recs[0].worktree.contains("notes.local"), "{:?}", recs[0]);
        assert!(s.wt(1).join("notes.local").exists());

        assert_eq!(
            pr_url_parts("https://github.com/o/r/pull/12"),
            Some(("github.com".into(), "o/r".into()))
        );
        for bad in [
            "http://github.com/o/r/pull/1",
            "https://github.com/o/r/pull/1/files",
            "https://github.com/o/r/pull/x",
            "https://github.com/o/r/issues/1",
            "https://github.com/o r/r/pull/1",
        ] {
            assert_eq!(pr_url_parts(bad), None, "{bad}");
        }
        assert_eq!(
            remote_host("git@github.example:o/r.git").as_deref(),
            Some("github.example")
        );
        assert_eq!(
            remote_host("https://x@ghe.example/o/r").as_deref(),
            Some("ghe.example")
        );
        assert_eq!(remote_host("/local/bare.git"), None);
    }
    /// Records `stackedOn` for these PRs in the run's status file: (child, parent).
    fn stack(s: &Setup, pairs: &[(&str, &str)]) {
        let path = s.run_dir.join("status.json");
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for pr in v["prs"].as_array_mut().unwrap() {
            for (child, parent) in pairs {
                if pr["id"] == *child {
                    pr["stackedOn"] = json!(parent);
                }
            }
        }
        std::fs::write(&path, v.to_string()).unwrap();
    }

    fn stacked_fake(s: &Setup, parent_state: &str) -> Fake {
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info(parent_state, 1, &s.tips[&1]));
        fake.prs.insert(url(2), info("OPEN", 2, &s.tips[&2]));
        fake.stacked
            .borrow_mut()
            .insert(format!("dcv2/{RUN}-pr1"), vec![url(2)]);
        fake
    }

    #[test]
    fn a_stacked_pr_is_retargeted_before_its_parents_branch_is_deleted() {
        let s = setup("done", &[1, 2], &[]);
        stack(&s, &[("PR2", "PR1")]);
        run(
            &s.repo,
            &["merge", "-q", "--no-edit", &format!("dcv2/{RUN}-pr1")],
        );
        let fake = stacked_fake(&s, "MERGED");
        let recs = cleanup_run(&s.run_dir, &fake);
        let r1 = by(&recs, "PR1");
        assert_eq!(r1.outcome, "cleaned", "{r1:?}");
        assert_eq!(r1.remote, "remote branch deleted");
        assert_eq!(
            fake.retargeted.borrow().as_slice(),
            [(url(2), "main".to_string())],
            "the child goes to the merged PR's own base"
        );
        assert!(
            !s.on_remote(1),
            "the parent's branch goes after the retarget"
        );
        // The stacked PR is open: asked again later, nothing of it touched.
        assert!(s.wt(2).exists() && s.branch(2) && s.on_remote(2));
        // A second pass: the retargeted PR is no longer on the branch; the finished record is not asked again.
        fake.calls.borrow_mut().clear();
        assert!(cleanup_run(&s.run_dir, &fake).is_empty());
        assert_eq!(fake.retargeted.borrow().len(), 1);
        assert!(!fake.calls.borrow().iter().any(|c| c.contains("retarget")));
    }

    #[test]
    fn a_failed_retarget_keeps_the_branch_records_why_and_is_tried_again() {
        let s = setup("done", &[1, 2], &[]);
        stack(&s, &[("PR2", "PR1")]);
        run(
            &s.repo,
            &["merge", "-q", "--no-edit", &format!("dcv2/{RUN}-pr1")],
        );
        let fake = stacked_fake(&s, "MERGED");
        *fake.retarget_fails.borrow_mut() = Some("gh pr edit --base: boom".into());
        let recs = cleanup_run(&s.run_dir, &fake);
        let r1 = by(&recs, "PR1");
        assert_eq!(r1.outcome, "kept", "{r1:?}");
        assert!(r1.retry, "tried again on the next sweep: {r1:?}");
        assert!(
            r1.remote.contains("not deleted") && r1.remote.contains("boom"),
            "{r1:?}"
        );
        assert!(
            s.on_remote(1),
            "the branch an open PR is based on is never deleted"
        );
        assert!(fake.retargeted.borrow().is_empty());
        assert!(wants_cleanup(&s.run_dir, chrono::Utc::now()), "asked again");
        // The next sweep: the retarget works, then the branch goes, and the record is final.
        *fake.retarget_fails.borrow_mut() = None;
        let recs = cleanup_run(&s.run_dir, &fake);
        let r1 = by(&recs, "PR1");
        assert_eq!(r1.outcome, "cleaned", "{r1:?}");
        assert_eq!(r1.remote, "remote branch deleted");
        assert!(!r1.retry);
        assert_eq!(fake.retargeted.borrow().len(), 1);
        assert!(!s.on_remote(1));
        assert!(
            cleanup_run(&s.run_dir, &fake).is_empty(),
            "a second run is a no-op"
        );
    }

    #[test]
    fn a_parent_closed_without_merge_is_not_swept_and_nothing_is_retargeted() {
        let s = setup("done", &[1, 2], &[]);
        stack(&s, &[("PR2", "PR1")]);
        let fake = stacked_fake(&s, "CLOSED");
        let recs = cleanup_run(&s.run_dir, &fake);
        assert_eq!(by(&recs, "PR1").outcome, "untouched");
        assert!(fake.retargeted.borrow().is_empty());
        assert!(
            !fake.calls.borrow().iter().any(|c| c.contains("list-base")),
            "{:?}",
            fake.calls.borrow()
        );
        assert!(s.on_remote(1) && s.on_remote(2));
    }

    #[test]
    fn a_stacked_pr_that_is_not_the_recorded_repos_or_an_unsafe_base_keeps_the_branch() {
        let s = setup("done", &[1, 2], &[]);
        run(
            &s.repo,
            &["merge", "-q", "--no-edit", &format!("dcv2/{RUN}-pr1")],
        );
        let fake = stacked_fake(&s, "MERGED");
        fake.stacked.borrow_mut().insert(
            format!("dcv2/{RUN}-pr1"),
            vec!["https://github.com/evil/other/pull/9".into()],
        );
        let recs = cleanup_run(&s.run_dir, &fake);
        let r1 = by(&recs, "PR1");
        assert_eq!(r1.outcome, "kept", "{r1:?}");
        assert!(r1.remote.contains("not a pull request of o/r"), "{r1:?}");
        assert!(fake.retargeted.borrow().is_empty() && s.on_remote(1));

        let s = setup("done", &[1, 2], &[]);
        let mut fake = stacked_fake(&s, "MERGED");
        fake.prs.insert(
            url(1),
            Ok(PrInfo {
                base_ref_name: "--delete".into(),
                ..info("MERGED", 1, &s.tips[&1]).unwrap()
            }),
        );
        let recs = cleanup_run(&s.run_dir, &fake);
        assert!(
            by(&recs, "PR1").remote.contains("no usable base"),
            "{recs:?}"
        );
        assert!(fake.retargeted.borrow().is_empty() && s.on_remote(1));
    }

    #[test]
    fn a_stack_is_cleaned_parent_first_and_an_old_run_keeps_its_order() {
        // PR1 is stacked on PR2 (the child has the lower id): the parent is asked about first.
        let s = setup("done", &[1, 2], &[]);
        stack(&s, &[("PR1", "PR2")]);
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        fake.prs.insert(url(2), info("MERGED", 2, &s.tips[&2]));
        let recs = cleanup_run(&s.run_dir, &fake);
        let ids: Vec<&str> = recs.iter().map(|r| r.pr_id.as_str()).collect();
        assert_eq!(ids, ["PR2", "PR1"], "{recs:?}");
        // No stackedOn anywhere: today's order.
        let s = setup("done", &[1, 2], &[]);
        let mut fake = Fake::default();
        fake.prs.insert(url(1), info("MERGED", 1, &s.tips[&1]));
        fake.prs.insert(url(2), info("MERGED", 2, &s.tips[&2]));
        let recs = cleanup_run(&s.run_dir, &fake);
        let ids: Vec<&str> = recs.iter().map(|r| r.pr_id.as_str()).collect();
        assert_eq!(ids, ["PR1", "PR2"], "{recs:?}");
    }
}
