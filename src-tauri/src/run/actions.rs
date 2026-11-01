//! The PR card's worktree actions (Reveal, Open in editor, Shell here).
//! They act only on a worktree the backend resolves itself from the run id
//! and the PR id and that passes `janitor::verify_worktree`; the page never
//! sends a path. The editor is started without a shell.

use super::{checked_run_dir, data_dir, janitor};
use crate::store::settings_store::DEFAULT_EDITOR;
use crate::store::{SettingsStore, StateStore, TabKind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// `program arg …` split like a shell would for quotes, but never run by
/// one: single and double quotes group, a backslash escapes the next
/// character outside single quotes. Empty input is an error.
pub fn split_command(text: &str) -> Result<(String, Vec<String>), String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                started = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("The editor command has an unclosed quote.".into());
    }
    if started {
        words.push(current);
    }
    let mut words = words.into_iter();
    let program = words
        .next()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "The editor command is empty.".to_string())?;
    Ok((program, words.collect()))
}

/// The editor command line for `worktree`: the setting (or `code`), its
/// arguments, and the worktree last.
pub fn editor_invocation(setting: &str, worktree: &Path) -> Result<(String, Vec<String>), String> {
    let text = if setting.trim().is_empty() {
        DEFAULT_EDITOR
    } else {
        setting
    };
    let (program, mut args) = split_command(text)?;
    args.push(worktree.display().to_string());
    Ok((program, args))
}

fn verified(app: &AppHandle, run_dir: &str, pr_id: &str) -> Result<PathBuf, String> {
    let dir = checked_run_dir(&data_dir(app), run_dir)?;
    janitor::verified_worktree(&dir, pr_id)
}

/// Which PRs of the run have a verified worktree (the card's buttons).
#[tauri::command]
pub async fn pr_worktree_checks(
    app: AppHandle,
    run_dir: String,
) -> Result<Vec<janitor::WorktreeCheck>, String> {
    super::blocking(move || {
        let dir = checked_run_dir(&data_dir(&app), &run_dir)?;
        Ok(janitor::check_worktrees(&dir))
    })
    .await
}

/// `reveal`, `editor` or `shell` on the PR's verified worktree. `shell`
/// opens a Shell tab in `window_id` with that folder and returns it.
#[tauri::command]
pub async fn pr_worktree_action(
    app: AppHandle,
    run_dir: String,
    pr_id: String,
    action: String,
    window_id: Option<String>,
) -> Result<Option<crate::commands::TabSummary>, String> {
    super::blocking(move || {
        let worktree = verified(&app, &run_dir, &pr_id)?;
        match action.as_str() {
            "reveal" => {
                tauri_plugin_opener::reveal_item_in_dir(&worktree)
                    .map_err(|e| format!("Could not reveal the folder: {e}"))?;
                Ok(None)
            }
            "editor" => {
                let setting = app
                    .state::<Mutex<SettingsStore>>()
                    .lock()
                    .map_err(|e| e.to_string())?
                    .ui()
                    .editor_command
                    .clone();
                let (program, args) = editor_invocation(&setting, &worktree)?;
                crate::platform::quiet_command(&program)
                    .args(&args)
                    .current_dir(&worktree)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("Could not start \"{program}\": {e}. Set the editor command in Settings."))?;
                Ok(None)
            }
            "shell" => {
                let state = app.state::<Mutex<StateStore>>();
                let mut state = state.lock().map_err(|e| e.to_string())?;
                state.bind_window(window_id.as_deref());
                crate::commands::add_tab(&mut state, &worktree.display().to_string(), TabKind::Shell)
                    .map(Some)
            }
            _ => Err("unknown action".into()),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::git::testing::{repo_at, run};
    use crate::test_support::TempDir;
    use serde_json::json;

    fn split(text: &str) -> Vec<String> {
        let (program, args) = split_command(text).unwrap();
        std::iter::once(program).chain(args).collect()
    }

    #[test]
    fn the_editor_command_is_split_like_a_shell_would_without_running_one() {
        assert_eq!(split("code"), ["code"]);
        assert_eq!(split("  code   -n  "), ["code", "-n"]);
        assert_eq!(
            split(r#""/Applications/Visual Studio Code.app/Contents/MacOS/code" --wait"#),
            [
                "/Applications/Visual Studio Code.app/Contents/MacOS/code",
                "--wait"
            ]
        );
        assert_eq!(
            split("open -a 'Sublime Text'"),
            ["open", "-a", "Sublime Text"]
        );
        assert_eq!(split(r"my\ editor -x"), ["my editor", "-x"]);
        // Shell syntax is just text: nothing expands or chains.
        assert_eq!(split("code; rm -rf ~"), ["code;", "rm", "-rf", "~"]);
        assert_eq!(split("code $(whoami)"), ["code", "$(whoami)"]);
        assert!(split_command("").is_err());
        assert!(split_command("   ").is_err());
        assert!(split_command("code 'unclosed").is_err());
    }

    #[test]
    fn the_worktree_goes_last_and_an_empty_setting_means_code() {
        let wt = Path::new("/w/ws-worktrees/dcv2-r-1-pr1");
        let (program, args) = editor_invocation("", wt).unwrap();
        assert_eq!(program, "code");
        assert_eq!(args, ["/w/ws-worktrees/dcv2-r-1-pr1"]);
        let (program, args) = editor_invocation("cursor --new-window", wt).unwrap();
        assert_eq!(program, "cursor");
        assert_eq!(args, ["--new-window", "/w/ws-worktrees/dcv2-r-1-pr1"]);
        assert!(editor_invocation("'", wt).is_err());
    }

    const RUN: &str = "r-20261022-1304";

    #[test]
    fn only_a_verified_contract_worktree_is_acted_on() {
        let dir = TempDir::new("pr_actions");
        let repo = dir.join("ws/mathlib");
        repo_at(&repo);
        let wts = dir.join("ws/mathlib-worktrees");
        let good = wts.join(format!("dcv2-{RUN}-pr1"));
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("dcv2/{RUN}-pr1"),
                good.to_str().unwrap(),
            ],
        );
        // A plain folder planted where the contract puts PR2's worktree.
        let planted = wts.join(format!("dcv2-{RUN}-pr2"));
        std::fs::create_dir_all(&planted).unwrap();
        // A real linked worktree, but not where the contract puts it.
        let elsewhere = dir.join("ws/elsewhere");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("dcv2/{RUN}-pr3"),
                elsewhere.to_str().unwrap(),
            ],
        );
        let run_dir = dir.join("runs").join(RUN);
        std::fs::create_dir_all(&run_dir).unwrap();
        let pr = |id: &str, wt: &Path| json!({"id": id, "repoPath": repo, "worktree": wt});
        std::fs::write(
            run_dir.join("status.json"),
            json!({"schema": 1, "stage": "building", "prs": [
                pr("PR1", &good), pr("PR2", &planted), pr("PR3", &elsewhere),
                pr("PR4", &repo)
            ]})
            .to_string(),
        )
        .unwrap();

        let ok = janitor::verified_worktree(&run_dir, "PR1").unwrap();
        assert_eq!(ok, crate::run::git::canonical(&good));
        for id in ["PR2", "PR3", "PR4", "PR9"] {
            assert!(janitor::verified_worktree(&run_dir, id).is_err(), "{id}");
        }
        let checks = janitor::check_worktrees(&run_dir);
        let by = |id: &str| checks.iter().find(|c| c.pr_id == id).unwrap().clone();
        assert!(by("PR1").verified && by("PR1").reason.is_none());
        assert!(!by("PR2").verified && by("PR2").reason.is_some());
        assert!(by("PR3").reason.unwrap().contains("not where"));
        assert!(by("PR4").reason.unwrap().contains("main checkout"));
    }
}
