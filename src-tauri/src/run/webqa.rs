//! Web QA (docs/run-folder.md, "Web QA mode"): the app's side of a black-box test pass of a deployed web app.
//!
//! - **targets**: the operator's list in Settings (`settings.json` `webQa.targets`), saved and checked here. The test
//!   account's password never enters settings.json: it is in the macOS keychain (service `dcterminalv2.webqa`, account =
//!   target id), behind [`PasswordStore`] so tests use memory and never touch the real keychain.

use crate::clickup::secret::keychain_error;
use crate::store::settings_store::{WebQaSettings, WebQaTarget};
use crate::store::SettingsStore;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// Keychain item service; the item's account field is the target id.
pub const KEYCHAIN_SERVICE: &str = "dcterminalv2.webqa";
/// A password longer than this is refused (it is typed by the driver, not pasted into a form by a person).
pub const MAX_PASSWORD: usize = 200;

/// One password slot per target id. Errors are plain English and never hold the password.
pub trait PasswordStore: Send + Sync {
    fn get(&self, target_id: &str) -> Result<Option<String>, String>;
    fn set(&self, target_id: &str, password: &str) -> Result<(), String>;
    /// Clearing an empty slot is fine.
    fn clear(&self, target_id: &str) -> Result<(), String>;
}

/// The OS keychain, through the `keyring` crate.
pub struct KeychainPasswords;

impl KeychainPasswords {
    fn entry(target_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, target_id).map_err(|err| keychain_error(&err))
    }
}

impl PasswordStore for KeychainPasswords {
    fn get(&self, target_id: &str) -> Result<Option<String>, String> {
        match Self::entry(target_id)?.get_password() {
            Ok(saved) => Ok(Some(saved)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(keychain_error(&err)),
        }
    }

    fn set(&self, target_id: &str, password: &str) -> Result<(), String> {
        Self::entry(target_id)?
            .set_password(password)
            .map_err(|err| keychain_error(&err))
    }

    fn clear(&self, target_id: &str) -> Result<(), String> {
        match Self::entry(target_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(keychain_error(&err)),
        }
    }
}

/// In-memory slots for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryPasswords {
    pub slots: Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl PasswordStore for MemoryPasswords {
    fn get(&self, target_id: &str) -> Result<Option<String>, String> {
        Ok(self
            .slots
            .lock()
            .map_err(|e| e.to_string())?
            .get(target_id)
            .cloned())
    }

    fn set(&self, target_id: &str, password: &str) -> Result<(), String> {
        self.slots
            .lock()
            .map_err(|e| e.to_string())?
            .insert(target_id.to_string(), password.to_string());
        Ok(())
    }

    fn clear(&self, target_id: &str) -> Result<(), String> {
        self.slots
            .lock()
            .map_err(|e| e.to_string())?
            .remove(target_id);
        Ok(())
    }
}

/// Whether a target's test password is saved, or why that cannot be told.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PasswordStatus {
    pub target_id: String,
    pub saved: bool,
    pub problem: Option<String>,
}

fn known_target(settings: &SettingsStore, target_id: &str) -> Result<(), String> {
    if settings.web_qa().targets.iter().any(|t| t.id == target_id) {
        Ok(())
    } else {
        Err("No web QA target with that id. Save the target first.".into())
    }
}

fn known_target_in_app(app: &AppHandle, target_id: &str) -> Result<(), String> {
    let settings = app.state::<Mutex<SettingsStore>>();
    let settings = settings.lock().map_err(|e| e.to_string())?;
    known_target(&settings, target_id)
}

/// A password the driver can type: one line, not empty, not control characters.
pub fn clean_password(password: &str) -> Result<&str, String> {
    if password.is_empty()
        || password.chars().count() > MAX_PASSWORD
        || password.chars().any(char::is_control)
    {
        return Err(format!(
            "The password must be one line of 1 to {MAX_PASSWORD} characters."
        ));
    }
    Ok(password)
}

pub fn password_status(store: &dyn PasswordStore, target_id: &str) -> PasswordStatus {
    match store.get(target_id) {
        Ok(saved) => PasswordStatus {
            target_id: target_id.into(),
            saved: saved.is_some(),
            problem: None,
        },
        Err(problem) => PasswordStatus {
            target_id: target_id.into(),
            saved: false,
            problem: Some(problem),
        },
    }
}

/// Saves the target list. A target that is gone loses its keychain password too (best effort).
pub fn save_targets(
    settings: &mut SettingsStore,
    store: &dyn PasswordStore,
    next: Vec<WebQaTarget>,
) -> Result<WebQaSettings, String> {
    let before: Vec<String> = settings
        .web_qa()
        .targets
        .iter()
        .map(|t| t.id.clone())
        .collect();
    let saved = settings.set_web_qa_targets(next)?;
    for gone in before
        .iter()
        .filter(|id| !saved.targets.iter().any(|t| &t.id == *id))
    {
        let _ = store.clear(gone);
    }
    Ok(saved)
}

#[tauri::command]
pub async fn webqa_targets_save(
    app: AppHandle,
    targets: Vec<WebQaTarget>,
) -> Result<WebQaSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = app.state::<Mutex<SettingsStore>>();
        let mut settings = settings.lock().map_err(|e| e.to_string())?;
        save_targets(&mut settings, &KeychainPasswords, targets)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn webqa_password_status(
    app: AppHandle,
    target_id: String,
) -> Result<PasswordStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        known_target_in_app(&app, &target_id)?;
        Ok(password_status(&KeychainPasswords, &target_id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn webqa_password_set(
    app: AppHandle,
    target_id: String,
    password: String,
) -> Result<PasswordStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        known_target_in_app(&app, &target_id)?;
        KeychainPasswords.set(&target_id, clean_password(&password)?)?;
        Ok(password_status(&KeychainPasswords, &target_id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn webqa_password_clear(
    app: AppHandle,
    target_id: String,
) -> Result<PasswordStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        known_target_in_app(&app, &target_id)?;
        KeychainPasswords.clear(&target_id)?;
        Ok(password_status(&KeychainPasswords, &target_id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::settings_store::{url_allowed, WebQaCharter};
    use crate::test_support::TempDir;

    fn target(name: &str) -> WebQaTarget {
        WebQaTarget {
            id: String::new(),
            name: name.into(),
            allowlist: vec!["https://app.example.com/".into()],
            username: "tester".into(),
            clickup_list_id: Some("901".into()),
            charters: vec![
                WebQaCharter {
                    id: String::new(),
                    title: "Search".into(),
                    text: "Search for a word".into(),
                    may_change_data: false,
                },
                WebQaCharter {
                    id: String::new(),
                    title: "Sign in".into(),
                    text: "Sign in".into(),
                    may_change_data: true,
                },
            ],
        }
    }

    fn open(dir: &TempDir) -> SettingsStore {
        SettingsStore::open(dir.path()).expect("opens")
    }

    #[test]
    fn a_saved_target_reloads_with_its_urls_and_charters_and_settings_json_holds_no_password() {
        let dir = TempDir::new("webqa-save");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        let saved = save_targets(&mut settings, &store, vec![target("Staging")]).expect("saves");
        assert_eq!(saved.targets.len(), 1);
        let id = saved.targets[0].id.clone();
        assert!(id.starts_with("t-"), "a new target gets an id: {id}");
        store.set(&id, "hunter2-password").unwrap();
        let again = open(&dir);
        let t = &again.web_qa().targets[0];
        assert_eq!(t.allowlist, vec!["https://app.example.com/".to_string()]);
        assert_eq!(t.username, "tester");
        assert_eq!(t.clickup_list_id.as_deref(), Some("901"));
        // Charters are ordered by title and each has an id; the data-change flag is kept as set.
        assert_eq!(
            t.charters
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Search", "Sign in"]
        );
        assert!(t.charters.iter().all(|c| !c.id.is_empty()));
        assert!(!t.charters[0].may_change_data && t.charters[1].may_change_data);
        let file =
            std::fs::read_to_string(dir.path().join("settings.json")).expect("settings.json");
        assert!(
            !file.contains("hunter2-password"),
            "the password is never in settings.json"
        );
        assert!(
            !file.to_lowercase().contains("password"),
            "settings.json has no password field at all"
        );
    }

    #[test]
    fn a_duplicate_name_or_an_invalid_url_refuses_the_save_and_keeps_the_old_list() {
        let dir = TempDir::new("webqa-refuse");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        save_targets(&mut settings, &store, vec![target("Staging")]).expect("saves");
        let err = save_targets(
            &mut settings,
            &store,
            vec![target("Staging"), target("staging")],
        )
        .unwrap_err();
        assert!(err.contains("Names must differ"), "{err}");
        for bad in [
            "ftp://app.example.com/",
            "https://user@app.example.com/",
            "javascript:alert(1)",
            "https://app.example.com/?q=1",
            "app.example.com",
            "",
        ] {
            let mut t = target("Other");
            t.allowlist = vec![bad.into()];
            let err = save_targets(&mut settings, &store, vec![t]).unwrap_err();
            assert!(!err.is_empty(), "{bad} was accepted");
        }
        assert_eq!(settings.web_qa().targets.len(), 1, "the old list is kept");
        assert_eq!(settings.web_qa().targets[0].name, "Staging");
        let mut none = target("No urls");
        none.allowlist.clear();
        assert!(save_targets(&mut settings, &store, vec![none]).is_err());
        let mut no_title = target("No title");
        no_title.charters[0].title = "  ".into();
        assert!(save_targets(&mut settings, &store, vec![no_title]).is_err());
    }

    #[test]
    fn removing_a_target_clears_its_password() {
        let dir = TempDir::new("webqa-remove");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        let saved = save_targets(&mut settings, &store, vec![target("A"), target("B")]).unwrap();
        let (a, b) = (saved.targets[0].id.clone(), saved.targets[1].id.clone());
        store.set(&a, "pw-a").unwrap();
        store.set(&b, "pw-b").unwrap();
        save_targets(
            &mut settings,
            &store,
            saved
                .targets
                .iter()
                .filter(|t| t.id == b)
                .cloned()
                .collect(),
        )
        .unwrap();
        assert_eq!(store.get(&a).unwrap(), None);
        assert_eq!(store.get(&b).unwrap().as_deref(), Some("pw-b"));
    }

    #[test]
    fn the_password_goes_through_the_store_and_only_for_a_saved_target() {
        let dir = TempDir::new("webqa-pw");
        let store = MemoryPasswords::default();
        let mut settings = open(&dir);
        assert!(known_target(&settings, "nope").is_err());
        let saved = save_targets(&mut settings, &store, vec![target("Staging")]).unwrap();
        let id = saved.targets[0].id.clone();
        assert!(known_target(&settings, &id).is_ok());
        assert_eq!(
            password_status(&store, &id),
            PasswordStatus {
                target_id: id.clone(),
                saved: false,
                problem: None
            }
        );
        store
            .set(&id, clean_password("a good password").unwrap())
            .unwrap();
        assert!(password_status(&store, &id).saved);
        store.clear(&id).unwrap();
        assert!(!password_status(&store, &id).saved);
        assert!(clean_password("").is_err());
        assert!(clean_password("two\nlines").is_err());
        assert!(clean_password(&"x".repeat(MAX_PASSWORD + 1)).is_err());
    }

    #[test]
    fn rust_and_the_driver_give_the_same_verdict_on_every_shared_url_case() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/webqa/urls.json"
        ))
        .expect("urls.json");
        let file: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let cases = file["cases"].as_array().expect("cases");
        assert!(cases.len() > 40);
        let mut wrong = Vec::new();
        for case in cases {
            let allowlist: Vec<String> = case["allowlist"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let url = case["url"].as_str().unwrap();
            let want = case["allowed"].as_bool().unwrap();
            if url_allowed(url, &allowlist) != want {
                wrong.push(format!(
                    "{url} for {allowlist:?}: want {want} ({})",
                    case["why"]
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "verdicts differ from the shared cases:\n{}",
            wrong.join("\n")
        );
    }

    #[test]
    fn keychain_service_is_web_qa_s_own() {
        assert_eq!(KEYCHAIN_SERVICE, "dcterminalv2.webqa");
    }
}
