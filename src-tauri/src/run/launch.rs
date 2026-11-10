//! The dcv2 launch (docs/launch.md): env, flags and first prompt for a
//! Claude tab, and where the plugin lives. `CLAUDE_CONFIG_DIR` comes from
//! the tab's account (the provider), never from here. The app never sets
//! `DCV2_TEST_MODE` and drops an inherited one.

use crate::provider::ProgramArgs;
use crate::store::settings_store::{valid_orchestrator_mode, valid_orchestrator_model};
use crate::store::TabLaunch;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Explicit plugin folder (tests, a plugin checkout elsewhere).
pub const PLUGIN_DIR_ENV: &str = "DCTV2_PLUGIN_DIR";
pub const TEST_MODE_ENV: &str = "DCV2_TEST_MODE";
/// The tab's Claude account id (the window's account), read by the plugin's ClickUp helper to pick the keychain item.
pub const ACCOUNT_ID_ENV: &str = "DCV2_ACCOUNT_ID";

/// The role tiles: (skill, label, group). The start screen shows the groups in first-seen order:
/// `pipeline` (the roles of the Eagle-Eye pipeline) and `persona` (QA Engineer, Product Manager).
pub const ROLES: &[(&str, &str, &str)] = &[
    ("planner", "Planner", "pipeline"),
    ("plan-reviewer", "Plan Reviewer", "pipeline"),
    ("risk-reviewer", "Risk Reviewer", "pipeline"),
    ("developer", "Developer", "pipeline"),
    ("pr-reviewer", "PR Reviewer", "pipeline"),
    ("recommendation", "Recommendation", "pipeline"),
    ("codebase-audit", "Codebase Audit", "pipeline"),
    ("onboard", "Onboard", "pipeline"),
    ("qa-engineer", "QA Engineer", "persona"),
    ("product-manager", "Product Manager", "persona"),
];

pub fn role_label(role: &str) -> Option<&'static str> {
    ROLES
        .iter()
        .find(|(id, _, _)| *id == role)
        .map(|(_, label, _)| *label)
}

/// A folder with `.claude-plugin/plugin.json`.
pub fn is_plugin_dir(dir: &Path) -> bool {
    dir.join(".claude-plugin").join("plugin.json").is_file()
}

/// The plugin folder: `DCTV2_PLUGIN_DIR`, else the bundled resource
/// (`<resources>/plugin`), else the repo's `plugin/` (dev builds).
pub fn resolve_plugin_dir(
    env_override: Option<&str>,
    resource_dir: Option<&Path>,
    dev_repo: &Path,
) -> Option<PathBuf> {
    let candidates = env_override
        .filter(|v| !v.trim().is_empty())
        .map(|v| PathBuf::from(v.trim()))
        .into_iter()
        .chain(resource_dir.map(|r| r.join("plugin")))
        .chain(std::iter::once(dev_repo.join("plugin")));
    candidates
        .filter(|dir| is_plugin_dir(dir))
        .map(|dir| dir.canonicalize().unwrap_or(dir))
        .next()
}

/// The repo root of this source tree (dev builds find `plugin/` there).
pub fn dev_repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// `plugin/VERSION`'s first line.
pub fn plugin_version(plugin: &Path) -> Option<String> {
    let text = std::fs::read_to_string(plugin.join("VERSION")).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.chars().take(80).collect())
}

/// `plugin/launch/settings.json` when present.
pub fn launch_settings_file(plugin: &Path) -> Option<PathBuf> {
    let file = plugin.join("launch").join("settings.json");
    file.is_file().then_some(file)
}

#[derive(Debug, Clone)]
pub struct LaunchInput {
    pub role: String,
    pub prompt: String,
    pub plugin_dir: PathBuf,
    pub memory_dir: PathBuf,
    /// The window's Claude account id (`DCV2_ACCOUNT_ID`).
    pub account_id: String,
    /// Eagle-Eye only.
    pub run_dir: Option<PathBuf>,
    pub max_implementers: u32,
    pub stop_hooks: bool,
    /// `DCV2_SLOP_CHECK` (the Stop hook's slop check; `run.slopCheck`).
    pub slop_check: bool,
    pub settings_file: Option<PathBuf>,
    pub memory_prompt: Option<PathBuf>,
    /// `--model` of an Eagle-Eye tab (run folder set); never added to a role tab.
    pub orchestrator_model: Option<String>,
    /// `--permission-mode` of an Eagle-Eye tab (run folder set); never `bypassPermissions`.
    pub orchestrator_mode: Option<String>,
}

/// Sets `DCV2_ACCOUNT_ID` in an env list: drops any earlier value, then pushes the given one.
pub fn with_account_id(env: &mut Vec<(String, String)>, account_id: &str) {
    env.retain(|(k, _)| k != ACCOUNT_ID_ENV);
    env.push((ACCOUNT_ID_ENV.to_string(), account_id.to_string()));
}

/// The tab's launch record (docs/launch.md "Command").
pub fn launch_spec(input: &LaunchInput) -> TabLaunch {
    let mut env = vec![(
        "DCV2_MEMORY_DIR".to_string(),
        input.memory_dir.display().to_string(),
    )];
    with_account_id(&mut env, &input.account_id);
    if let Some(run) = &input.run_dir {
        env.push(("DCV2_RUN_DIR".into(), run.display().to_string()));
        env.push((
            "DCV2_MAX_IMPLEMENTERS".into(),
            input.max_implementers.to_string(),
        ));
    }
    env.push((
        "DCV2_STOP_HOOKS".into(),
        if input.stop_hooks { "on" } else { "off" }.into(),
    ));
    env.push((
        "DCV2_SLOP_CHECK".into(),
        if input.slop_check { "on" } else { "off" }.into(),
    ));
    let mut args = vec![
        "--plugin-dir".to_string(),
        input.plugin_dir.display().to_string(),
    ];
    if let Some(prompt) = &input.memory_prompt {
        args.push("--append-system-prompt-file".into());
        args.push(prompt.display().to_string());
    }
    // Only an Eagle-Eye tab (it has a run folder) is pinned; a role or plain tab stays a plain claude.
    // A value outside the allow-list (so never `bypassPermissions`) adds no flag.
    if input.run_dir.is_some() {
        if let Some(model) = input
            .orchestrator_model
            .as_deref()
            .filter(|m| !m.is_empty() && valid_orchestrator_model(m))
        {
            args.push("--model".into());
            args.push(model.to_string());
        }
        if let Some(mode) = input
            .orchestrator_mode
            .as_deref()
            .filter(|m| !m.is_empty() && valid_orchestrator_mode(m))
        {
            args.push("--permission-mode".into());
            args.push(mode.to_string());
        }
    }
    TabLaunch {
        role: input.role.clone(),
        prompt: input.prompt.clone(),
        env,
        args,
        settings_file: input
            .settings_file
            .as_ref()
            .map(|f| f.display().to_string()),
        run_dir: input.run_dir.as_ref().map(|r| r.display().to_string()),
    }
}

/// The first prompt for a role tile: `/dcv2:<role> <request>` on one line.
pub fn role_prompt(role: &str, request: &str) -> String {
    let request = request.split_whitespace().collect::<Vec<_>>().join(" ");
    if request.is_empty() {
        format!("/dcv2:{role}")
    } else {
        format!("/dcv2:{role} {request}")
    }
}

/// One `--settings` value: the plugin's launch settings (a copy, never the
/// file itself) with the app's own keys laid over. `env` merges name by
/// name and `hooks` event by event (the app's hook lists are appended), so
/// neither side's entries are lost.
pub fn merged_settings(launch_file: Option<&str>, app: Option<Value>) -> Option<Value> {
    let base = launch_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object);
    match (base, app) {
        (None, app) => app,
        (Some(base), None) => Some(base),
        (Some(mut base), Some(Value::Object(app))) => {
            if let Some(map) = base.as_object_mut() {
                for (key, value) in app {
                    let merged = match (key.as_str(), map.remove(&key), value) {
                        ("env", Some(Value::Object(mut mine)), Value::Object(theirs)) => {
                            mine.extend(theirs);
                            Value::Object(mine)
                        }
                        ("hooks", Some(Value::Object(mut mine)), Value::Object(theirs)) => {
                            for (event, list) in theirs {
                                let next = match (mine.remove(&event), list) {
                                    (Some(Value::Array(mut a)), Value::Array(b)) => {
                                        a.extend(b);
                                        Value::Array(a)
                                    }
                                    (_, list) => list,
                                };
                                mine.insert(event, next);
                            }
                            Value::Object(mine)
                        }
                        (_, _, value) => value,
                    };
                    map.insert(key, merged);
                }
            }
            Some(base)
        }
        (Some(base), Some(_)) => Some(base),
    }
}

/// The launch flags, without an `--append-system-prompt-file` whose file
/// is gone (pruned or deleted): Claude would refuse to start.
fn present_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--append-system-prompt-file" {
            if let Some(file) = iter.next() {
                if Path::new(file).is_file() {
                    out.push(arg.clone());
                    out.push(file.clone());
                }
            }
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// The prompt for a fresh session. An Eagle-Eye tab started fresh again
/// (reopened, or "start fresh" after an exit) on a run that already has a
/// status.json resumes that run instead of starting it twice.
pub fn fresh_prompt(launch: &TabLaunch) -> String {
    if let Some(run) = launch.run_dir.as_deref() {
        let started = Path::new(run).join("status.json").is_file();
        if started && launch.prompt.starts_with("/dcv2:eagle-eye run=") {
            return format!("/dcv2:eagle-eye resume run={run}");
        }
    }
    launch.prompt.clone()
}

/// Adds a launch's flags, env and (on a fresh start) its prompt to the
/// command. `account_id` is the tab's current window account: like
/// `CLAUDE_CONFIG_DIR` it is set again at every spawn, whatever the record
/// holds (a window can move to another account; older records have none).
/// Returns the env names to drop from the inherited environment.
pub fn apply_launch(
    command: &mut ProgramArgs,
    launch: Option<&TabLaunch>,
    resumed: bool,
    account_id: &str,
) -> Vec<String> {
    let Some(launch) = launch else {
        return Vec::new();
    };
    command.args.extend(present_args(&launch.args));
    if !resumed && !launch.prompt.trim().is_empty() {
        command.args.push(fresh_prompt(launch));
    }
    command.env.extend(
        launch
            .env
            .iter()
            .filter(|(k, _)| k != TEST_MODE_ENV && k != "CLAUDE_CONFIG_DIR" && k != ACCOUNT_ID_ENV)
            .cloned(),
    );
    with_account_id(&mut command.env, account_id);
    vec![TEST_MODE_ENV.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;

    fn input(run: Option<&str>) -> LaunchInput {
        LaunchInput {
            role: "eagle-eye".into(),
            prompt: "/dcv2:eagle-eye run=/d/runs/r1".into(),
            plugin_dir: "/app/plugin".into(),
            memory_dir: "/d/memory".into(),
            account_id: "acct-1".into(),
            run_dir: run.map(PathBuf::from),
            max_implementers: 2,
            stop_hooks: false,
            slop_check: false,
            settings_file: None,
            memory_prompt: Some("/d/runs/r1/memory-prompt.md".into()),
            orchestrator_model: Some("opus".into()),
            orchestrator_mode: Some("auto".into()),
        }
    }

    #[test]
    fn eagle_eye_gets_the_run_env_and_roles_do_not() {
        let spec = launch_spec(&input(Some("/d/runs/r1")));
        let get = |k: &str| {
            spec.env
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("DCV2_RUN_DIR"), Some("/d/runs/r1"));
        assert_eq!(get("DCV2_MEMORY_DIR"), Some("/d/memory"));
        assert_eq!(get("DCV2_MAX_IMPLEMENTERS"), Some("2"));
        assert_eq!(get("DCV2_STOP_HOOKS"), Some("off"));
        assert_eq!(get("DCV2_SLOP_CHECK"), Some("off"));
        assert_eq!(get("DCV2_TEST_MODE"), None);
        assert_eq!(
            spec.args,
            [
                "--plugin-dir",
                "/app/plugin",
                "--append-system-prompt-file",
                "/d/runs/r1/memory-prompt.md",
                "--model",
                "opus",
                "--permission-mode",
                "auto"
            ]
        );
        let role = launch_spec(&LaunchInput {
            memory_prompt: None,
            ..input(None)
        });
        assert!(role
            .env
            .iter()
            .all(|(k, _)| k != "DCV2_RUN_DIR" && k != "DCV2_MAX_IMPLEMENTERS"));
        assert_eq!(role.args, ["--plugin-dir", "/app/plugin"]);
        assert_eq!(
            role_prompt("planner", "  add\nmultiply  "),
            "/dcv2:planner add multiply"
        );
        assert_eq!(role_prompt("onboard", ""), "/dcv2:onboard");
    }

    fn account_ids(env: &[(String, String)]) -> Vec<&str> {
        env.iter()
            .filter(|(k, _)| k == ACCOUNT_ID_ENV)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn the_faked_backends_roles_are_the_roles_in_order() {
        // AD-12: the start screen's render test counts tiles from this fixture, so it must not drift from ROLES.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake/runs.json");
        let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let faked: Vec<(String, String, String)> = fixture["run_overview"]["roles"]
            .as_array()
            .expect("run_overview.roles")
            .iter()
            .map(|r| {
                let field = |k: &str| r[k].as_str().unwrap_or_default().to_string();
                (field("id"), field("label"), field("group"))
            })
            .collect();
        let wanted: Vec<(String, String, String)> = ROLES
            .iter()
            .map(|(id, label, group)| (id.to_string(), label.to_string(), group.to_string()))
            .collect();
        assert_eq!(faked, wanted);
    }

    #[test]
    fn eight_pipeline_roles_come_before_the_two_personas() {
        let groups: Vec<&str> = ROLES.iter().map(|(_, _, g)| *g).collect();
        assert_eq!(groups.len(), 10);
        assert!(groups[..8].iter().all(|g| *g == "pipeline"));
        assert!(groups[8..].iter().all(|g| *g == "persona"));
        assert_eq!(role_label("qa-engineer"), Some("QA Engineer"));
        assert_eq!(role_label("nope"), None);
    }

    #[test]
    fn every_launch_record_carries_the_window_account() {
        for run in [Some("/d/runs/r1"), None] {
            let spec = launch_spec(&input(run));
            assert_eq!(account_ids(&spec.env), ["acct-1"], "{run:?}");
        }
        let resume = launch_spec(&LaunchInput {
            role: "eagle-eye-resume".into(),
            account_id: "acct-2".into(),
            ..input(Some("/d/runs/r1"))
        });
        assert_eq!(account_ids(&resume.env), ["acct-2"]);
    }

    #[test]
    fn with_account_id_replaces_a_stale_value() {
        let mut env = vec![
            ("A".to_string(), "1".to_string()),
            (ACCOUNT_ID_ENV.to_string(), "old".to_string()),
        ];
        with_account_id(&mut env, "new");
        assert_eq!(account_ids(&env), ["new"]);
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn the_spawn_sets_the_tabs_current_account_whatever_the_record_holds() {
        let mut spec = launch_spec(&input(None));
        spec.env.retain(|(k, _)| k != ACCOUNT_ID_ENV);
        // A record made before the account id existed: it is added.
        let mut command = ProgramArgs::new("claude", vec![]);
        apply_launch(&mut command, Some(&spec), true, "new");
        assert_eq!(account_ids(&command.env), ["new"]);
        // A stale value in the record (the window moved to another account): exactly one, the current one.
        spec.env.push((ACCOUNT_ID_ENV.into(), "old".into()));
        let mut command = ProgramArgs::new("claude", vec![]);
        apply_launch(&mut command, Some(&spec), false, "new");
        assert_eq!(account_ids(&command.env), ["new"]);
        // A plain tab (no launch record) gets none.
        let mut plain = ProgramArgs::new("claude", vec![]);
        apply_launch(&mut plain, None, false, "new");
        assert!(account_ids(&plain.env).is_empty());
    }

    #[test]
    fn only_an_eagle_eye_tab_is_pinned_and_never_to_bypass_permissions() {
        let flag = |spec: &TabLaunch, name: &str| {
            spec.args
                .iter()
                .position(|a| a == name)
                .map(|i| spec.args[i + 1].clone())
        };
        let eagle = launch_spec(&input(Some("/d/runs/r1")));
        assert_eq!(flag(&eagle, "--model").as_deref(), Some("opus"));
        assert_eq!(flag(&eagle, "--permission-mode").as_deref(), Some("auto"));
        // A role tab carries neither flag even when the values are given.
        let role = launch_spec(&input(None));
        assert_eq!(
            role.args,
            [
                "--plugin-dir",
                "/app/plugin",
                "--append-system-prompt-file",
                "/d/runs/r1/memory-prompt.md"
            ]
        );
        // None adds no flag.
        let none = launch_spec(&LaunchInput {
            orchestrator_model: None,
            orchestrator_mode: None,
            ..input(Some("/d/runs/r1"))
        });
        assert_eq!(flag(&none, "--model"), None);
        assert_eq!(flag(&none, "--permission-mode"), None);
        for bad in ["bypassPermissions", "dontAsk", "plan", ""] {
            let spec = launch_spec(&LaunchInput {
                orchestrator_mode: Some(bad.into()),
                ..input(Some("/d/runs/r1"))
            });
            assert_eq!(flag(&spec, "--permission-mode"), None, "{bad}");
            assert!(!spec.args.iter().any(|a| a == "bypassPermissions"));
        }
        let spec = launch_spec(&LaunchInput {
            orchestrator_model: Some("gpt-4".into()),
            ..input(Some("/d/runs/r1"))
        });
        assert_eq!(flag(&spec, "--model"), None);
    }

    #[test]
    fn a_resumed_tab_keeps_env_and_flags_but_not_the_prompt() {
        let spec = launch_spec(&input(Some("/d/runs/r1")));
        let mut fresh = ProgramArgs::new("claude", vec!["--session-id".into(), "x".into()]);
        let drop = apply_launch(&mut fresh, Some(&spec), false, "acct-1");
        assert_eq!(drop, ["DCV2_TEST_MODE"]);
        assert_eq!(fresh.args.last().unwrap(), "/dcv2:eagle-eye run=/d/runs/r1");
        assert!(
            !fresh
                .args
                .iter()
                .any(|a| a == "--append-system-prompt-file"),
            "the prompt file does not exist, so the flag is dropped"
        );
        let mut resumed = ProgramArgs::new("claude", vec![]);
        apply_launch(&mut resumed, Some(&spec), true, "acct-1");
        assert!(!resumed.args.iter().any(|a| a.starts_with("/dcv2")));
        assert!(resumed.env.iter().any(|(k, _)| k == "DCV2_RUN_DIR"));
        let mut plain = ProgramArgs::new("claude", vec![]);
        assert!(apply_launch(&mut plain, None, false, "acct-1").is_empty());

        let dir = TempDir::new("fresh_prompt");
        let run = dir.join("r1");
        std::fs::create_dir_all(&run).unwrap();
        let spec = launch_spec(&LaunchInput {
            prompt: format!("/dcv2:eagle-eye run={}", run.display()),
            ..input(run.to_str())
        });
        assert_eq!(fresh_prompt(&spec), spec.prompt);
        std::fs::write(run.join("status.json"), "{}").unwrap();
        assert_eq!(
            fresh_prompt(&spec),
            format!("/dcv2:eagle-eye resume run={}", run.display())
        );
        assert!(plain.args.is_empty());
    }

    #[test]
    fn launch_settings_are_merged_with_the_apps_status_line() {
        let dir = TempDir::new("launch_settings");
        let file = dir.join("settings.json");
        std::fs::write(
            &file,
            r#"{"env":{"BASH_MAX_OUTPUT_LENGTH":"30000"},"statusLine":{"type":"x"}}"#,
        )
        .unwrap();
        let app = json!({"statusLine": {"type": "command", "command": "dct"}});
        let merged = merged_settings(file.to_str(), Some(app.clone())).unwrap();
        assert_eq!(merged["env"]["BASH_MAX_OUTPUT_LENGTH"], "30000");
        assert_eq!(merged["statusLine"]["command"], "dct");
        assert_eq!(merged_settings(None, Some(app.clone())), Some(app.clone()));
        assert_eq!(merged_settings(Some("/missing.json"), None), None);
        assert!(
            std::fs::read_to_string(&file).unwrap().contains("\"x\""),
            "the plugin file is not changed"
        );
    }

    #[test]
    fn the_plugin_folder_is_found_by_override_resource_then_repo() {
        let dir = TempDir::new("plugin_dir");
        let make = |p: &Path| {
            std::fs::create_dir_all(p.join(".claude-plugin")).unwrap();
            std::fs::write(p.join(".claude-plugin/plugin.json"), "{}").unwrap();
        };
        let repo = dir.join("repo");
        make(&repo.join("plugin"));
        let res = dir.join("res");
        assert_eq!(
            resolve_plugin_dir(None, Some(&res), &repo).unwrap(),
            repo.join("plugin").canonicalize().unwrap()
        );
        make(&res.join("plugin"));
        assert_eq!(
            resolve_plugin_dir(None, Some(&res), &repo).unwrap(),
            res.join("plugin").canonicalize().unwrap()
        );
        let other = dir.join("other");
        make(&other);
        assert_eq!(
            resolve_plugin_dir(other.to_str(), Some(&res), &repo).unwrap(),
            other.canonicalize().unwrap()
        );
        assert!(resolve_plugin_dir(None, None, &dir.join("nope")).is_none());
        std::fs::write(repo.join("plugin/VERSION"), "abc123\nmore\n").unwrap();
        assert_eq!(
            plugin_version(&repo.join("plugin")).as_deref(),
            Some("abc123")
        );
        assert!(launch_settings_file(&repo.join("plugin")).is_none());
    }

    #[test]
    fn the_repo_plugin_is_a_real_plugin() {
        assert!(is_plugin_dir(&dev_repo_root().join("plugin")));
    }
}
