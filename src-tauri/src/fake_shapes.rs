//! The shape of every faked command's answer, taken from its real Rust return type.
//!
//! `npm run dev:fake` answers commands from `fixtures/fake/*.json`. Those answers must not drift from what the
//! Rust commands return. This test holds one sample per faked command, built from the real type (a struct literal,
//! or a JSON literal round-tripped through the type's own `Deserialize`), serializes it with serde and reduces it to
//! a shape: object keys, JSON type, whether a key is nullable (an `Option`) or may be absent
//! (`skip_serializing_if`), and the shape of an array's element. The result is compared with the checked-in
//! `fixtures/fake/shapes.json`, which `src/fake/shapes.test.ts` then holds each fixture against.
//!
//! Renaming a field in a faked type changes the sample, so this test fails until the shape file is regenerated
//! with `DCV2_UPDATE_SHAPES=1 cargo test fake_shapes` (and the fixtures are fixed so `npm test` passes).
//! No ts-rs, no specta: nothing is added to the crate or npm dependencies.

use crate::commands::{
    AppStateSnapshot, ClaudeAccountLogin, ClosedTabSummary, ResolvedClaudeAccount, SettingsView,
    SplitSummary, TabSummary,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A sample answer, and the key paths that are nullable or may be absent.
/// Paths are `key`, `outer.inner`, and `list[].key` for the element of an array.
struct Sample {
    value: Value,
    nullable: &'static [&'static str],
    optional: &'static [&'static str],
    /// Objects whose keys are data (a `BTreeMap`): the shape is the shape of a value, not of the sample's keys.
    maps: &'static [&'static str],
    /// Objects of free-form JSON (`serde_json::Map`): any keys, any values.
    free: &'static [&'static str],
}

fn of<T: Serialize>(value: T) -> Sample {
    Sample {
        value: serde_json::to_value(value).expect("serializes"),
        nullable: &[],
        optional: &[],
        maps: &[],
        free: &[],
    }
}

impl Sample {
    fn nullable(mut self, paths: &'static [&'static str]) -> Self {
        self.nullable = paths;
        self
    }
    fn optional(mut self, paths: &'static [&'static str]) -> Self {
        self.optional = paths;
        self
    }
    fn maps(mut self, paths: &'static [&'static str]) -> Self {
        self.maps = paths;
        self
    }
    fn free(mut self, paths: &'static [&'static str]) -> Self {
        self.free = paths;
        self
    }
}

fn cfg() -> crate::provider::claude_config::ConfigDirInfo {
    crate::provider::claude_config::ConfigDirInfo {
        path: "/h/.claude".into(),
        display: "~/.claude".into(),
        source: crate::provider::claude_config::ConfigDirSource::Default,
        exists: true,
    }
}

fn cost_line() -> crate::observe::transcript::CostLine {
    crate::observe::transcript::CostLine {
        name: "n".into(),
        tokens: crate::observe::price::Tokens::default(),
        total_tokens: 1,
        cost_usd: Some(1.0),
        minutes: 1.0,
    }
}

fn compare_side(id: &str) -> crate::flight::host::CompareSide {
    crate::flight::host::CompareSide {
        run_id: id.into(),
        run_dir: format!("/r/{id}"),
        title: "t".into(),
        outcome: "done".into(),
        indexed: true,
        off: false,
    }
}

fn tab_summary() -> TabSummary {
    TabSummary {
        id: "tab_1".into(),
        label: "Claude".into(),
        custom_label: false,
        cwd: "/w".into(),
        kind: crate::store::TabKind::Claude,
        folder_status: "ok".into(),
        has_session: false,
        run_dir: Some("/r".into()),
        role: Some("eagle-eye".into()),
        split: Some(SplitSummary {
            direction: crate::store::SplitDirection::Right,
            ratio: 0.5,
            id: "tab_2".into(),
            cwd: "/w".into(),
            folder_status: "ok".into(),
        }),
        pending_start: false,
    }
}

fn eval_ref() -> crate::run::folder::EvalRef {
    crate::run::folder::EvalRef {
        group: "g".into(),
        index: 1,
        of: 3,
        source: "r-1".into(),
        bases: Some(BTreeMap::from([(
            "/w".to_string(),
            crate::run::folder::EvalBase {
                base: "main".into(),
                base_commit: "abc".into(),
            },
        )])),
    }
}

fn tab_snap() -> crate::observe::attention::TabSnap {
    use crate::observe::attention::{PrFact, TabSnap};
    TabSnap {
        window_label: "main".into(),
        tab_id: "tab_1".into(),
        label: "Run".into(),
        activity: crate::observe::hooks::TabActivity::Working,
        run_id: Some("r-1".into()),
        run_dir: Some("/r".into()),
        stage: Some("building".into()),
        waiting_for: Some("plan-approval".into()),
        waiting_since: Some("2026-10-22T13:04:00Z".into()),
        run_mode: Some("triage".into()),
        eval: Some(eval_ref()),
        stalled: false,
        prs_done: 1,
        prs_total: 2,
        prs: vec![PrFact {
            id: "PR1".into(),
            state: Some("reviewed".into()),
            app_check: Some("pass".into()),
            ci: Some("pass".into()),
            pr_state: Some("OPEN".into()),
        }],
    }
}

fn run_view() -> crate::run::status::RunView {
    use crate::run::status::*;
    let pr = PrView {
        id: "PR1".into(),
        title: Some("Add multiply()".into()),
        repo: Some("mathlib".into()),
        repo_path: Some("/w/mathlib".into()),
        branch: Some("b".into()),
        worktree: Some("/w/wt".into()),
        state: Some("reviewed".into()),
        note: Some("n".into()),
        stories_done: Some(1),
        stories_total: Some(2),
        check: CheckView {
            command: Some("npm test".into()),
            result: Some("pass".into()),
            summary: Some("4 passed".into()),
            cwd: Some("/w".into()),
        },
        review: ReviewView {
            round: Some(1),
            verdict: Some("APPROVED".into()),
            open_findings: Some(0),
        },
        pr_url: Some("https://github.com/o/r/pull/1".into()),
        gate: Some(GateFile {
            gate: Some("PASS".into()),
            reason: Some("ok".into()),
            updated: Some("2026-10-22T13:39:00Z".into()),
            problem: Some("p".into()),
        }),
        app_check: Some(AppCheck {
            result: "pass".into(),
            command: Some("npm test".into()),
            skill_command: Some("npm test".into()),
            head: Some("abc".into()),
            clean: Some(true),
            cwd: Some("/w".into()),
            exit_code: Some(0),
            finished_at: Some("2026-10-22T13:39:00Z".into()),
            log: Some("logs/PR1-app-check.txt".into()),
            detail: Some("d".into()),
        }),
        gate_status: Some(GateStatus {
            kind: "proven".into(),
            text: "PASS".into(),
        }),
        app_command: Some("npm test".into()),
        check_known: true,
        current_head: Some("abc".into()),
        worktree_clean: Some(true),
        app_check_stale: false,
        draft: Some(false),
        comments: vec![PrComment {
            round: Some(1),
            url: "https://github.com/o/r/pull/1#c".into(),
        }],
        ci: Some("pass".into()),
        ci_at: Some("2026-10-22T13:40:00Z".into()),
        ci_error: Some("e".into()),
        pr_state: Some("OPEN".into()),
        receipt: ReceiptView {
            state: "rows".into(),
            rows: vec!["r".into()],
        },
        qa: Some(QaView {
            round: Some(1),
            result: "pass".into(),
            tried: 2,
            total: 2,
            failed: 0,
            path: Some("reviews/PR1-qa1.md".into()),
            criteria: vec![crate::run::qa_image::QaCriterion {
                id: "S1.1".into(),
                result: "PASS".into(),
                text: "t".into(),
                image: Some("S1.1".into()),
            }],
        }),
    };
    RunView {
        run_id: "r-1".into(),
        run_dir: "/r".into(),
        folder: Some("/w".into()),
        has_status: true,
        status_problem: Some("p".into()),
        stage: Some("ship-approval".into()),
        waiting_for: Some("ship-approval".into()),
        paused_reason: Some("p".into()),
        error: Some("e".into()),
        updated_at: Some("2026-10-22T13:40:00Z".into()),
        plan: Some(PlanView {
            approved: true,
            verdict: Some("APPROVED".into()),
            review_round: Some(1),
            open_findings: Some(0),
            risk_verdict: Some("APPROVED".into()),
            risk_open_findings: Some(0),
        }),
        prs: vec![pr],
        ship_note: Some("n".into()),
        timeline: vec![LogLine {
            time: Some("2026-10-22T13:04:05Z".into()),
            stage: Some("planning".into()),
            text: "t".into(),
        }],
        has_report: true,
        review_mode: "local".into(),
        run_mode: Some("triage".into()),
        orchestrator: Some(OrchestratorView {
            model: Some("opus".into()),
            permission_mode: Some("auto".into()),
        }),
        eval: Some(eval_ref()),
        ship_approved: false,
        bmad: Some(BmadView {
            mode: "auto".into(),
            version: Some("v".into()),
            size: Some("small".into()),
            reasons: vec!["r".into()],
            applied: true,
            check_result: Some("pass".into()),
            documents: vec!["bmad/spec.md".into()],
        }),
        cleanup: vec![crate::run::cleanup::CleanupRecord {
            pr_id: "PR1".into(),
            pr_url: "u".into(),
            outcome: "cleaned".into(),
            note: "n".into(),
            retry: false,
            worktree: "w".into(),
            branch: "b".into(),
            remote: "r".into(),
            at: "2026-10-22T13:40:00Z".into(),
        }],
    }
}

fn session_view() -> Sample {
    use crate::flight::schema::{AgentNode, Event, Failure, Phase};
    let derived = json!({"label": "exact:rule", "evidence": ["e1"]});
    let event: Event = serde_json::from_value(json!({
        "schema": 1, "id": "e1", "trace": "t", "session": "s",
        "agent": {"id": "main", "parent": "p", "kind": "main", "type": "t", "role": "orchestrator", "pr": "PR1"},
        "provider": {"name": "dcv2", "version": "1", "native": "n"},
        "kind": "tool.end", "at": "2026-10-22T13:04:00Z", "span": "sp", "status": "ok", "name": "Bash",
        "attrs": {"k": "v"},
        "usage": {"input": 1, "output": 1, "cacheWrite5m": 0, "cacheWrite1h": 0, "cacheRead": 0, "model": "m", "usd": 0.1},
        "content": {"ref": {"file": "f", "offset": 0, "len": 1, "sha256": "0"}, "policy": "metadata", "redactions": 0},
        "derived": "exact:rule"
    }))
    .expect("event");
    let node: AgentNode = serde_json::from_value(json!({
        "id": "main", "session": "s", "kind": "main", "type": "t", "role": "orchestrator", "pr": "PR1",
        "prDerived": derived, "description": "d", "spawnSpan": "sp", "files": ["f"], "children": [], "derived": derived
    }))
    .expect("agent node");
    let failure: Failure = serde_json::from_value(json!({
        "id": "e1", "agent": "main", "at": "2026-10-22T13:04:00Z", "name": "Bash", "kind": "tool.end",
        "derived": derived, "before": ["e0"], "repeatOf": derived, "repeats": [], "rule": "r"
    }))
    .expect("failure");
    let view = crate::flight::session::SessionView {
        status: "ok".into(),
        trace: "t".into(),
        policy: crate::flight::schema::CapturePolicy::Metadata,
        agents: vec![node],
        failures: vec![failure],
        page: crate::flight::session::Page {
            events: vec![event],
            next_cursor: Some("c".into()),
        },
        notices: vec!["n".into()],
        phases: BTreeMap::from([(
            "e1".to_string(),
            Phase {
                stage: Some("planning".into()),
                pr_state: Some("opened".into()),
                label: Some("exact:log-line".into()),
            },
        )]),
        snapshot: Some(crate::flight::session::SnapshotInfo {
            created_at: "2026-10-22T13:04:00Z".into(),
            policy: crate::flight::schema::CapturePolicy::Metadata,
            content_kept: false,
        }),
    };
    of(view).maps(&["phases"]).free(&["page.events[].attrs"])
}

fn pass_view() -> crate::run::webqa::PassView {
    crate::run::webqa::PassView {
        pass_id: "r-20261110-054207-f172".into(),
        run_dir: Some("/r".into()),
        release: false,
        created_at: Some("2026-11-10T05:42:07Z".into()),
        stage: Some("done".into()),
        target_id: Some("t-1".into()),
        target_name: Some("Staging".into()),
        running: false,
        result: Some("fail".into()),
        problem: Some("p".into()),
        rows: vec![
            json!({"charter": "c1", "title": "Sign in", "result": "FAIL", "reason": "r", "shot": "webqa/shots/c1.png", "comparison": "new"}),
        ],
        findings: vec![
            json!({"key": "login button hidden", "title": "Login button hidden", "severity": "high", "steps": "s", "expected": "e", "seen": "x", "shot": "webqa/shots/c1.png", "comparison": "still-failing"}),
        ],
        comparison: json!({"first": false, "fixedFindings": ["gone"]}),
        clickup_list_id: Some("901".into()),
        posted: std::collections::BTreeMap::from([(
            "create:0".to_string(),
            crate::run::triage::PostItem {
                result: "posted".into(),
                at: "2026-11-10T06:00:00Z".into(),
                message: Some("created".into()),
            },
        )]),
    }
}

fn web_qa_settings() -> crate::store::settings_store::WebQaSettings {
    use crate::store::settings_store::*;
    WebQaSettings {
        targets: vec![WebQaTarget {
            id: "t-1".into(),
            name: "Staging".into(),
            allowlist: vec!["https://app.example.com/".into()],
            username: "tester".into(),
            clickup_list_id: Some("901".into()),
            charters: vec![WebQaCharter {
                id: "c1".into(),
                title: "Sign in".into(),
                text: "Sign in and see the home page".into(),
                may_change_data: false,
            }],
        }],
    }
}

fn settings_view() -> Sample {
    use crate::store::settings_store::*;
    let terminal: TerminalSettings =
        serde_json::from_value(json!({"shell": "", "copyOnSelect": false})).expect("terminal");
    let ui: UiSettings = serde_json::from_value(json!({"editorCommand": ""})).expect("ui");
    let run: RunSettings = serde_json::from_value(json!({
        "usageHoldPercent": 90, "autoStartWaves": false,
        "triageSchedule": [{"accountId": "default", "listId": "1", "time": "08:00", "on": true, "folder": "/w"}]
    }))
    .expect("run");
    let observe: ObserveSettings =
        serde_json::from_value(json!({"evalEnabled": false})).expect("observe");
    let notify: NotifySettings =
        serde_json::from_value(json!({"events": {"done": true}})).expect("notify");
    let push: PushSettings =
        serde_json::from_value(json!({"events": {"done": true}})).expect("push");
    let claude: ClaudeSettings = serde_json::from_value(json!({
        "claudePath": "/bin/claude", "accounts": [{"id": "default", "name": "Personal", "configDir": "/h/.claude"}]
    }))
    .expect("claude");
    of(SettingsView {
        terminal,
        ui,
        claude,
        run,
        observe,
        notify,
        push,
        clickup: ClickUpSettings {
            auto_post: BTreeMap::from([("default".to_string(), true)]),
            done_status: BTreeMap::from([("default".to_string(), "done".to_string())]),
        },
        web_qa: web_qa_settings(),
        accounts: vec![ResolvedClaudeAccount {
            id: "default".into(),
            name: "Personal".into(),
            config: cfg(),
            env_override: false,
        }],
        config_dir_env: Some("/h/.claude".into()),
    })
    .nullable(&["configDirEnv"])
    .optional(&["claude.claudePath", "claude.accounts[].configDir"])
    .maps(&[
        "notify.events",
        "push.events",
        "clickup.autoPost",
        "clickup.doneStatus",
    ])
}

/// One sample per faked command: every key of `fixtures/fake/*.json` (but `not-faked.json` and `shapes.json`).
fn table() -> BTreeMap<&'static str, Sample> {
    let mut t: BTreeMap<&'static str, Sample> = BTreeMap::new();
    let string = || of("text");
    let nothing = || of(());

    // ---- app.json ----
    t.insert(
        "window_context",
        of(crate::windows::WindowContext {
            id: "main".into(),
            account_id: "default".into(),
            account_name: "Personal".into(),
            title: "t".into(),
            config: cfg(),
            account_missing: false,
        }),
    );
    t.insert(
        "get_app_state",
        of(AppStateSnapshot {
            window_id: "main".into(),
            active_tab_id: Some("tab_1".into()),
            tabs: vec![tab_summary()],
            closed_tabs: vec![ClosedTabSummary {
                id: "tab_3".into(),
                label: "l".into(),
                cwd: "/w".into(),
                kind: crate::store::TabKind::Shell,
            }],
        })
        .nullable(&[
            "activeTabId",
            "tabs[].runDir",
            "tabs[].role",
            "tabs[].split",
        ]),
    );
    t.insert("recent_folders", of(vec!["/w"]));
    t.insert(
        "attention_summary",
        of(crate::observe::attention::Summary {
            needs_you: 1,
            active_runs: 1,
        }),
    );
    t.insert(
        "active_runs",
        of(vec![tab_snap()])
            .optional(&["[].eval.bases"])
            .maps(&["[].eval.bases"]),
    );
    t.insert(
        "run_tabs",
        of(vec![tab_snap()])
            .optional(&["[].eval.bases"])
            .maps(&["[].eval.bases"]),
    );
    t.insert("terminal_resume_id", of(Some("session-id")).nullable(&[""]));
    t.insert("pty_set_scheme", nothing());
    t.insert(
        "get_claude_usage",
        of(crate::usage::UsageSnapshot {
            config_dir: "/h/.claude".into(),
            windows: vec![crate::usage::RateWindow {
                rate_limit_type: "five_hour".into(),
                label: "5h".into(),
                utilization: Some(0.5),
                resets_at: Some(1),
                status: "allowed".into(),
                seen_at_ms: 1,
            }],
            history: vec![crate::usage::WindowHistory {
                rate_limit_type: "five_hour".into(),
                readings: vec![crate::usage::Reading {
                    at_ms: 1,
                    utilization: Some(0.5),
                    resets_at: Some(1),
                }],
            }],
        }),
    );
    t.insert(
        "claude_status",
        of(crate::provider::claude::ClaudeStatus {
            found: true,
            path: Some("/bin/claude".into()),
            version: Some("2.1.0".into()),
            error: Some("e".into()),
        }),
    );
    t.insert(
        "claude_version_status",
        of(crate::observe::ClaudeVersionStatus {
            version: Some("2.1.0".into()),
            range: "ok".into(),
            min: "2.0.0".into(),
            max_tested: "2.2.0".into(),
            selftest: Some(crate::observe::version::SelfTest::default()),
            selftest_wanted: false,
        }),
    );
    t.insert(
        "claude_account_logins",
        of(vec![ClaudeAccountLogin {
            id: "default".into(),
            name: "Personal".into(),
            config: cfg(),
            login: crate::cli_detect::LoginStatus {
                state: "loggedIn".into(),
                account: Some("a@b.c".into()),
                detail: Some("d".into()),
                api_key_env: false,
                method: Some("claude.ai".into()),
                organization: Some("o".into()),
            },
        }])
        .optional(&["[].login.method", "[].login.organization"]),
    );
    t.insert(
        "memory_status",
        // Untyped in Rust (`memory.sh status` JSON plus `available`): the keys the UI reads.
        of(json!({"available": true, "problem": "p", "workspace": {"present": true, "path": "/m"}, "repos": []}))
            .optional(&["problem", "workspace", "repos"]),
    );
    t.insert(
        "paths_exist",
        of(vec![crate::run::preview::PathHit {
            candidate: "a".into(),
            path: Some("/a".into()),
            previewable: true,
        }]),
    );
    t.insert(
        "take_run_link",
        of(Some(crate::run::link::RunLinkTarget {
            run_dir: Some("/r".into()),
            error: Some("e".into()),
        }))
        .nullable(&[""])
        .optional(&["runDir", "error"]),
    );
    t.insert(
        "dev_import_status",
        of(crate::import_dev::ImportStatus {
            available: true,
            offered: false,
            imported_at: Some("2026-10-22T13:04:00Z".into()),
            dev_dir: "/d".into(),
        }),
    );
    t.insert("notification_permission", string());
    t.insert(
        "push_status",
        of(crate::observe::push::PushStatus {
            saved: vec!["ntfy-topic".into()],
        }),
    );
    t.insert(
        "detect_folder_repos",
        of(crate::run::repos::FolderRepos {
            kind: "repo".into(),
            repos: vec![crate::run::repos::Repo {
                name: "app".into(),
                path: "/w/app".into(),
                check: Some("npm test".into()),
                setup: Some("npm install".into()),
                copy_files: vec![".env".into()],
                live_tests: true,
                receipt: true,
                qa: Some(crate::store::QaSetup {
                    command: "npm start".into(),
                    url: "http://127.0.0.1:%PORT%".into(),
                }),
            }],
        })
        .optional(&["repos[].liveTests", "repos[].receipt", "repos[].qa"]),
    );
    t.insert(
        "dispatch_overview",
        of(crate::run::dispatch::Overview {
            cap: 3,
            usage_hold_percent: Some(90),
            queue: vec![serde_json::from_value(json!({
                "id": "q-1",
                "params": {"windowId": "main", "folder": "/w", "request": "r", "role": "eagle-eye", "clickup": {"refs": [], "ticketIds": ["1"]},
                           "reviewMode": "github", "bmadMode": "auto", "force": false, "repos": ["a"], "maxImplementers": 3,
                           "runMode": "triage", "dispatchEntryId": "q-1", "triageListId": "1",
                           "eval": {"group": "g", "index": 1, "of": 2, "source": "r-1"}},
                "accountId": "default", "label": "l", "state": "queued", "reason": "r",
                "queuedAt": "2026-10-22T14:00:00Z", "startedAt": "2026-10-22T14:01:00Z", "batchId": "b",
                "bypassCap": false, "wave": {"epicRun": "r-1", "n": 1}
            }))
            .expect("queue entry")],
            holders: vec![crate::run::dispatch::Holder {
                run_dir: "/r".into(),
                run_id: "r-1".into(),
                stage: Some("building".into()),
                why: "building".into(),
            }],
            at_cap: false,
        })
        .nullable(&["usageHoldPercent"])
        .optional(&["queue[].params.eval", "queue[].params.triageListId", "queue[].params.eval.bases"]),
    );
    t.insert("dispatch_runs", string());
    t.insert(
        "epics_overview",
        of(vec![crate::run::epics::EpicView {
            run_dir: "/r".into(),
            run_id: "r-1".into(),
            problem: Some("p".into()),
            cards: vec![crate::run::epics::WaveCard {
                n: 1,
                title: "t".into(),
                ticket_ids: vec!["1".into()],
                state: "ready".into(),
                reason: Some("r".into()),
            }],
        }]),
    );
    t.insert(
        "batch_overview",
        of(vec![crate::run::dispatch::BatchView {
            id: "b".into(),
            created_at: "2026-10-22T14:00:00Z".into(),
            rows: vec![crate::run::dispatch::BatchRowView {
                entry_id: "e".into(),
                label: "l".into(),
                ticket_ids: vec!["1".into()],
                removed: false,
                run_dir: Some("/r".into()),
                stage: "queued".into(),
                waiting_for: Some("w".into()),
                pr_url: Some("u".into()),
                ci: Some("pass".into()),
            }],
            waiting_for_you: 0,
            eval: Some(crate::run::dispatch::EvalBatchView {
                n: 3,
                source: "r-1".into(),
                finished: 1,
            }),
        }]),
    );
    t.insert("pty_open", of(1u32));
    t.insert("pty_write", nothing());
    t.insert("pty_resize", nothing());
    t.insert("pty_kill", nothing());

    // ---- clickup.json ----
    t.insert(
        "clickup_token_status",
        of(crate::clickup::commands::TokenStatus {
            account_id: "default".into(),
            saved: true,
            problem: Some("p".into()),
        }),
    );
    t.insert(
        "clickup_run_state",
        of(crate::clickup::commands::RunClickUp {
            has_tickets: true,
            ticket_count: 1,
            fetched_at: Some("2026-10-22T14:00:00Z".into()),
            tickets_problem: Some("p".into()),
            pulled: vec!["1".into()],
            update_file: Some("f".into()),
            drafts_version: Some("v".into()),
            writeback_problem: Some("p".into()),
            auto_post: Some(Default::default()),
            ..Default::default()
        })
        // The draft and merged rows are filled by the writeback flow; their inner shape is not held here.
        .nullable(&["drafts"]),
    );

    // ---- run.json ----
    t.insert(
        "read_run",
        of(run_view())
            .nullable(&["plan", "orchestrator", "eval"])
            .optional(&["orchestrator", "prs[].qa", "eval.bases"])
            .maps(&["eval.bases"]),
    );
    t.insert(
        "resume_run",
        of(crate::run::StartedRun {
            tab: tab_summary(),
            run_dir: Some("/r".into()),
            focused: false,
        })
        .nullable(&["runDir", "tab.runDir", "tab.role", "tab.split"]),
    );
    t.insert(
        "set_repo_setup",
        of(crate::store::RepoSetup {
            setup: Some("npm install".into()),
            copy_files: vec![".env".into()],
            live_tests: false,
            receipt: true,
            qa: Some(crate::store::QaSetup {
                command: "npm start".into(),
                url: "http://127.0.0.1:%PORT%".into(),
            }),
        })
        .optional(&["qa"]),
    );
    t.insert(
        "run_timeline",
        of(crate::run::timeline::Timeline {
            lanes: vec![crate::run::timeline::Lane {
                id: "orch".into(),
                session: 0,
                agent_id: Some("a1".into()),
                label: "Orchestrator".into(),
                description: Some("d".into()),
                state: "finished".into(),
                start_ms: Some(1),
                finish_ms: Some(2),
                cost_usd: Some(0.5),
            }],
            bands: vec![crate::run::timeline::Band {
                kind: "stage".into(),
                name: "planning".into(),
                start_ms: 1,
                end_ms: 2,
            }],
            work_minutes: 1.0,
            wait_minutes: 1.0,
            cost_usd: Some(1.0),
            started_ms: Some(1),
            ended_ms: Some(2),
        }),
    );
    t.insert("flight_session", session_view());
    t.insert(
        "run_worktrees",
        of(vec![crate::run::janitor::WorktreeItem {
            pr_id: "PR1".into(),
            worktree: "/wt".into(),
            repo_path: Some("/w".into()),
            branch: Some("b".into()),
            exists: true,
            clean: true,
            changed_files: 0,
            removable: true,
            disposable_only: false,
            problem: Some("p".into()),
        }]),
    );
    t.insert(
        "pr_worktree_checks",
        of(vec![crate::run::janitor::WorktreeCheck {
            pr_id: "PR1".into(),
            verified: true,
            reason: Some("r".into()),
        }]),
    );
    t.insert(
        "flight_compare",
        of(crate::flight::host::CompareOut {
            a: compare_side("r-1"),
            b: compare_side("r-2"),
            rows: vec![],
            divergence: Some(0),
            progress: crate::flight::index::Counts {
                current: 1,
                total: 2,
            },
        })
        .nullable(&["divergence"]),
    );
    t.insert(
        "flight_eval_results",
        of(crate::flight::host::EvalResults {
            group: "g".into(),
            source: "/r".into(),
            n: 2,
            runs: vec![crate::flight::host::EvalRunResult {
                index: 1,
                label: "Run 1".into(),
                run_id: Some("r-1".into()),
                run_dir: Some("/r".into()),
                status: "finished".into(),
                completed: Some(true),
                check_pass_rate: Some(1.0),
                check_state: "checked".into(),
                retries: Some(0),
                tool_failures: Some(0),
                tokens: Some(1),
                active_minutes: Some(1.0),
                kept_branches: vec![crate::run::janitor::KeptBranch {
                    pr_id: "PR1".into(),
                    branch: "b".into(),
                    repo_path: "/w".into(),
                    unmerged_commits: Some(1),
                }],
            }],
            distributions: crate::flight::host::EvalDistributions::default(),
            finished: 1,
            running: 0,
            failed: 0,
        })
        .nullable(&[
            "runs[].runId",
            "runs[].runDir",
            "runs[].completed",
            "runs[].checkPassRate",
            "runs[].retries",
            "runs[].toolFailures",
            "runs[].tokens",
            "runs[].activeMinutes",
            "runs[].keptBranches[].unmergedCommits",
            "distributions.completed.min",
            "distributions.completed.median",
            "distributions.completed.max",
            "distributions.checkPassRate.min",
            "distributions.checkPassRate.median",
            "distributions.checkPassRate.max",
            "distributions.retries.min",
            "distributions.retries.median",
            "distributions.retries.max",
            "distributions.toolFailures.min",
            "distributions.toolFailures.median",
            "distributions.toolFailures.max",
            "distributions.tokens.min",
            "distributions.tokens.median",
            "distributions.tokens.max",
            "distributions.activeMinutes.min",
            "distributions.activeMinutes.median",
            "distributions.activeMinutes.max",
        ]),
    );
    t.insert("flight_run_share", of(Some(1.0_f64)).nullable(&[""]));
    t.insert(
        "flight_export_preview",
        of(crate::flight::host::ExportPreview {
            policy: crate::flight::export::ExportPolicy::Metadata,
            encoding: crate::flight::export::Encoding::Json,
            endpoint: "http://127.0.0.1:4318/v1/traces".into(),
            spans: 1,
            events: 1,
            bytes: 1,
            digest: "d".into(),
            fields: vec![crate::flight::export::FieldCount {
                field: "f".into(),
                count: 1,
                leaves: 1,
            }],
            warnings: vec!["w".into()],
            loopback: true,
            capture_policy: crate::flight::schema::CapturePolicy::Metadata,
        }),
    );
    t.insert(
        "dispatch_eval_preview",
        of(crate::run::dispatch::EvalPreview {
            title: "t".into(),
            hold_percent: Some(80),
            five_hour: Some(crate::run::dispatch::FiveHourReading {
                percent: 1.0,
                resets_at: Some(1),
            }),
            based_repos: vec!["/w".into()],
            default_branch_repos: vec!["/x".into()],
        })
        .nullable(&["holdPercent", "fiveHour", "fiveHour.resetsAt"]),
    );
    t.insert(
        "observe_tab",
        of(crate::observe::TabObservation {
            hook: Some(crate::observe::hooks::TabHookState {
                activity: crate::observe::hooks::TabActivity::Working,
                last_event: Some("PreToolUse".into()),
                updated_ms: Some(1),
                subagents_running: 1,
                tool_running: true,
                session_id: Some("s".into()),
                activity_since_ms: Some(1),
                compacting: false,
                model: Some("opus".into()),
                ..Default::default()
            }),
            hooks_active: true,
            hooks_installed: true,
            transcript_found: true,
            context: Some(crate::observe::transcript::ContextMeter {
                model: "opus".into(),
                used: 1,
                window: 2,
                percent: 50.0,
                warn: false,
                warn_percent: 80.0,
            }),
            subagents: vec![crate::observe::transcript::SubagentInfo {
                tool_use_id: "t".into(),
                agent_id: Some("a".into()),
                agent_type: "dcv2:implementer".into(),
                description: Some("d".into()),
                status: "running".into(),
                background: false,
                started_ms: Some(1),
                finished_ms: Some(2),
            }],
            usage: Some(crate::observe::transcript::UsageSummary {
                stages: vec![cost_line()],
                agents: vec![cost_line()],
                total_tokens: 1,
                total_cost_usd: Some(1.0),
                minutes: 1.0,
                messages: 1,
            }),
            stall: Some(crate::observe::watchdog::Stall {
                minutes: 1,
                threshold: 10,
            }),
            waiting: Some(crate::observe::watchdog::Waiting {
                minutes: 1,
                threshold: 10,
                since_ms: 1,
            }),
        })
        .nullable(&[
            "hook",
            "hook.lastEvent",
            "hook.updatedMs",
            "hook.sessionId",
            "hook.activitySinceMs",
            "hook.model",
            "context",
            "subagents[].agentId",
            "subagents[].description",
            "subagents[].startedMs",
            "subagents[].finishedMs",
            "usage",
            "usage.totalCostUsd",
            "usage.stages[].costUsd",
            "usage.agents[].costUsd",
            "stall",
            "waiting",
        ]),
    );
    t.insert(
        "observe_tabs",
        of(BTreeMap::from([(
            "tab_1".to_string(),
            crate::observe::hooks::TabHookState {
                activity: crate::observe::hooks::TabActivity::Working,
                last_event: Some("PreToolUse".into()),
                updated_ms: Some(1),
                subagents_running: 0,
                tool_running: false,
                session_id: Some("s".into()),
                activity_since_ms: Some(1),
                compacting: false,
                model: Some("opus".into()),
                ..Default::default()
            },
        )]))
        .maps(&[""]),
    );

    // ---- runs.json ----
    t.insert(
        "run_overview",
        of(crate::run::RunOverview {
            plugin_dir: Some("/p".into()),
            plugin_version: Some("0.10.0".into()),
            roles: vec![crate::run::RoleTile {
                id: "planner".into(),
                label: "Planner".into(),
            }],
            runs: vec![crate::run::folder::RunListItem {
                run_id: "r-1".into(),
                run_dir: "/r".into(),
                folder: Some("/w".into()),
                created_at: Some("2026-10-22T13:04:00Z".into()),
                stage: Some("done".into()),
                updated_at: Some("2026-10-22T13:40:00Z".into()),
                title: Some("t".into()),
                adopted: false,
                record: Some(crate::run::folder::RecordTotals {
                    outcome: Some("done".into()),
                    total_tokens: 1,
                    estimate_usd: Some(1.0),
                    minutes: 1.0,
                    wait_minutes: Some(1.0),
                    span_minutes: Some(1.0),
                    pr_urls: vec!["u".into()],
                    first_activity: Some("2026-10-22T13:04:00Z".into()),
                    last_activity: Some("2026-10-22T13:40:00Z".into()),
                }),
                pr_count: Some(1),
                ticket_ids: vec!["1".into()],
                has_report: true,
                repo_count: Some(1),
                repos: vec!["mathlib".into()],
                bmad_applied: Some(true),
                eval: Some(eval_ref()),
            }],
        })
        .nullable(&["runs[].record", "runs[].eval"])
        .optional(&["runs[].eval.bases"])
        .maps(&["runs[].eval.bases"]),
    );
    t.insert("run_qa_image", string());
    t.insert(
        "triage_overview",
        of(vec![crate::run::triage::TriageView {
            run_dir: "/r".into(),
            run_id: "r-1".into(),
            list_id: Some("1".into()),
            proposal: json!({"groups": []}),
            digest: "d".into(),
            approval: Some(
                serde_json::from_value(json!({
                    "schema": 1, "triageDigest": "d", "decision": "approved",
                    "groups": [{"n": 1, "title": "t", "tickets": ["1"], "dependsOn": [], "why": "w"}],
                    "creates": [{"index": 0, "name": "n", "description": "d"}], "closes": ["2"], "at": "2026-10-22T13:04:00Z"
                }))
                .expect("approval"),
            ),
            posted: Some(
                serde_json::from_value(json!({
                    "schema": 1, "items": {"1": {"result": "created", "at": "2026-10-22T13:04:00Z", "message": "m"}}
                }))
                .expect("posted"),
            ),
            problem: Some("p".into()),
        }])
        .nullable(&["[].listId", "[].approval", "[].posted", "[].problem"]),
    );
    t.insert("triage_blocking", of(Some("blocked")).nullable(&[""]));

    // ---- settings.json ----
    t.insert("get_settings", settings_view());
    t.insert("webqa_targets_save", of(web_qa_settings()));
    t.insert(
        "webqa_start",
        of(crate::run::StartedRun {
            tab: tab_summary(),
            run_dir: Some("/r".into()),
            focused: false,
        })
        .nullable(&["runDir", "tab.runDir", "tab.role", "tab.split"]),
    );
    t.insert(
        "webqa_overview",
        of(vec![pass_view()])
            .maps(&["[].posted"])
            .free(&["[].comparison"]),
    );
    t.insert(
        "webqa_release_open",
        of(pass_view()).maps(&["posted"]).free(&["comparison"]),
    );
    t.insert(
        "webqa_approve",
        of(crate::run::webqa::WebQaApproval {
            schema: 1,
            digest: "d".into(),
            creates: vec![crate::run::triage::CreateRec {
                index: 0,
                name: "n".into(),
                description: "d".into(),
            }],
            at: "2026-11-10T06:00:00Z".into(),
        }),
    );
    t.insert(
        "webqa_apply",
        of(vec![crate::run::triage::ApplyOutcome {
            item: "create:0".into(),
            result: "posted".into(),
            message: "created".into(),
        }]),
    );
    for command in [
        "webqa_password_status",
        "webqa_password_set",
        "webqa_password_clear",
    ] {
        t.insert(
            command,
            of(crate::run::webqa::PasswordStatus {
                target_id: "t-1".into(),
                saved: true,
                problem: Some("p".into()),
            }),
        );
    }
    t
}

// ---- reduction to a shape ----------------------------------------------------------------------------------

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The key paths a sample marks (see [`Sample`]).
#[derive(Clone, Copy, Default)]
struct Marks<'a> {
    nullable: &'a [&'a str],
    optional: &'a [&'a str],
    maps: &'a [&'a str],
    free: &'a [&'a str],
}

/// The shape of `value` at `path`: its JSON type; for an object the shape of each key (with `nullable` and
/// `optional` where the table says so, or where the sample itself is null); for an array the shape of its first
/// element (`null` when the sample has none). A `maps` path has the shape of one value under `map`; a `free`
/// path accepts any keys.
fn shape(value: &Value, path: &str, marks: Marks) -> Value {
    let mut out = Map::new();
    out.insert("type".into(), type_name(value).into());
    match value {
        Value::Object(_) if marks.free.contains(&path) => {
            out.insert("free".into(), true.into());
        }
        Value::Object(map) if marks.maps.contains(&path) => {
            let element = map
                .values()
                .next()
                .map(|first| shape(first, &format!("{path}{{}}"), marks));
            out.insert("map".into(), element.unwrap_or(Value::Null));
        }
        Value::Object(map) => {
            let mut fields = Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let mut field = shape(&map[key], &child_path, marks);
                let object = field.as_object_mut().expect("shape is an object");
                if marks.nullable.contains(&child_path.as_str()) || map[key].is_null() {
                    object.insert("nullable".into(), true.into());
                }
                if marks.optional.contains(&child_path.as_str()) {
                    object.insert("optional".into(), true.into());
                }
                fields.insert(key.clone(), field);
            }
            out.insert("fields".into(), fields.into());
        }
        Value::Array(items) => {
            let element = items
                .first()
                .map(|first| shape(first, &format!("{path}[]"), marks));
            out.insert("items".into(), element.unwrap_or(Value::Null));
        }
        _ => {}
    }
    Value::Object(out)
}

/// The whole file: one shape per faked command; a top-level `nullable` marks an `Option` answer.
fn shapes_of(samples: &BTreeMap<&'static str, Sample>) -> Value {
    let mut all = Map::new();
    for (command, sample) in samples {
        let marks = Marks {
            nullable: sample.nullable,
            optional: sample.optional,
            maps: sample.maps,
            free: sample.free,
        };
        let mut one = shape(&sample.value, "", marks);
        if sample.nullable.contains(&"") {
            one.as_object_mut()
                .expect("object")
                .insert("nullable".into(), true.into());
        }
        all.insert((*command).into(), one);
    }
    Value::Object(all)
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake")
}

fn rendered(samples: &BTreeMap<&'static str, Sample>) -> String {
    let mut text = serde_json::to_string_pretty(&shapes_of(samples)).expect("serializes");
    text.push('\n');
    text
}

/// The commands the fixtures fake: every key of `fixtures/fake/*.json` but the two bookkeeping files.
fn faked_commands() -> Vec<String> {
    let mut commands = Vec::new();
    for entry in std::fs::read_dir(fixtures_dir()).expect("fixtures/fake exists") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.ends_with(".json") || name == "not-faked.json" || name == "shapes.json" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("reads");
        let value: Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} is JSON: {e}"));
        commands.extend(
            value
                .as_object()
                .unwrap_or_else(|| panic!("{name} is an object"))
                .keys()
                .cloned(),
        );
    }
    commands.sort();
    commands
}

#[test]
fn every_faked_command_has_a_sample_and_no_sample_is_orphaned() {
    let table = table();
    let faked = faked_commands();
    let missing: Vec<&String> = faked
        .iter()
        .filter(|c| !table.contains_key(c.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "faked commands with no sample in src/fake_shapes.rs: {missing:?}"
    );
    let orphan: Vec<&&str> = table
        .keys()
        .filter(|c| !faked.iter().any(|f| f == *c))
        .collect();
    assert!(
        orphan.is_empty(),
        "samples for commands that no fixture fakes: {orphan:?}"
    );
}

#[test]
fn shapes_json_matches_the_rust_types() {
    let want = rendered(&table());
    let path = fixtures_dir().join("shapes.json");
    // `UPDATE_FAKE_SHAPES` is the same switch for a dcv2 session, whose guard refuses setting any DCV2_* variable.
    let update = |name: &str| std::env::var(name).as_deref() == Ok("1");
    if update("DCV2_UPDATE_SHAPES") || update("UPDATE_FAKE_SHAPES") {
        std::fs::write(&path, &want).expect("writes shapes.json");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "fixtures/fake/shapes.json is stale: a faked Rust type changed. Regenerate it with \
         `DCV2_UPDATE_SHAPES=1 cargo test --manifest-path src-tauri/Cargo.toml fake_shapes`, then make `npm test` pass \
         by updating the fixtures."
    );
}

#[test]
fn a_renamed_field_changes_the_shape() {
    let none = Marks::default();
    let before = shape(&json!({"cap": 3, "holders": [{"runId": "r"}]}), "", none);
    let renamed = shape(
        &json!({"cap": 3, "holders": [{"runIdentifier": "r"}]}),
        "",
        none,
    );
    assert_ne!(before, renamed);
    let nullable = shape(&json!({"a": null}), "", none);
    assert_eq!(nullable["fields"]["a"]["nullable"], json!(true));
    let optional = shape(
        &json!({"a": 1}),
        "",
        Marks {
            optional: &["a"],
            ..none
        },
    );
    assert_eq!(optional["fields"]["a"]["optional"], json!(true));
    let map = shape(
        &json!({"events": {"x": true}}),
        "",
        Marks {
            maps: &["events"],
            ..none
        },
    );
    assert_eq!(map["fields"]["events"]["map"]["type"], json!("boolean"));
    assert!(map["fields"]["events"].get("fields").is_none());
}
