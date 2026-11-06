use super::*;
use crate::clickup::client::tests::{ok, status, FakeClock, Recorded};
use crate::clickup::client::Gate;
use crate::clickup::intake::{tests::sample, SnapshotSource, TicketsSnapshot, LIMITS};
use crate::clickup::transport::NoReply;
use crate::clickup::ClickUpToken;
use crate::run::epics;
use crate::test_support::TempDir;
use chrono::NaiveDate;

const LIST: &str = "901800112233";
const NOW: i64 = 1_790_000_000;

fn key() -> Key {
    trust::tests::key()
}

fn task(id: &str, kind: &str, updated: Option<&str>) -> Task {
    serde_json::from_value(json!({
        "id": id, "name": format!("Ticket {id}"),
        "status": {"status": kind, "type": kind},
        "date_updated": updated,
    }))
    .unwrap()
}

fn backdate(path: &Path, age: Duration) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - age).unwrap();
}

const MD: &str = "## Group 1: Foundations\n- Tickets: t1, t2\n- Depends on: none\n- Why: first\n\n## Group 2: Features\n- Tickets: t3\n- Depends on: Group 1\n- Why: second\n";

/// A finished triage run of list `list` under `data/runs/<id>`: meta, status, snapshot, triage.md and triage.json, and
/// the app's signed snapshot record.
fn triage_run(data: &Path, id: &str, list: &str, stage: &str, created: &str) -> PathBuf {
    let dir = data.join("runs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("request.md"), "folder: /w/app\n").unwrap();
    std::fs::write(
        dir.join("meta.json"),
        json!({"schema": 1, "createdAt": created, "mode": "triage", "folder": "/w/app", "account": {"id": "acct-1"},
               "triage": {"listId": list}, "bmad": {"mode": "off"}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("status.json"),
        json!({"schema": 1, "stage": stage, "prs": []}).to_string(),
    )
    .unwrap();
    let names = ["t1", "t2", "t3", "t4", "t5", "t6"];
    std::fs::write(
        dir.join("tickets.json"),
        json!({"tasks": names.iter().map(|i| json!({"id": i, "name": format!("Ticket {i}")})).collect::<Vec<_>>()})
            .to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("triage.md"), MD).unwrap();
    let digest = digest_of_markdown(&dir).unwrap();
    std::fs::write(
        dir.join("triage.json"),
        json!({"schema": 1, "digest": digest,
               "groups": [
                   {"n": 1, "title": "Foundations", "tickets": ["t1", "t2"], "dependsOn": [], "why": "first"},
                   {"n": 2, "title": "Features", "tickets": ["t3"], "dependsOn": [1], "why": "second"}],
               "needsYou": [{"ticket": "t4", "reason": "which provider?"}],
               "suggestions": [{"index": 0, "title": "Add a retry", "why": "sync fails once", "description": "Retry the call twice."},
                               {"index": 1, "title": "Add a log line", "why": "debugging", "description": "Log the failure."}],
               "closes": [{"ticket": "t5", "why": "done in the code"}],
               "outOfScope": [{"ticket": "t6", "reason": "later"}]})
        .to_string(),
    )
    .unwrap();
    let snapshot = TicketsSnapshot {
        schema: 1,
        fetched_at: "t".into(),
        source: SnapshotSource {
            kind: "clickup".into(),
            account_id: "acct-1".into(),
            workspace_id: None,
            refs: vec![],
        },
        limits: LIMITS,
        note: String::new(),
        tasks: names
            .iter()
            .map(|i| sample(i, &format!("Ticket {i}")))
            .collect(),
    };
    trust::write_start(&key(), data, &dir, "acct-1", &snapshot).unwrap();
    dir
}

fn digest(dir: &Path) -> String {
    digest_of_markdown(dir).unwrap()
}

fn all_edits() -> Edits {
    Edits {
        groups: vec![
            EditGroup {
                title: "Foundations".into(),
                tickets: vec!["t1".into(), "t2".into()],
                depends_on: vec![],
                why: "first".into(),
            },
            EditGroup {
                title: "Features".into(),
                tickets: vec!["t3".into()],
                depends_on: vec![1],
                why: "second".into(),
            },
        ],
        creates: vec![0],
        closes: vec!["t5".into()],
    }
}

// ---- S11: start and schedule -----------------------------------------------------------------------------------

#[test]
fn closed_and_done_tickets_are_left_out_the_rest_come_newest_first_at_most_a_hundred() {
    let mut tasks = vec![
        task("old", "open", Some("1000")),
        task("closed", "closed", Some("9999")),
        task("done", "done", Some("9998")),
        task("new", "in progress", Some("5000")),
        task("undated", "open", None),
    ];
    let open = open_tasks(tasks.clone());
    assert_eq!(
        open.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["new", "old", "undated"]
    );
    tasks
        .extend((0..150).map(|i| task(&format!("n{i}"), "open", Some(&format!("{}", 10_000 + i)))));
    let capped = open_tasks(tasks);
    assert_eq!(capped.len(), MAX_TICKETS);
    assert_eq!(capped[0].id, "n149", "newest first");
}

#[test]
fn a_triage_start_queues_only_the_open_tickets_with_triage_mode_and_bmad_off() {
    let data = TempDir::new("triage_enqueue");
    let tasks = vec![
        task("a", "open", Some("10")),
        task("b", "closed", Some("30")),
        task("c", "open", Some("20")),
    ];
    let id = enqueue_triage(
        data.path(),
        "acct-1",
        "/w/app",
        LIST,
        "Tasks",
        tasks,
        SystemTime::now(),
    )
    .unwrap();
    let state = dispatch::load(data.path());
    let entry = &state.queue[0];
    assert_eq!(entry.id, id);
    assert_eq!(entry.params.run_mode.as_deref(), Some("triage"));
    assert_eq!(entry.params.bmad_mode.as_deref(), Some("off"));
    assert_eq!(entry.params.triage_list_id.as_deref(), Some(LIST));
    assert_eq!(entry.account_id, "acct-1");
    assert_eq!(
        entry.params.clickup.as_ref().unwrap().ticket_ids,
        ["c", "a"]
    );
    let again = enqueue_triage(
        data.path(),
        "acct-1",
        "/w/app",
        LIST,
        "Tasks",
        vec![task("x", "open", None)],
        SystemTime::now(),
    );
    assert!(again.unwrap_err().contains("already queued"));
    let other = enqueue_triage(
        data.path(),
        "acct-1",
        "/w/app",
        "777",
        "Other",
        vec![task("x", "open", None)],
        SystemTime::now(),
    );
    assert!(other.is_ok(), "another list is not blocked");
    let empty = enqueue_triage(
        data.path(),
        "acct-1",
        "/w/app",
        "888",
        "Closed",
        vec![task("z", "closed", None)],
        SystemTime::now(),
    );
    assert!(empty.unwrap_err().contains("no open tickets"));
}

#[test]
fn a_triage_run_records_its_mode_and_list_in_meta_json() {
    use crate::run::folder::{create_run, RunMeta};
    let data = TempDir::new("triage_meta");
    let meta = RunMeta {
        account_id: "a".into(),
        mode: Some("triage".into()),
        bmad_mode: Some("off".into()),
        triage_list_id: Some(LIST.into()),
        ..Default::default()
    };
    let created = create_run(data.path(), Path::new("/w"), &[], &[], "Triage", &meta).unwrap();
    let dir = Path::new(&created.run_dir);
    assert_eq!(folder::run_mode_of(dir).as_deref(), Some("triage"));
    assert_eq!(folder::triage_list_of(dir).as_deref(), Some(LIST));
    assert!(folder::RUN_MODES.contains(&"triage"));
}

#[test]
fn an_unfinished_run_blocks_its_list_and_a_stale_one_older_than_six_hours_does_not() {
    let data = TempDir::new("triage_block");
    let run = triage_run(
        data.path(),
        "r-20261106-080000-aaaa",
        LIST,
        "planning",
        "2026-11-06T08:00:00Z",
    );
    let why = blocking_reason(data.path(), LIST, SystemTime::now()).unwrap();
    assert!(
        why.contains("r-20261106-080000-aaaa"),
        "names the blocking run: {why}"
    );
    assert!(
        blocking_reason(data.path(), "777", SystemTime::now()).is_none(),
        "another list is free"
    );
    backdate(&run.join("status.json"), Duration::from_secs(7 * 60 * 60));
    assert!(
        blocking_reason(data.path(), LIST, SystemTime::now()).is_none(),
        "a dead run stops blocking"
    );
    backdate(&run.join("status.json"), Duration::from_secs(5 * 60 * 60));
    assert!(blocking_reason(data.path(), LIST, SystemTime::now()).is_some());
    std::fs::write(
        run.join("status.json"),
        json!({"schema": 1, "stage": "done", "prs": []}).to_string(),
    )
    .unwrap();
    assert!(
        blocking_reason(data.path(), LIST, SystemTime::now()).is_none(),
        "a finished run does not block"
    );
}

fn day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
}

fn schedule() -> crate::store::settings_store::TriageSchedule {
    crate::store::settings_store::TriageSchedule {
        account_id: "acct-1".into(),
        list_id: LIST.into(),
        time: "08:00".into(),
        on: true,
        folder: "/w/app".into(),
    }
}

fn local(date: &str, time: &str) -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M").unwrap()
}

/// One dispatcher tick of the schedule over what is on disk: a fresh read each time (a restart has no memory).
fn tick(data: &Path, now_local: chrono::NaiveDateTime, approved: bool) -> bool {
    let runs = triage_runs(data);
    let newest = runs.iter().find(|r| r.list_id.as_deref() == Some(LIST));
    let queued =
        blocking_reason(data, LIST, SystemTime::now()).is_some_and(|w| w.contains("queued"));
    schedule_due(
        &schedule(),
        now_local,
        SystemTime::now(),
        newest,
        &day,
        &|_| approved,
        queued,
    )
}

#[test]
fn the_schedule_fires_once_per_local_day_across_a_restart_and_waits_for_an_answer() {
    let data = TempDir::new("triage_schedule");
    // Before the time: nothing. After it, with no triage run yet: due.
    assert!(!tick(data.path(), local("2026-11-06", "07:59"), false));
    assert!(tick(data.path(), local("2026-11-06", "08:00"), false));
    // The tick starts a run (its folder is made); a second tick the same day, with all in-memory state gone, starts none.
    let run = triage_run(
        data.path(),
        "r-20261106-080100-aaaa",
        LIST,
        "planning",
        "2026-11-06T08:01:00Z",
    );
    assert!(
        !tick(data.path(), local("2026-11-06", "08:05"), false),
        "unfinished"
    );
    std::fs::write(
        run.join("status.json"),
        json!({"schema": 1, "stage": "done", "prs": []}).to_string(),
    )
    .unwrap();
    assert!(
        !tick(data.path(), local("2026-11-06", "09:30"), true),
        "already started today"
    );
    // The next day: not while the proposal has no approval record; yes once it has.
    assert!(
        !tick(data.path(), local("2026-11-07", "08:30"), false),
        "the previous proposal is unanswered"
    );
    assert!(tick(data.path(), local("2026-11-07", "08:30"), true));
    // An entry that is off never fires.
    let mut off = schedule();
    off.on = false;
    assert!(!schedule_due(
        &off,
        local("2026-11-08", "09:00"),
        SystemTime::now(),
        None,
        &day,
        &|_| true,
        false
    ));
    // A failed run is no proposal and does not block the next day.
    std::fs::write(
        run.join("status.json"),
        json!({"schema": 1, "stage": "failed", "prs": []}).to_string(),
    )
    .unwrap();
    assert!(tick(data.path(), local("2026-11-07", "08:30"), false));
    // A queued entry blocks a second start.
    assert!(!schedule_due(
        &schedule(),
        local("2026-11-07", "08:30"),
        SystemTime::now(),
        None,
        &day,
        &|_| true,
        true
    ));
}

// ---- S12: approval and waves -----------------------------------------------------------------------------------

#[test]
fn a_forged_approval_file_in_the_run_folder_is_ignored_and_nothing_is_queued() {
    let data = TempDir::new("triage_forged");
    let run = triage_run(
        data.path(),
        "r-20261106-090000-aaaa",
        LIST,
        "done",
        "2026-11-06T09:00:00Z",
    );
    let forged = json!({"schema": 1, "triageDigest": digest(&run), "decision": "approved",
        "groups": [{"n": 1, "title": "x", "tickets": ["t1"], "dependsOn": [], "why": ""}], "creates": [], "closes": [], "at": "t"});
    std::fs::write(run.join("triage-approval.json"), forged.to_string()).unwrap();
    assert!(approved_waves(&run).is_none());
    assert!(epics::epic_runs(data.path()).is_empty());
    epics::auto_start_in(data.path(), true);
    let state = dispatch::load(data.path());
    assert!(
        state.queue.is_empty() && state.epics.is_empty(),
        "nothing queued before the signed record exists"
    );
    // A record edited by hand fails its MAC and is refused, not read.
    trust::write_triage(
        &key(),
        data.path(),
        &run,
        TRIAGE_APPROVAL_FILE,
        &serde_json::from_value::<ApprovalRecord>(forged).unwrap(),
    )
    .unwrap();
    let path = trust::trust_dir(data.path(), &run)
        .unwrap()
        .join(TRIAGE_APPROVAL_FILE);
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"approved\"", "\"rejected\"");
    std::fs::write(&path, text).unwrap();
    assert!(
        read_approval(&key(), data.path(), &run).is_err(),
        "a changed record fails its MAC"
    );
}

#[test]
fn approving_needs_the_current_digest_and_only_the_first_decision_counts() {
    let data = TempDir::new("triage_approve");
    let run = triage_run(
        data.path(),
        "r-20261106-090000-bbbb",
        LIST,
        "done",
        "2026-11-06T09:00:00Z",
    );
    let stale = approve(
        &key(),
        data.path(),
        &run,
        "not-the-digest",
        "approved",
        &all_edits(),
        "t",
    );
    assert_eq!(stale.unwrap_err(), STALE);
    assert!(
        read_approval(&key(), data.path(), &run).unwrap().is_none(),
        "nothing was written"
    );
    let record = approve(
        &key(),
        data.path(),
        &run,
        &digest(&run),
        "approved",
        &all_edits(),
        "2026-11-06T09:10:00Z",
    )
    .unwrap();
    assert_eq!(record.groups.len(), 2);
    assert_eq!(record.groups[1].depends_on, [1]);
    // Each ticked create is stored with its exact (capped) name and description.
    assert_eq!(
        record.creates,
        vec![CreateRec {
            index: 0,
            name: "Add a retry".into(),
            description: "Retry the call twice.".into()
        }]
    );
    assert_eq!(record.closes, ["t5"]);
    let again = approve(
        &key(),
        data.path(),
        &run,
        &digest(&run),
        "rejected",
        &Edits::default(),
        "t",
    );
    assert_eq!(again.unwrap_err(), "This proposal was already decided.");
    assert_eq!(
        read_approval(&key(), data.path(), &run)
            .unwrap()
            .unwrap()
            .decision,
        "approved"
    );
    // The proposal view carries the decision.
    let view = overview(data.path());
    assert_eq!(view[0].approval.as_ref().unwrap().decision, "approved");
    assert!(view[0].problem.is_none());
}

#[test]
fn a_rejection_records_nothing_to_queue_or_send() {
    let data = TempDir::new("triage_reject");
    let run = triage_run(
        data.path(),
        "r-20261106-090000-cccc",
        LIST,
        "done",
        "2026-11-06T09:00:00Z",
    );
    let record = approve(
        &key(),
        data.path(),
        &run,
        &digest(&run),
        "rejected",
        &all_edits(),
        "t",
    )
    .unwrap();
    assert!(record.groups.is_empty() && record.creates.is_empty() && record.closes.is_empty());
    assert!(approved_waves(&run).is_none());
}

#[test]
fn bad_edits_are_refused_with_a_reason() {
    let data = TempDir::new("triage_edits");
    let run = triage_run(
        data.path(),
        "r-20261106-090000-dddd",
        LIST,
        "done",
        "2026-11-06T09:00:00Z",
    );
    let d = digest(&run);
    let try_edit = |edit: &dyn Fn(&mut Edits)| {
        let mut e = all_edits();
        edit(&mut e);
        approve(&key(), data.path(), &run, &d, "approved", &e, "t").unwrap_err()
    };
    assert!(
        try_edit(&|e| e.groups[0].depends_on = vec![2]).contains("earlier waves only"),
        "a forward dependency"
    );
    assert!(
        try_edit(&|e| e.groups[1].tickets.clear()).contains("no ticket"),
        "an empty group"
    );
    assert!(try_edit(&|e| e.groups[0].tickets.push("ghost".into())).contains("not a ticket"));
    assert!(try_edit(&|e| e.groups[1].tickets = vec!["t1".into()]).contains("two groups"));
    assert!(try_edit(&|e| e.creates = vec![7]).contains("no suggested ticket 7"));
    assert!(
        try_edit(&|e| e.closes = vec!["t1".into()]).contains("not a ticket this proposal closes")
    );
    assert!(
        read_approval(&key(), data.path(), &run).unwrap().is_none(),
        "every refusal wrote nothing"
    );
}

fn merged_run(data: &Path, id: &str, state: &str) -> PathBuf {
    let dir = data.join("runs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("status.json"),
        json!({"schema": 1, "stage": "done", "prs": [{"id": "PR1"}]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("app-github.json"),
        json!({"prs": {"PR1": {"ci": "pass", "prState": state, "at": "t"}}}).to_string(),
    )
    .unwrap();
    dir
}

#[test]
fn an_approved_triage_group_is_queued_with_auto_start_off_and_the_next_waits_for_the_merge() {
    let data = TempDir::new("triage_waves");
    let run = triage_run(
        data.path(),
        "r-20261106-090000-eeee",
        LIST,
        "done",
        "2026-11-06T09:00:00Z",
    );
    approve(
        &key(),
        data.path(),
        &run,
        &digest(&run),
        "approved",
        &all_edits(),
        "t",
    )
    .unwrap();
    // run.autoStartWaves is off (`all` false): the triage epic still starts its ready group.
    epics::auto_start_in(data.path(), false);
    let state = dispatch::load(data.path());
    assert_eq!(state.queue.len(), 1, "group 1 only");
    let entry = state.queue[0].clone();
    assert_eq!(entry.wave.as_ref().unwrap().n, 1);
    assert_eq!(
        entry.params.clickup.as_ref().unwrap().ticket_ids,
        ["t1", "t2"]
    );
    assert_eq!(
        entry.params.bmad_mode, None,
        "the triage run's own off is not copied: the operator's default applies"
    );
    let epic = &state.epics[0];
    assert_eq!(
        (epic.source.as_deref(), epic.auto_start),
        (Some("triage"), true)
    );
    // Group 2 waits: group 1 has no run yet, then its PR is open, then merged.
    epics::auto_start_in(data.path(), false);
    assert_eq!(dispatch::load(data.path()).queue.len(), 1);
    let w1 = merged_run(data.path(), "r-20261106-100000-wave1", "OPEN");
    let mut state = dispatch::load(data.path());
    dispatch::finish(&mut state, &entry.id, Ok(Some(w1.display().to_string())));
    dispatch::save(data.path(), &state).unwrap();
    epics::auto_start_in(data.path(), false);
    assert_eq!(
        dispatch::load(data.path()).queue.len(),
        0,
        "wave 1's PR is open: group 2 waits"
    );
    std::fs::write(
        w1.join("app-github.json"),
        json!({"prs": {"PR1": {"ci": "pass", "prState": "MERGED", "at": "t"}}}).to_string(),
    )
    .unwrap();
    epics::auto_start_in(data.path(), false);
    let state = dispatch::load(data.path());
    assert_eq!(state.queue.len(), 1);
    assert_eq!(state.queue[0].wave.as_ref().unwrap().n, 2);
}

#[test]
fn an_epic_plan_wave_still_copies_its_runs_bmad_mode_and_needs_auto_start() {
    let data = TempDir::new("triage_epicplan");
    let epic = data.join("runs").join("r-epic");
    std::fs::create_dir_all(&epic).unwrap();
    std::fs::write(epic.join("request.md"), "folder: /w\n").unwrap();
    std::fs::write(
        epic.join("status.json"),
        json!({"schema": 1, "stage": "done"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        epic.join("tickets.json"),
        json!({"tasks": [{"id": "t1"}]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        epic.join("waves.json"),
        json!({"waves": [{"n": 1, "title": "One", "tickets": ["t1"], "dependsOn": [], "why": "x"}]})
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        epic.join("meta.json"),
        json!({"mode": "epic-plan", "folder": "/w", "bmad": {"mode": "auto"}, "account": {"id": "a"}})
            .to_string(),
    )
    .unwrap();
    epics::auto_start_in(data.path(), false);
    assert!(
        dispatch::load(data.path()).queue.is_empty(),
        "an epic-plan epic keeps the setting"
    );
    epics::auto_start_in(data.path(), true);
    let state = dispatch::load(data.path());
    assert_eq!(state.queue[0].params.bmad_mode.as_deref(), Some("auto"));
    assert_eq!(state.epics[0].source, None);
}

// ---- S13: the ClickUp writes -----------------------------------------------------------------------------------

#[derive(Default)]
struct Saves(Vec<PostedRecord>);

impl PostStore for Saves {
    fn save(&mut self, posted: &PostedRecord) -> Result<(), String> {
        self.0.push(posted.clone());
        Ok(())
    }
}

fn record(creates: Vec<CreateRec>, closes: Vec<&str>) -> ApprovalRecord {
    ApprovalRecord {
        schema: 1,
        triage_digest: "d".into(),
        decision: "approved".into(),
        groups: vec![],
        creates,
        closes: closes.into_iter().map(String::from).collect(),
        at: "t".into(),
    }
}

fn create(index: usize, name: &str, description: &str) -> CreateRec {
    CreateRec {
        index,
        name: name.into(),
        description: description.into(),
    }
}

fn snapshot_ids() -> HashSet<String> {
    ["t1", "t5"].iter().map(|s| s.to_string()).collect()
}

fn run_apply(
    api: &Recorded,
    record: &ApprovalRecord,
    posted: &mut PostedRecord,
) -> Vec<ApplyOutcome> {
    let (gate, clock, token) = (
        Gate::new(),
        FakeClock::at(NOW),
        ClickUpToken::parse("pk_test_TOKEN").unwrap(),
    );
    let client = ClickUp::with(api, &token, &gate, &clock);
    let mut saves = Saves::default();
    apply(
        &client,
        LIST,
        record,
        &snapshot_ids(),
        posted,
        &mut saves,
        &|| "t".to_string(),
    )
    .unwrap()
}

fn post_path() -> String {
    format!("POST /list/{LIST}/task")
}

#[test]
fn one_ticked_create_sends_exactly_one_post_and_no_status_change_and_a_second_apply_sends_nothing()
{
    let api = Recorded::default().on(&post_path(), ok("{}".into()));
    let rec = record(vec![create(0, "Add a retry", "Retry twice.")], vec![]);
    let mut posted = PostedRecord::default();
    let out = run_apply(&api, &rec, &mut posted);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].result, "posted");
    let writes = api.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1, "one POST and no status change");
    assert_eq!(writes[0].method, "POST");
    assert_eq!(writes[0].path, format!("/list/{LIST}/task"));
    assert_eq!(
        writes[0].body,
        json!({"name": "Add a retry", "description": "Retry twice."})
    );
    // Re-applying sends nothing (the Recorded transport panics on an unexpected call).
    assert!(run_apply(&api, &rec, &mut posted).is_empty());
    assert_eq!(api.writes.lock().unwrap().len(), 1);
}

#[test]
fn a_maybe_is_not_resent_and_a_failed_one_is_retried() {
    let api = Recorded::default().on(&post_path(), Err(NoReply::Failed));
    let rec = record(vec![create(0, "A", "d")], vec![]);
    let mut posted = PostedRecord::default();
    assert_eq!(run_apply(&api, &rec, &mut posted)[0].result, "maybe");
    assert!(
        run_apply(&api, &rec, &mut posted).is_empty(),
        "a maybe is never resent"
    );
    assert_eq!(api.writes.lock().unwrap().len(), 1);
    let refused = Recorded::default()
        .on(&post_path(), status(422))
        .on(&post_path(), ok("{}".into()));
    let mut posted = PostedRecord::default();
    assert_eq!(run_apply(&refused, &rec, &mut posted)[0].result, "failed");
    assert_eq!(
        run_apply(&refused, &rec, &mut posted)[0].result,
        "posted",
        "Retry sends a failed item again"
    );
    assert_eq!(refused.writes.lock().unwrap().len(), 2);
}

#[test]
fn a_secret_in_a_suggested_ticket_is_blocked_and_never_sent() {
    let api = Recorded::default();
    let rec = record(
        vec![create(
            0,
            "Fix the sync",
            "use ghp_abcdefghijklmnopqrstuvwxyz0123 to log in",
        )],
        vec![],
    );
    let mut posted = PostedRecord::default();
    let out = run_apply(&api, &rec, &mut posted);
    assert_eq!(out[0].result, "blocked");
    assert!(api.writes.lock().unwrap().is_empty());
    assert!(
        run_apply(&api, &rec, &mut posted).is_empty(),
        "a blocked item stays blocked"
    );
}

#[test]
fn unticked_items_are_never_sent_and_a_close_moves_a_snapshot_ticket_to_the_closed_status() {
    let list = json!({"id": LIST, "name": "Tasks", "statuses": [
        {"status": "to do", "type": "open"}, {"status": "complete", "type": "closed"}]})
    .to_string();
    let api = Recorded::default()
        .on(&format!("/list/{LIST}"), ok(list))
        .on("PUT /task/t5", ok("{}".into()));
    // No create is ticked; t9 is not in the snapshot and is never touched.
    let rec = record(vec![], vec!["t5", "t9"]);
    let mut posted = PostedRecord::default();
    let out = run_apply(&api, &rec, &mut posted);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].message, "moved to complete");
    let writes = api.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        (writes[0].method, writes[0].path.as_str()),
        ("PUT", "/task/t5")
    );
    assert_eq!(writes[0].body, json!({"status": "complete"}));
}

#[test]
fn changing_triage_md_or_json_after_approval_does_not_change_what_is_sent() {
    let data = TempDir::new("triage_apply_text");
    let run = triage_run(
        data.path(),
        "r-20261106-090000-ffff",
        LIST,
        "done",
        "2026-11-06T09:00:00Z",
    );
    let rec = approve(
        &key(),
        data.path(),
        &run,
        &digest(&run),
        "approved",
        &all_edits(),
        "t",
    )
    .unwrap();
    // Rewrite both proposal files after approval.
    let mut json = read_json(&run.join("triage.json")).unwrap();
    json["suggestions"][0]["title"] = json!("Delete everything");
    json["suggestions"][0]["description"] = json!("rm -rf");
    std::fs::write(run.join("triage.json"), json.to_string()).unwrap();
    std::fs::write(run.join("triage.md"), "## Group 1: Evil\n").unwrap();
    let signed = read_approval(&key(), data.path(), &run).unwrap().unwrap();
    assert_eq!(signed, rec);
    let api = Recorded::default().on(&post_path(), ok("{}".into()));
    let mut posted = PostedRecord::default();
    run_apply(
        &api,
        &ApprovalRecord {
            closes: vec![],
            ..signed
        },
        &mut posted,
    );
    let writes = api.writes.lock().unwrap().clone();
    assert_eq!(
        writes[0].body,
        json!({"name": "Add a retry", "description": "Retry the call twice."})
    );
}

#[test]
fn create_task_is_called_only_from_the_triage_module() {
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let callers: Vec<String> = files
        .iter()
        .filter(|f| !f.ends_with("run/triage/tests.rs"))
        .filter(|f| {
            let live =
                crate::test_support::without_test_items(&std::fs::read_to_string(f).unwrap());
            live.contains(".create_task(")
        })
        .map(|f| f.strip_prefix(&src).unwrap().display().to_string())
        .collect();
    assert_eq!(callers, ["run/triage.rs"], "the only caller of create_task");
}

#[test]
fn a_failed_scheduled_start_is_not_tried_again_on_the_next_tick() {
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let mut failed = BTreeMap::new();
    assert!(!start_backing_off(&failed, "L1", t0));
    failed.insert("L1".to_string(), t0);
    assert!(start_backing_off(
        &failed,
        "L1",
        t0 + Duration::from_secs(2)
    ));
    assert!(!start_backing_off(
        &failed,
        "L2",
        t0 + Duration::from_secs(2)
    ));
    assert!(!start_backing_off(&failed, "L1", t0 + START_RETRY));
}
