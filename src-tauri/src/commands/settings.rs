//! Settings: the terminal, the look, and the Claude accounts (a name and a
//! config folder each). Login checks run off the main thread and without
//! the settings lock: each `claude auth status` can take up to 20 s.

use crate::cli_detect::LoginStatus;
use crate::provider::claude::ClaudeStatus;
use crate::provider::claude_config::{ConfigDirInfo, CONFIG_DIR_ENV};
use crate::provider::claude_detect::{claude_login_status, resolve_claude};
use crate::provider::ClaudeProvider;
use crate::store::settings_store::{resolved_account_config, NotifySettings, ObserveSettings};
use crate::store::{ClaudeSettings, RunSettings, SettingsStore, TerminalSettings, UiSettings};
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub terminal: TerminalSettings,
    pub ui: UiSettings,
    pub claude: ClaudeSettings,
    pub run: RunSettings,
    pub observe: ObserveSettings,
    pub notify: NotifySettings,
    /// ClickUp per-account switches (auto-post after ship approval).
    pub clickup: crate::store::settings_store::ClickUpSettings,
    /// Each saved account with its resolved folder. Login is a separate call.
    pub accounts: Vec<ResolvedClaudeAccount>,
    /// Value of `DCTV2_CLAUDE_CONFIG_DIR` when set (the first account only).
    pub config_dir_env: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedClaudeAccount {
    pub id: String,
    pub name: String,
    pub config: ConfigDirInfo,
    /// True only for the first account while `DCTV2_CLAUDE_CONFIG_DIR` is set.
    pub env_override: bool,
}

fn view(settings: &SettingsStore) -> SettingsView {
    let claude = settings.claude();
    let env = std::env::var(CONFIG_DIR_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let accounts = claude
        .accounts
        .iter()
        .enumerate()
        .map(|(index, account)| ResolvedClaudeAccount {
            id: account.id.clone(),
            name: account.name.clone(),
            config: resolved_account_config(claude, &account.id),
            env_override: index == 0 && env.is_some(),
        })
        .collect();
    SettingsView {
        terminal: settings.terminal().clone(),
        ui: settings.ui().clone(),
        claude: claude.clone(),
        run: settings.run().clone(),
        observe: settings.observe().clone(),
        notify: settings.notify().clone(),
        clickup: settings.clickup().clone(),
        accounts,
        config_dir_env: env,
    }
}

#[tauri::command]
pub fn get_settings(settings: State<Mutex<SettingsStore>>) -> Result<SettingsView, String> {
    let settings = settings.lock().map_err(|e| e.to_string())?;
    Ok(view(&settings))
}

#[tauri::command]
pub fn set_terminal_settings(
    terminal: TerminalSettings,
    settings: State<Mutex<SettingsStore>>,
) -> Result<TerminalSettings, String> {
    let mut settings = settings.lock().map_err(|err| err.to_string())?;
    settings.set_terminal(terminal)?;
    Ok(settings.terminal().clone())
}

#[tauri::command]
pub fn set_ui_settings(
    ui: UiSettings,
    settings: State<Mutex<SettingsStore>>,
) -> Result<UiSettings, String> {
    let mut settings = settings.lock().map_err(|err| err.to_string())?;
    settings.set_ui(ui)?;
    Ok(settings.ui().clone())
}

#[tauri::command]
pub fn set_run_settings(
    run: RunSettings,
    settings: State<Mutex<SettingsStore>>,
) -> Result<RunSettings, String> {
    let mut settings = settings.lock().map_err(|err| err.to_string())?;
    settings.set_run(run)?;
    Ok(settings.run().clone())
}

#[tauri::command]
pub fn set_observe_settings(
    observe: ObserveSettings,
    settings: State<Mutex<SettingsStore>>,
) -> Result<ObserveSettings, String> {
    let mut settings = settings.lock().map_err(|err| err.to_string())?;
    settings.set_observe(observe)?;
    Ok(settings.observe().clone())
}

#[tauri::command]
pub fn set_notify_settings(
    notify: NotifySettings,
    settings: State<Mutex<SettingsStore>>,
) -> Result<NotifySettings, String> {
    let mut settings = settings.lock().map_err(|err| err.to_string())?;
    settings.set_notify(notify)?;
    Ok(settings.notify().clone())
}

/// Save the Claude path and accounts. Open windows are retitled, since an
/// account may have been renamed. A window on a removed account uses the
/// first account from now on.
#[tauri::command]
pub fn set_claude_settings(
    app: tauri::AppHandle,
    claude: ClaudeSettings,
    settings: State<Mutex<SettingsStore>>,
) -> Result<SettingsView, String> {
    let view = {
        let mut settings = settings.lock().map_err(|e| e.to_string())?;
        settings.set_claude(claude)?;
        view(&settings)
    };
    crate::windows::retitle_open_windows(&app);
    Ok(view)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAccountLogin {
    pub id: String,
    pub name: String,
    pub config: ConfigDirInfo,
    pub login: LoginStatus,
}

/// Signed-in email/org for every account. Skips `claude auth status` when
/// that account's folder does not exist, so nothing is created.
#[tauri::command]
pub async fn claude_account_logins(
    settings: State<'_, Mutex<SettingsStore>>,
) -> Result<Vec<ClaudeAccountLogin>, String> {
    let claude = claude_settings(&settings)?;
    tauri::async_runtime::spawn_blocking(move || account_logins(&claude))
        .await
        .map_err(|err| err.to_string())
}

/// Where `claude` is and its version, under the first account.
#[tauri::command]
pub async fn claude_status(
    settings: State<'_, Mutex<SettingsStore>>,
) -> Result<ClaudeStatus, String> {
    let claude = claude_settings(&settings)?;
    let first = claude
        .accounts
        .first()
        .map(|account| account.id.clone())
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        ClaudeProvider::for_account(&claude, &first).detect()
    })
    .await
    .map_err(|err| err.to_string())
}

fn claude_settings(settings: &Mutex<SettingsStore>) -> Result<ClaudeSettings, String> {
    let settings = settings.lock().map_err(|err| err.to_string())?;
    Ok(settings.claude().clone())
}

/// The command's work without an AppHandle, for tests.
#[cfg(all(test, unix))]
fn account_logins_from(settings: &Mutex<SettingsStore>) -> Result<Vec<ClaudeAccountLogin>, String> {
    Ok(account_logins(&claude_settings(settings)?))
}

/// Checks every account at once, so the slowest one sets the wait. Results
/// keep the accounts' order.
fn account_logins(claude: &ClaudeSettings) -> Vec<ClaudeAccountLogin> {
    let claude_bin = resolve_claude(claude.claude_path.as_deref());
    std::thread::scope(|scope| {
        let checks: Vec<_> = claude
            .accounts
            .iter()
            .map(|account| {
                let config = resolved_account_config(claude, &account.id);
                let claude_bin = claude_bin.as_deref();
                let check = {
                    let config = config.clone();
                    scope.spawn(move || claude_login_status(claude_bin, &config))
                };
                (account, config, check)
            })
            .collect();
        checks
            .into_iter()
            .map(|(account, config, check)| ClaudeAccountLogin {
                id: account.id.clone(),
                name: account.name.clone(),
                config,
                login: check.join().unwrap_or_else(|_| LoginStatus {
                    state: "unknown".into(),
                    detail: Some("the login check failed".into()),
                    ..Default::default()
                }),
            })
            .collect()
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::store::settings_store::ClaudeAccount;
    use crate::test_support::TempDir;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Env overrides would point the check at a real binary or folder.
    fn overridden() -> bool {
        ["DCTV2_CLAUDE_PATH", CONFIG_DIR_ENV]
            .iter()
            .any(|name| std::env::var_os(name).is_some())
    }

    /// Settings with a fake `claude` and one config folder per account.
    fn settings_with(dir: &TempDir, script: &str, accounts: &[&str]) -> Mutex<SettingsStore> {
        let fake = dir.join("claude");
        std::fs::write(&fake, script).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut store = SettingsStore::open(&dir.join("data")).unwrap();
        let claude = ClaudeSettings {
            claude_path: Some(fake.display().to_string()),
            accounts: accounts
                .iter()
                .map(|id| {
                    let config = dir.join("accounts").join(id);
                    std::fs::create_dir_all(&config).unwrap();
                    ClaudeAccount {
                        id: id.to_string(),
                        name: id.to_string(),
                        config_dir: Some(config.display().to_string()),
                    }
                })
                .collect(),
        };
        store.set_claude(claude).unwrap();
        // Never let a check fall back to a real `claude` on PATH.
        let claude = store.claude();
        assert_eq!(
            resolve_claude(claude.claude_path.as_deref()),
            Some(fake.clone()),
            "claude resolved to something other than the fake"
        );
        // Never let the fake run against a real Claude folder.
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for account in &claude.accounts {
            let config = resolved_account_config(claude, &account.id);
            assert!(
                std::fs::canonicalize(&config.path).is_ok_and(|path| path.starts_with(&root)),
                "account {} resolved outside the temp dir",
                account.id
            );
        }
        Mutex::new(store)
    }

    /// Creates the file when dropped, including on a failed assert.
    struct ReleaseOnDrop<'a>(&'a Path);

    impl Drop for ReleaseOnDrop<'_> {
        fn drop(&mut self) {
            let _ = std::fs::write(self.0, b"");
        }
    }

    fn wait_for(path: &Path, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    // Regression: the settings lock was held for every account's
    // `claude auth status` (up to 20 s each), freezing Settings.
    #[test]
    fn account_logins_do_not_hold_the_settings_lock() {
        if overridden() {
            eprintln!("ignored: DCTV2_CLAUDE_PATH or DCTV2_CLAUDE_CONFIG_DIR is set");
            return;
        }
        let dir = TempDir::new("logins_lock");
        let started = dir.join("started");
        let release = dir.join("release");
        let script = format!(
            "#!/bin/sh\ntouch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.02; done\nprintf '{{\"loggedIn\":true}}'\n",
            started.display(),
            release.display()
        );
        let settings = settings_with(&dir, &script, &["one"]);
        std::thread::scope(|scope| {
            // Releases the fake even if an assert below fails, so the scope's
            // join never waits out the fake's loop.
            let _release = ReleaseOnDrop(&release);
            let check = scope.spawn(|| account_logins_from(&settings));
            assert!(
                wait_for(&started, Duration::from_secs(5)),
                "fake claude started"
            );
            let free = settings.try_lock().is_ok();
            std::fs::write(&release, b"").unwrap();
            let logins = check.join().unwrap().unwrap();
            assert!(free, "settings lock is free while claude auth status runs");
            assert_eq!(logins[0].login.state, "loggedIn");
        });
    }

    // Regression: accounts were checked one after another, so N stuck
    // accounts froze the window for N x 20 s.
    #[test]
    fn account_logins_run_concurrently() {
        if overridden() {
            eprintln!("ignored: DCTV2_CLAUDE_PATH or DCTV2_CLAUDE_CONFIG_DIR is set");
            return;
        }
        let dir = TempDir::new("logins_parallel");
        // Each account marks itself started, then reports signed in only if
        // the other account started too within 3 s.
        let marks = dir.join("marks");
        std::fs::create_dir_all(&marks).unwrap();
        let script = format!(
            "#!/bin/sh\nm='{m}'\ntouch \"$m/started-$(basename \"$CLAUDE_CONFIG_DIR\")\"\nn=0\nwhile [ $n -lt 150 ]; do\n  if [ -e \"$m/started-one\" ] && [ -e \"$m/started-two\" ]; then printf '{{\"loggedIn\":true}}'; exit 0; fi\n  sleep 0.02; n=$((n+1))\ndone\nprintf '{{\"loggedIn\":false}}'\nexit 1\n",
            m = marks.display()
        );
        let settings = settings_with(&dir, &script, &["one", "two"]);
        let logins = account_logins_from(&settings).unwrap();
        let states: Vec<(&str, &str)> = logins
            .iter()
            .map(|login| (login.id.as_str(), login.login.state.as_str()))
            .collect();
        assert_eq!(states, [("one", "loggedIn"), ("two", "loggedIn")]);
    }

    #[test]
    fn the_view_resolves_every_account() {
        if overridden() {
            eprintln!("ignored: DCTV2_CLAUDE_PATH or DCTV2_CLAUDE_CONFIG_DIR is set");
            return;
        }
        let dir = TempDir::new("settings_view");
        let settings = settings_with(&dir, "#!/bin/sh\n", &["one", "two"]);
        let view = view(&settings.lock().unwrap());
        assert_eq!(view.accounts.len(), 2);
        assert!(view
            .accounts
            .iter()
            .all(|account| account.config.exists && !account.env_override));
        assert!(view.accounts[1].config.path.ends_with("two"));
        assert_eq!(view.config_dir_env, None);
    }
}
