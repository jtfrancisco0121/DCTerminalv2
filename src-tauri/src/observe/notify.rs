//! The sinks of the attention detector (`attention.rs`): the in-app notice
//! (an app-wide `attention` event every window shows), the system
//! notification, the dock badge and the dock bounce, and the run's log.
//!
//! macOS shows no banner while DCTerminalv2 is the frontmost app and
//! ignores the bounce then, so the in-app notice is always sent; the system
//! notification is also always sent (hidden by macOS when the app is in
//! front, which is harmless); the bounce only when the app is in the
//! background.

use super::attention::{Event, Kind, Notifier, RunLogNotifier, Snapshot, TabSnap};
use crate::store::settings_store::NotifySettings;
use crate::store::SettingsStore;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// The app-wide event every window listens to.
pub const ATTENTION_EVENT: &str = "attention";

/// Which sinks get an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sinks {
    pub in_app: bool,
    pub system: bool,
    pub bounce: bool,
}

/// The sinks for an event of `kind`: none when the master switch or the
/// event's own switch is off; else the in-app notice and the system
/// notification always, the bounce only while the app is not active.
pub fn route(kind: Kind, settings: &NotifySettings, app_active: bool) -> Sinks {
    let on = settings.enabled(kind.key());
    Sinks {
        in_app: on,
        system: on,
        bounce: on && !app_active,
    }
}

/// The badge: tabs that need the person, counting only the kinds whose
/// switch is on; 0 when the master switch is off.
pub fn badge_count(snapshot: &Snapshot, settings: &NotifySettings) -> usize {
    snapshot
        .tabs
        .iter()
        .filter(|tab| counts(tab, settings))
        .count()
}

fn counts(tab: &TabSnap, settings: &NotifySettings) -> bool {
    (tab.activity == super::hooks::TabActivity::NeedsYou && settings.enabled(Kind::NeedsYou.key()))
        || (tab.waiting_for.is_some() && settings.enabled(Kind::ApprovalWait.key()))
}

/// What the page receives.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionPayload {
    pub kind: String,
    pub tab_id: Option<String>,
    pub window_label: Option<String>,
    pub run_id: Option<String>,
    pub text: String,
}

impl AttentionPayload {
    pub fn of(event: &Event) -> Self {
        Self {
            kind: event.kind.key().to_string(),
            tab_id: event.tab_id.clone(),
            window_label: event.window_label.clone(),
            run_id: event.run_id.clone(),
            text: event.text(),
        }
    }
}

/// The phone push queue, shared with the "Send test" command.
pub struct PushHandle(pub std::sync::Arc<super::push::PushClient>);

pub struct AppNotifier {
    app: AppHandle,
    push: std::sync::Arc<super::push::PushClient>,
    /// The last badge set, so the main thread is bothered only on a change.
    badge: Mutex<Option<usize>>,
}

impl AppNotifier {
    pub fn new(app: &AppHandle, push: std::sync::Arc<super::push::PushClient>) -> Self {
        Self {
            app: app.clone(),
            push,
            badge: Mutex::new(None),
        }
    }

    fn settings(&self) -> NotifySettings {
        self.app
            .state::<Mutex<SettingsStore>>()
            .lock()
            .map(|s| s.notify().clone())
            .unwrap_or_default()
    }
}

impl Notifier for AppNotifier {
    fn notify(&self, event: &Event, app_active: bool) {
        RunLogNotifier.notify(event, app_active);
        // The phone gets the app-only kinds, whatever the Mac's switches say.
        if let Ok(settings) = self.app.state::<Mutex<SettingsStore>>().lock() {
            self.push.push(event, settings.push());
        }
        let sinks = route(event.kind, &self.settings(), app_active);
        if sinks.in_app {
            let _ = self.app.emit(ATTENTION_EVENT, AttentionPayload::of(event));
        }
        if sinks.system {
            let shown = self
                .app
                .notification()
                .builder()
                .title("DCTerminalv2")
                .body(event.text())
                .show();
            if let Err(err) = shown {
                eprintln!("DCTerminalv2: could not show a notification: {err}");
            }
        }
        if sinks.bounce {
            bounce(&self.app, event.kind.is_urgent());
        }
    }

    fn is_shown(&self, event: &Event) -> bool {
        let sinks = route(event.kind, &self.settings(), false);
        sinks.in_app || sinks.system
    }

    fn badge(&self, snapshot: &Snapshot) {
        let count = badge_count(snapshot, &self.settings());
        let Ok(mut last) = self.badge.lock() else {
            return;
        };
        if *last == Some(count) {
            return;
        }
        *last = Some(count);
        set_badge(&self.app, count);
    }
}

/// Bounces the dock icon (macOS) or flashes the taskbar entry (Windows).
fn bounce(app: &AppHandle, critical: bool) {
    use tauri::UserAttentionType::{Critical, Informational};
    let kind = if critical { Critical } else { Informational };
    if let Some(window) = app.webview_windows().into_values().next() {
        let _ = window.request_user_attention(Some(kind));
    }
}

/// The app-wide badge, set on one existing window on the main thread; 0
/// clears it. macOS only (Windows has no badge count).
#[cfg(target_os = "macos")]
fn set_badge(app: &AppHandle, count: usize) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.webview_windows().into_values().next() {
            let value = (count > 0).then_some(count as i64);
            let _ = window.set_badge_count(value);
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set_badge(_app: &AppHandle, _count: usize) {}

/// "Send a test notification": one system notification and one in-app
/// notice. Errors from the system side come back so Settings can say so.
#[tauri::command]
pub fn send_test_notification(app: AppHandle) -> Result<(), String> {
    let text = "dcv2: this is a test notification";
    let _ = app.emit(
        ATTENTION_EVENT,
        AttentionPayload {
            kind: "test".into(),
            tab_id: None,
            window_label: None,
            run_id: None,
            text: text.into(),
        },
    );
    app.notification()
        .builder()
        .title("DCTerminalv2")
        .body(text)
        .show()
        .map_err(|err| format!("The system refused the notification: {err}"))
}

/// `granted`, `denied` or `prompt`. (Desktop builds of the plugin always
/// report `granted`; macOS decides when the banner is shown.)
#[tauri::command]
pub fn notification_permission(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_notification::PermissionState;
    let state = app
        .notification()
        .permission_state()
        .map_err(|err| err.to_string())?;
    Ok(match state {
        PermissionState::Granted => "granted",
        PermissionState::Denied => "denied",
        _ => "prompt",
    }
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::attention::TabSnap;
    use crate::observe::hooks::TabActivity;

    fn off(kind: Kind) -> NotifySettings {
        let mut s = NotifySettings::default();
        s.events.insert(kind.key().into(), false);
        s
    }

    #[test]
    fn the_in_app_notice_is_sent_with_the_app_active_or_not_and_the_bounce_only_when_inactive() {
        let s = NotifySettings::default();
        let active = route(Kind::NeedsYou, &s, true);
        assert_eq!(
            active,
            Sinks {
                in_app: true,
                system: true,
                bounce: false
            }
        );
        let background = route(Kind::NeedsYou, &s, false);
        assert!(background.in_app && background.system && background.bounce);
    }

    #[test]
    fn nothing_is_produced_when_the_master_or_the_event_switch_is_off() {
        let none = Sinks {
            in_app: false,
            system: false,
            bounce: false,
        };
        let master = NotifySettings {
            master: false,
            ..Default::default()
        };
        for kind in Kind::ALL {
            assert_eq!(
                route(kind, &master, false),
                none,
                "{kind:?} with master off"
            );
        }
        assert_eq!(route(Kind::Stall, &off(Kind::Stall), false), none);
        assert!(route(Kind::NeedsYou, &off(Kind::Stall), false).in_app);
    }

    fn snap(activity: TabActivity, waiting_for: Option<&str>) -> TabSnap {
        TabSnap {
            window_label: "main".into(),
            tab_id: "t".into(),
            label: "t".into(),
            activity,
            run_id: None,
            run_dir: None,
            stage: None,
            waiting_for: waiting_for.map(str::to_string),
            stalled: false,
            prs_done: 0,
            prs_total: 0,
        }
    }

    #[test]
    fn the_badge_counts_tabs_needing_you_and_follows_the_switches() {
        let snapshot = Snapshot {
            tabs: vec![
                snap(TabActivity::NeedsYou, None),
                snap(TabActivity::Waiting, Some("plan-approval")),
                snap(TabActivity::Working, None),
                snap(TabActivity::Waiting, None),
            ],
        };
        let on = NotifySettings::default();
        assert_eq!(badge_count(&snapshot, &on), 2);
        assert_eq!(badge_count(&snapshot, &off(Kind::NeedsYou)), 1);
        assert_eq!(badge_count(&snapshot, &off(Kind::ApprovalWait)), 1);
        let mut master = on.clone();
        master.master = false;
        assert_eq!(
            badge_count(&snapshot, &master),
            0,
            "cleared with the master off"
        );
        assert_eq!(badge_count(&Snapshot::default(), &on), 0);
    }
}
