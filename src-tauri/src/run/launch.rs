//! The dcv2 launch (docs/launch.md): env, flags and first prompt for a
//! Claude tab, and where the plugin lives. `CLAUDE_CONFIG_DIR` comes from
//! the tab's account (the provider), never from here. The app never sets
//! `DCV2_TEST_MODE` and drops an inherited one.

use crate::provider::ProgramArgs;
use crate::store::TabLaunch;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Explicit plugin folder (tests, a plugin checkout elsewhere).
pub const PLUGIN_DIR_ENV: &str = "DCTV2_PLUGIN_DIR";
pub const TEST_MODE_ENV: &str = "DCV2_TEST_MODE";

/// The seven role tiles: (skill, label).
pub const ROLES: &[(&str, &str)] = &[
    ("planner", "Planner"),
    ("plan-reviewer", "Plan Reviewer"),
    ("developer", "Developer"),
    ("pr-reviewer", "PR Reviewer"),
    ("recommendation", "Recommendation"),
    ("codebase-audit", "Codebase Audit"),
    ("onboard", "Onboard"),
];

pub fn role_label(role: &str) -> Option<&'static str> {
    ROLES.iter().find(|(id, _)| *id == role).map(|(_, l)| *l)
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
    /// Eagle-Eye only.
    pub run_dir: Option<PathBuf>,
    pub max_implementers: u32,
    pub stop_hooks: bool,
    pub settings_file: Option<PathBuf>,
    pub memory_prompt: Option<PathBuf>,
}

/// The tab's launch record (docs/launch.md "Command").
pub fn launch_spec(input: &LaunchInput) -> TabLaunch {
    let mut env = vec![(
        "DCV2_MEMORY_DIR".to_string(),
        input.memory_dir.display().to_string(),
    )];
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
    let mut args = vec![
        "--plugin-dir".to_string(),
        input.plugin_dir.display().to_string(),
    ];
    if let Some(prompt) = &input.memory_prompt {
        args.push("--append-system-prompt-file".into());
        args.push(prompt.display().to_string());
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
/// command. Returns the env names to drop from the inherited environment.
pub fn apply_launch(
    command: &mut ProgramArgs,
    launch: Option<&TabLaunch>,
    resumed: bool,
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
            .filter(|(k, _)| k != TEST_MODE_ENV && k != "CLAUDE_CONFIG_DIR")
            .cloned(),
    );
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
            run_dir: run.map(PathBuf::from),
            max_implementers: 2,
            stop_hooks: false,
            settings_file: None,
            memory_prompt: Some("/d/runs/r1/memory-prompt.md".into()),
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
        assert_eq!(get("DCV2_TEST_MODE"), None);
        assert_eq!(
            spec.args,
            [
                "--plugin-dir",
                "/app/plugin",
                "--append-system-prompt-file",
                "/d/runs/r1/memory-prompt.md"
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

    #[test]
    fn a_resumed_tab_keeps_env_and_flags_but_not_the_prompt() {
        let spec = launch_spec(&input(Some("/d/runs/r1")));
        let mut fresh = ProgramArgs::new("claude", vec!["--session-id".into(), "x".into()]);
        let drop = apply_launch(&mut fresh, Some(&spec), false);
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
        apply_launch(&mut resumed, Some(&spec), true);
        assert!(!resumed.args.iter().any(|a| a.starts_with("/dcv2")));
        assert!(resumed.env.iter().any(|(k, _)| k == "DCV2_RUN_DIR"));
        let mut plain = ProgramArgs::new("claude", vec![]);
        assert!(apply_launch(&mut plain, None, false).is_empty());

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
