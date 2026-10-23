//! P3b: watching a dcv2 run from Claude Code's own signals.
//!
//! - A2: the session log tailer (`tail`, `transcript`) and the hook
//!   forwarder (`forward` in the `dcv2-hook` binary, `hooks` in the app).
//! - R9: tokens, $ estimate and minutes per stage (`price`, `transcript`).
//! - R2: the watchdog (`watchdog`) with Nudge and Resume.
//! - A4: the context meter (`transcript`) and check-output trimming (`trim`).
//! - R3: the pinned Claude Code range and the haiku self-test (`version`).
//!
//! Nothing here writes under the account's config folder.

// The `dcv2-hook` binary's code; the app uses its constants and its tests.
#[allow(dead_code)]
pub mod forward;
pub mod hooks;
pub mod price;
pub mod tail;
pub mod transcript;
pub mod trim;
pub mod version;
pub mod watchdog;

use crate::store::{SettingsStore, StateStore};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// The hook listener (None when it could not bind) and one session tracker
/// per tab.
pub struct ObserveState {
    pub hooks: Option<hooks::HookServer>,
    trackers: Mutex<HashMap<String, (String, transcript::SessionTracker)>>,
}

impl ObserveState {
    pub fn start() -> Self {
        let hooks = match hooks::HookServer::start() {
            Ok(server) => Some(server),
            Err(err) => {
                eprintln!("DCTerminalv2: hook listener not started: {err}");
                None
            }
        };
        Self {
            hooks,
            trackers: Mutex::new(HashMap::new()),
        }
    }
}

/// The `hooks` settings value and the env for one dcv2 tab launch. None
/// when the listener or the `dcv2-hook` binary is missing (the tab then
/// runs without per-tab state; nothing else changes).
pub fn launch_hooks(app: &AppHandle, tab_id: &str) -> Option<(Value, Vec<(String, String)>)> {
    let state = app.try_state::<ObserveState>()?;
    let server = state.hooks.as_ref()?;
    let exe = std::env::current_exe().ok();
    let bin = hooks::forwarder_path(
        std::env::var("DCV2_HOOK_BIN").ok().as_deref(),
        exe.as_deref(),
    )?;
    let token = server.register_tab(tab_id);
    let env = vec![
        (forward::TAB_ENV.to_string(), tab_id.to_string()),
        (forward::PORT_ENV.to_string(), server.port.to_string()),
        (forward::TOKEN_ENV.to_string(), token),
    ];
    Some((hooks::hook_settings(&hooks::quoted(&bin)), env))
}

/// The app's `--settings` keys: the status line (when any) plus `hooks`.
pub fn with_hooks(usage: Option<Value>, hooks: Option<&Value>) -> Option<Value> {
    match (usage, hooks) {
        (usage, None) => usage,
        (Some(Value::Object(mut map)), Some(Value::Object(extra))) => {
            map.extend(extra.clone());
            Some(Value::Object(map))
        }
        (_, Some(hooks)) => Some(hooks.clone()),
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TabObservation {
    /// From hook events; None before the first one (or with no forwarder).
    pub hook: Option<hooks::TabHookState>,
    pub hooks_active: bool,
    /// The tab's launch got the hook forwarder (false: `dcv2-hook` missing).
    pub hooks_installed: bool,
    pub transcript_found: bool,
    pub context: Option<transcript::ContextMeter>,
    pub subagents: Vec<transcript::SubagentInfo>,
    pub usage: Option<transcript::UsageSummary>,
    pub stall: Option<watchdog::Stall>,
}

/// status.json's `stage` and `waitingFor`, and when it last changed.
fn status_signals(run_dir: &Path) -> (Option<String>, bool, Option<std::time::SystemTime>) {
    let file = run_dir.join("status.json");
    let changed = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let stage = value
        .as_ref()
        .and_then(|v| v["stage"].as_str().map(str::to_string));
    let waiting = value
        .as_ref()
        .is_some_and(|v| v["waitingFor"].as_str().is_some_and(|w| !w.is_empty()));
    (stage, waiting, changed)
}

fn timeline(run_dir: &Path) -> Vec<(i64, String)> {
    let text = std::fs::read_to_string(run_dir.join("log.md")).unwrap_or_default();
    crate::run::status::parse_log(&text)
        .into_iter()
        .filter_map(|line| {
            let at = transcript::parse_time_ms(line.time.as_deref()?)?;
            Some((at, line.stage?))
        })
        .collect()
}

/// Everything the panel shows for one tab (and its run, when it has one).
pub fn observe(
    tracker: &mut transcript::SessionTracker,
    run_dir: Option<&Path>,
    hook: Option<&hooks::TabHookState>,
    watchdog_minutes: u32,
    now: std::time::SystemTime,
) -> TabObservation {
    tracker.poll();
    let mut out = TabObservation {
        transcript_found: tracker.session_file.is_some(),
        context: tracker.context(),
        subagents: tracker.subagents(),
        ..Default::default()
    };
    if let Some(dir) = run_dir {
        let (stage, waiting, changed) = status_signals(dir);
        let finished = matches!(stage.as_deref(), None | Some("done") | Some("failed"));
        out.usage = Some(tracker.summary(&timeline(dir), stage.as_deref().unwrap_or("start")));
        out.stall = watchdog::check(
            now,
            &watchdog::WatchInput {
                status_changed: changed,
                transcript_written: tracker.last_write,
                waiting_for: waiting,
                finished,
                activity: hook.map(|h| h.activity),
                tool_running: hook.is_some_and(|h| h.tool_running),
                hook_updated: hook.and_then(|h| h.updated_ms).map(|ms| {
                    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
                }),
            },
            watchdog_minutes,
        );
    } else {
        out.usage = Some(tracker.summary(&[], "session"));
    }
    out
}

fn data_dir(app: &AppHandle) -> PathBuf {
    crate::data_dir::app_data_dir(app).path
}

#[tauri::command]
pub async fn observe_tab(app: AppHandle, tab_id: String) -> Result<TabObservation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let tab = {
            let state = app.state::<Mutex<StateStore>>();
            let state = state.lock().map_err(|e| e.to_string())?;
            state.tab_by_id(&tab_id).cloned()
        }
        .ok_or_else(|| format!("unknown tab: {tab_id}"))?;
        let minutes = {
            let settings = app.state::<Mutex<SettingsStore>>();
            let settings = settings.lock().map_err(|e| e.to_string())?;
            settings.run().watchdog_minutes
        };
        let observe_state = app.state::<ObserveState>();
        let hook = observe_state.hooks.as_ref().and_then(|h| h.state(&tab_id));
        let installed = observe_state
            .hooks
            .as_ref()
            .is_some_and(|h| h.has_tab(&tab_id));
        let run_dir = tab
            .launch
            .as_ref()
            .and_then(|l| l.run_dir.as_deref())
            .and_then(|d| crate::run::checked_run_dir(&data_dir(&app), d).ok());
        // A `/clear` (or a resume that forks) moves Claude to a new session;
        // SessionStart's session id says which log to follow.
        let session = hook
            .as_ref()
            .and_then(|h| h.session_id.clone())
            .or_else(|| tab.claude_session_id.clone())
            .unwrap_or_default();
        let mut trackers = observe_state.trackers.lock().map_err(|e| e.to_string())?;
        let fresh = trackers
            .get(&tab_id)
            .is_none_or(|(id, t)| id != &session || t.session_file.is_none());
        if fresh {
            let file = tab.claude_config_dir.as_deref().and_then(|config| {
                crate::claude_session_log::find_session_log(Path::new(config), &session)
            });
            let tracker = file
                .map(transcript::SessionTracker::for_file)
                .unwrap_or_default();
            trackers.insert(tab_id.clone(), (session, tracker));
        }
        let (_, tracker) = trackers.get_mut(&tab_id).expect("inserted");
        let mut out = observe(
            tracker,
            run_dir.as_deref(),
            hook.as_ref(),
            minutes,
            std::time::SystemTime::now(),
        );
        out.hooks_active = hook.is_some();
        out.hooks_installed = installed;
        out.hook = hook;
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A closed tab's token and session tracker go.
pub fn forget_tab(app: &AppHandle, tab_id: &str) {
    if let Some(state) = app.try_state::<ObserveState>() {
        if let Some(hooks) = &state.hooks {
            hooks.forget_tab(tab_id);
        }
        if let Ok(mut trackers) = state.trackers.lock() {
            trackers.remove(tab_id);
        }
    }
}

/// Every tab's hook state, for the tab bar.
#[tauri::command]
pub fn observe_tabs(state: tauri::State<ObserveState>) -> HashMap<String, hooks::TabHookState> {
    state.hooks.as_ref().map(|h| h.states()).unwrap_or_default()
}

/// R2 Nudge: types `continue from status.json`, then Enter, into the tab.
/// Only ever called from your click.
#[tauri::command]
pub async fn nudge_tab(app: AppHandle, tab_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let activity = app
            .state::<ObserveState>()
            .hooks
            .as_ref()
            .and_then(|h| h.state(&tab_id))
            .map(|s| s.activity);
        if let Some(why) = watchdog::nudge_refusal(activity) {
            return Err(why.to_string());
        }
        let registry = app.state::<Mutex<crate::pty::PtyRegistry>>();
        crate::pty::write_to_tab(&registry, &tab_id, watchdog::NUDGE_TEXT.as_bytes())?;
        // Apart, so Claude's input box takes the Enter as a submit, not as
        // part of a paste.
        std::thread::sleep(std::time::Duration::from_millis(150));
        crate::pty::write_to_tab(&registry, &tab_id, b"\r")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeVersionStatus {
    pub version: Option<String>,
    /// `ok`, `below`, `above` or `unknown`.
    pub range: String,
    pub min: String,
    pub max_tested: String,
    pub selftest: Option<version::SelfTest>,
    pub selftest_wanted: bool,
}

fn window_provider(
    app: &AppHandle,
    window_id: Option<&str>,
) -> Result<crate::provider::ClaudeProvider, String> {
    let account = {
        let state = app.state::<Mutex<StateStore>>();
        let state = state.lock().map_err(|e| e.to_string())?;
        let window = window_id
            .map(str::to_string)
            .unwrap_or_else(|| state.focus_window().to_string());
        state.account_for_window(&window)
    };
    let settings = app.state::<Mutex<SettingsStore>>();
    let settings = settings.lock().map_err(|e| e.to_string())?;
    Ok(crate::provider::ClaudeProvider::for_account(
        settings.claude(),
        &account,
    ))
}

#[tauri::command]
pub async fn claude_version_status(
    app: AppHandle,
    window_id: Option<String>,
) -> Result<ClaudeVersionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let provider = window_provider(&app, window_id.as_deref())?;
        let detected = provider.detect().version;
        let last = version::read_selftest(&data_dir(&app));
        Ok(ClaudeVersionStatus {
            range: version::range_state(detected.as_deref()).to_string(),
            selftest_wanted: version::selftest_wanted(detected.as_deref(), last.as_ref()),
            version: detected,
            min: version::version_text(version::CLAUDE_MIN),
            max_tested: version::version_text(version::CLAUDE_MAX_TESTED),
            selftest: last,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

static SELFTEST_RUNNING: Mutex<bool> = Mutex::new(false);

/// R3: one click runs the haiku self-test; the result is kept in app data.
#[tauri::command]
pub async fn run_claude_selftest(
    app: AppHandle,
    window_id: Option<String>,
) -> Result<version::SelfTest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        {
            let mut running = SELFTEST_RUNNING.lock().map_err(|e| e.to_string())?;
            if *running {
                return Err("The self-test is already running.".into());
            }
            *running = true;
        }
        let result = (|| {
            let provider = window_provider(&app, window_id.as_deref())?;
            let program =
                crate::provider::claude_detect::resolve_claude(provider.claude_path.as_deref())
                    .ok_or_else(|| provider.missing_message())?;
            let cli = crate::provider::claude_detect::unwrap_windows_shim(&program);
            let plugin =
                crate::run::plugin_dir_for(&app).ok_or("The dcv2 plugin folder was not found.")?;
            let data = data_dir(&app);
            let work = std::env::temp_dir().join(format!("dcv2-selftest-{}", uuid::Uuid::new_v4()));
            let mut result = version::run_selftest(
                &cli.program,
                &cli.prefix_args,
                &provider.env(),
                &plugin,
                &work,
                version::SELFTEST_TIMEOUT,
            );
            if result.version.is_none() {
                result.version = provider.detect().version;
            }
            version::write_selftest(&data, &result)?;
            Ok(result)
        })();
        if let Ok(mut running) = SELFTEST_RUNNING.lock() {
            *running = false;
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The app's check output of one PR, trimmed (A4).
#[tauri::command]
pub async fn read_check_log(
    app: AppHandle,
    run_dir: String,
    pr_id: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = crate::run::checked_run_dir(&data_dir(&app), &run_dir)?;
        if !crate::run::status::valid_pr_id(&pr_id) {
            return Err("not a PR id".into());
        }
        let file = dir.join("logs").join(format!("{pr_id}-app-check.txt"));
        let bytes = std::fs::read(&file).map_err(|_| "No check output yet.".to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(trim::trim_output(&text, trim::HEAD, trim::TAIL))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::time::{Duration, SystemTime};

    #[test]
    fn hooks_are_laid_over_the_status_line_settings() {
        let hooks = hooks::hook_settings("'/x/dcv2-hook'");
        let usage = json!({"statusLine": {"type": "command"}});
        let merged = with_hooks(Some(usage.clone()), Some(&hooks)).unwrap();
        assert_eq!(merged["statusLine"]["type"], "command");
        assert!(merged["hooks"]["Stop"].is_array());
        assert_eq!(with_hooks(None, Some(&hooks)), Some(hooks.clone()));
        assert_eq!(with_hooks(Some(usage.clone()), None), Some(usage));
        // And the plugin's own hooks survive the launch merge.
        let dir = TempDir::new("observe_merge");
        let file = dir.join("settings.json");
        std::fs::write(
            &file,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"plugin"}]}]}}"#,
        )
        .unwrap();
        let all = crate::run::launch::merged_settings(file.to_str(), Some(hooks)).unwrap();
        assert_eq!(all["hooks"]["Stop"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_quiet_run_shows_a_stall_and_a_waiting_one_does_not() {
        let dir = TempDir::new("observe_run");
        let run = dir.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("status.json"),
            r#"{"stage":"building","waitingFor":null}"#,
        )
        .unwrap();
        std::fs::write(
            run.join("log.md"),
            "2026-10-22T10:00:00Z building PR1 started\n",
        )
        .unwrap();
        let mut tracker = transcript::SessionTracker::default();
        let later = SystemTime::now() + Duration::from_secs(11 * 60);
        let seen = observe(&mut tracker, Some(&run), None, 10, later);
        assert_eq!(seen.stall.unwrap().threshold, 10);
        assert!(!seen.transcript_found);
        assert!(
            observe(&mut tracker, Some(&run), None, 10, SystemTime::now())
                .stall
                .is_none()
        );
        std::fs::write(
            run.join("status.json"),
            r#"{"stage":"plan-approval","waitingFor":"plan-approval"}"#,
        )
        .unwrap();
        assert!(observe(&mut tracker, Some(&run), None, 10, later)
            .stall
            .is_none());
        let asking = hooks::TabHookState {
            activity: hooks::TabActivity::NeedsYou,
            ..Default::default()
        };
        std::fs::write(
            run.join("status.json"),
            r#"{"stage":"building","waitingFor":null}"#,
        )
        .unwrap();
        assert!(observe(&mut tracker, Some(&run), Some(&asking), 10, later)
            .stall
            .is_none());
        assert!(watchdog::nudge_refusal(Some(hooks::TabActivity::NeedsYou)).is_some());
        assert!(watchdog::nudge_refusal(None).is_none());
        std::fs::write(run.join("status.json"), r#"{"stage":"done"}"#).unwrap();
        assert!(observe(&mut tracker, Some(&run), None, 10, later)
            .stall
            .is_none());
    }
}
