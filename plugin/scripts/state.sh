#!/usr/bin/env bash
# Run-folder state for the eagle-eye orchestrator. Every write is validated and
# atomic (a mktemp file in the same folder, then mv). Needs python3 (preflight checks it).
#
#   state.sh preflight <run> [resume]         check python3, git, DCV2_RUN_DIR and request.md;
#                                             with resume, also log "resumed at stage <stage>";
#                                             print folder= / repos= / tickets= / reposJson= / testmode= /
#                                             maxImplementers= / memory=
#   state.sh testmode                         print the test mode, from the env DCV2_TEST_MODE only
#   state.sh put <run>/status.json  < json    validate the whole document, then write it
#   state.sh log <run> [<stage>]    < text    append "<UTC time> <stage> <text>" to log.md (text on stdin;
#                                             stage defaults to status.json's)
#   state.sh now                              print the UTC time (ISO-8601)
#   state.sh approve <run> plan|ship user|test
#                                             record the user's yes at stop #1 / #2 (test: only when
#                                             DCV2_TEST_MODE has auto-approve); ship also sets stage shipping
#   state.sh plan <run> prs|tickets|stories   print only that part of plan.md
#   state.sh base <repo path>                 print "base=<b> baseCommit=<sha> remote=<name|none>"
#   state.sh worktree <run> <repo path> <pr id> <base commit>
#                                             make (or reuse / re-make) the PR's worktree beside the
#                                             repo, record it in <run>/.guard.json, print
#                                             "branch=<b> worktree=<path>". Never touches the main checkout.
#                                             A newly made worktree also gets repos.json's copyFiles
#                                             (git-ignored files only) and its setup (or the lockfile's
#                                             install), printing "setup=ok|failed|none log=<path>"
#   state.sh mode <run>                       print "reviewMode=github|local" (meta.json; missing = local)
#   state.sh ship-check <run>                 per PR: remote, and whether its base is on the remote
#   state.sh ship-pr <run> <pr id>            local mode: push the PR's branch and open its GitHub PR
#                                             (stage shipping + ship approval only); updates status.json
# github review mode only (meta.json reviewMode "github"; after the plan approval):
#   state.sh draft-pr <run> <pr id>           push the branch (never force) and open a DRAFT PR (or find
#                                             the branch's existing one); records prUrl, draft, state drafted
#   state.sh push <run> <pr id>               push new commits of a drafted PR (never force)
#   state.sh post-review <run> <pr id> <round>
#                                             post reviews/<pr>-round<n>.md as one PR comment (secret-scanned,
#                                             capped); records prs[].comments; never posts a round twice
#   state.sh comment <run> <pr id> fixes <round>
#                                             post the fixed "Round n fixes: <commits>" comment (once per round)
#   state.sh ready <run> <pr id>              after the ship approval: final PR body + `gh pr ready`
#                                             (opens a ready PR when no draft exists); updates status.json
#   state.sh writeback <run> [--force]        write writeback.json from tickets.json, plan.md and
#                                             status.json (refuses to overwrite it without --force)
#   state.sh gate <run> <pr id> PASS|CONCERNS|FAIL|WAIVED "<reason>" [--review <file>]
#                                             write gates/<pr id>.yml (A1). PASS/CONCERNS need the review
#                                             file (default reviews/<pr>-round<review.round>.md) to say
#                                             APPROVED; WAIVED only over a CONCERNS/FAIL gate, with the
#                                             user's reason
#   state.sh report <run>                     write report.md from status.json, plan.md, log.md and the
#                                             gates (R10); put does it itself at stage done / failed
#
# Limits put enforces: at most DCV2_MAX_IMPLEMENTERS (default 3) PRs building/fixing at once (R6);
# a PR becomes reviewed only with a PASS, CONCERNS or WAIVED gate file (A1).
#
# Exit 1 = refused (the message says why; nothing was written). Exit 3 = needs attention.
set -euo pipefail

py_works() { command -v python3 >/dev/null 2>&1 && python3 -c 'import json, sys; sys.exit(0)' >/dev/null 2>&1; }

IFS= read -r -d '' PY <<'PY' || true
import datetime, json, os, re, shutil, signal, subprocess, sys, tempfile

STAGES = ["planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"]
STATES = ["pending", "building", "drafted", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"]
COMMENT_CAP = 60000
GITHUB_STAGES = ("building", "ship-approval", "shipping")
PLUGIN_ROOT = os.environ.get("DCV2_PLUGIN_ROOT", "")
CHECK_RESULTS = ["not-run", "pass", "fail", "none", "error"]
POST_APPROVAL = ("building", "ship-approval", "shipping", "done")
SHIPPED = ("opened", "pushed", "local-only")
# Run files only the app writes (docs/run-folder.md); `put` refuses them.
APP_FILES = ("tickets.json", "meta.json", "repos.json", "writeback-posted.json", "tickets-pulls.json", "tickets-update.md", "app-checks.json")
ISO = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
AGENT_FOR_STATE = {"building": "implementer", "fixing": "implementer", "reviewing": "pr-reviewer"}
GATES = ("PASS", "CONCERNS", "FAIL", "WAIVED")


def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def die(msg, code=1):
    print(msg, file=sys.stderr)
    sys.exit(code)


def read(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return ""


def load_json(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def atomic_write(dest, text):
    d = os.path.dirname(os.path.abspath(dest))
    fd, tmp = tempfile.mkstemp(prefix="." + os.path.basename(dest) + ".", suffix=".tmp", dir=d)
    try:
        with os.fdopen(fd, "w") as f:
            f.write(text)
        os.replace(tmp, dest)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def append_log(run, stage, text):
    with open(os.path.join(run, "log.md"), "a") as f:
        f.write(f"{now()} {stage} {' '.join(text.split())}\n")


def testmode():
    tm = {"auto": False, "halt": None}
    for tok in re.split(r"[,\s]+", os.environ.get("DCV2_TEST_MODE", "").strip()):
        if not tok:
            continue
        if tok == "auto-approve":
            tm["auto"] = True
        elif re.fullmatch(r"halt-after=PR\d+", tok):
            tm["halt"] = tok.split("=", 1)[1]
        else:
            die(f"DCV2_TEST_MODE: unknown value {tok!r} (allowed: auto-approve, halt-after=PR<n>)")
    return tm


def git(*args, cwd=None):
    return subprocess.run(["git", *args], capture_output=True, text=True, cwd=cwd)


def run_cmd(args, cwd=None, timeout=120):
    try:
        return subprocess.run(args, capture_output=True, text=True, cwd=cwd, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired) as e:
        return subprocess.CompletedProcess(args, 127, "", str(e))


def first_line(s):
    return next((l.strip() for l in (s or "").splitlines() if l.strip()), "")


def canon(repo_path, run_id, pr_id):
    repo = os.path.realpath(repo_path)
    lc = str(pr_id).lower()
    return f"dcv2/{run_id}-{lc}", os.path.join(os.path.dirname(repo), os.path.basename(repo) + "-worktrees", f"dcv2-{run_id}-{lc}")


def resumed(run):
    return "resumed at stage" in read(os.path.join(run, "log.md"))


def review_mode(run):
    """meta.json's reviewMode (written by the app at Start). Missing or unknown = local (old runs keep their flow)."""
    m = load_json(os.path.join(run, "meta.json"))
    return "github" if isinstance(m, dict) and m.get("reviewMode") == "github" else "local"


def remote_of(repo):
    remotes = git("-C", repo, "remote").stdout.split()
    return "origin" if "origin" in remotes else (remotes[0] if remotes else None)


def on_remote(repo, remote, branch):
    return bool(branch) and git("-C", repo, "ls-remote", "--exit-code", "--heads", remote, branch).returncode == 0


# ---- status.json -------------------------------------------------------------------------

def check_approval(v, name, problems):
    if v is None:
        return
    if not (isinstance(v, dict) and v.get("by") in ("user", "test") and ISO.match(str(v.get("at")))):
        problems.append(f"{name} must be null or {{by: user|test, at: <UTC>}}")


def validate_status(doc, run):
    p = []
    if doc.get("schema") != 1:
        p.append("schema must be 1")
    stage = doc.get("stage")
    if stage not in STAGES:
        p.append(f"stage {stage!r} not one of {STAGES}")
    if doc.get("waitingFor") not in (None, "plan-approval", "ship-approval"):
        p.append("waitingFor must be null, plan-approval or ship-approval")
    run_id = os.path.basename(os.path.realpath(run))
    if doc.get("runId") != run_id:
        p.append(f"runId must be the run folder's name {run_id!r}")
    plan = doc.get("plan")
    if plan is not None and not isinstance(plan, dict):
        p.append("plan must be null or an object")
        plan = None
    check_approval((plan or {}).get("approval"), "plan.approval", p)
    check_approval(doc.get("shipApproval"), "shipApproval", p)
    paused = doc.get("paused")
    if paused is not None and not (isinstance(paused, dict) and isinstance(paused.get("reason"), str) and paused["reason"]):
        p.append("paused must be null or {reason: <one line>, at: <UTC>}")
    if stage in POST_APPROVAL and not (plan or {}).get("approval"):
        p.append(f"stage {stage} needs the plan approved first: after the user's yes run `state.sh approve <run> plan user`")
    if stage in ("shipping", "done") and not doc.get("shipApproval"):
        p.append(f"stage {stage} needs the ship approval first: after the user's yes run `state.sh approve <run> ship user`")
    if stage == "failed" and not doc.get("error"):
        p.append("stage failed needs error set (one line)")
    prs = doc.get("prs")
    if not isinstance(prs, list):
        p.append("prs must be a list")
        prs = []
    seen = set()
    for pr in prs:
        if not isinstance(pr, dict):
            p.append("each prs[] entry must be an object")
            continue
        pid = pr.get("id")
        if not re.fullmatch(r"PR\d+", str(pid)) or pid in seen:
            p.append(f"PR id {pid!r} must be PR<n> and unique")
        seen.add(pid)
        state = pr.get("state")
        if state not in STATES:
            p.append(f"{pid}: state {state!r} not one of {STATES}")
        if not isinstance(pr.get("dependsOn"), list):
            p.append(f"{pid}: dependsOn must be a list")
        chk = pr.get("check")
        if not isinstance(chk, dict):
            p.append(f"{pid}: check must be an object")
            chk = {}
        result = chk.get("result")
        if result not in CHECK_RESULTS:
            p.append(f"{pid}: check.result {result!r} not one of {CHECK_RESULTS}")
        if chk.get("cwd") is not None and not os.path.isabs(str(chk.get("cwd"))):
            p.append(f"{pid}: check.cwd must be an absolute path or null")
        if chk.get("log") is not None and not isinstance(chk.get("log"), str):
            p.append(f"{pid}: check.log must be a path or null")
        rev = pr.get("review")
        if not isinstance(rev, dict):
            p.append(f"{pid}: review must be an object")
            rev = {}
        rnd = rev.get("round")
        if not (isinstance(rnd, int) and 0 <= rnd <= 3):
            p.append(f"{pid}: review.round must be an int 0-3")
            rnd = 0
        if state in ("reviewing", "fixing") and rnd < 1:
            p.append(f"{pid}: state {state} needs review.round = the round in flight (1-3), set when the round starts")
        if state == "reviewed" and result not in ("pass", "none"):
            p.append(f"{pid}: reviewed needs check.result pass or none (got {result!r}); a failing or broken check means needs-attention")
        if state == "needs-attention" and not pr.get("note"):
            p.append(f"{pid}: needs-attention needs a one-line note")
        if state == "drafted" and not pr.get("prUrl"):
            p.append(f"{pid}: drafted needs the draft PR's prUrl (set only by `state.sh draft-pr`)")
        if "draft" in pr and not isinstance(pr.get("draft"), bool):
            p.append(f"{pid}: draft must be true or false")
        cm = pr.get("comments")
        if cm is not None and not (isinstance(cm, list) and all(isinstance(c, dict) and isinstance(c.get("round"), int)
                                                                  and isinstance(c.get("url"), str) for c in cm)):
            p.append(f"{pid}: comments must be a list of {{round: <n>, url: <comment url>}}")
        if stage in POST_APPROVAL:
            ids = (pr.get("stories") or {}).get("ids") if isinstance(pr.get("stories"), dict) else None
            if not (isinstance(ids, list) and ids and all(isinstance(i, str) for i in ids)):
                p.append(f"{pid}: stories.ids must list the PR's story ids (from plan.md)")
            if not pr.get("repoPath") or not os.path.isabs(str(pr.get("repoPath"))):
                p.append(f"{pid}: repoPath must be the repo's absolute path")
            else:
                branch, wt = canon(pr["repoPath"], run_id, pid)
                if pr.get("branch") != branch:
                    p.append(f"{pid}: branch must be {branch!r} (as printed by state.sh worktree)")
                if pr.get("worktree") is not None and os.path.realpath(str(pr["worktree"])) != wt:
                    p.append(f"{pid}: worktree must be {wt!r} (as printed by state.sh worktree) or null")
    return p


def prepare_status(doc, old, trusted, run_dir=None):
    """Stamp times and derived fields; keep what only state.sh may change."""
    notes = []
    t = now()
    doc["updatedAt"] = t
    old = old if isinstance(old, dict) else {}
    oplan = old.get("plan") if isinstance(old.get("plan"), dict) else {}
    if not trusted:
        if isinstance(doc.get("plan"), dict):
            want = doc["plan"].get("approval")
            claimed = bool(want or doc["plan"].get("approved"))
            if claimed != bool(oplan.get("approval")) or (want and want != oplan.get("approval")):
                notes.append("note: the plan approval is set only by `state.sh approve`; kept the recorded value")
            doc["plan"]["approval"] = oplan.get("approval")
        if doc.get("shipApproval") and doc.get("shipApproval") != old.get("shipApproval"):
            notes.append("note: the ship approval is set only by `state.sh approve`; kept the recorded value")
        doc["shipApproval"] = old.get("shipApproval")
    if isinstance(doc.get("plan"), dict):
        doc["plan"]["approved"] = bool(doc["plan"].get("approval"))
    doc.setdefault("shipApproval", None)
    doc.setdefault("paused", None)
    if isinstance(doc.get("paused"), dict) and not doc["paused"].get("at"):
        doc["paused"]["at"] = t
    oprs = {p.get("id"): p for p in old.get("prs") or [] if isinstance(p, dict)}
    bad = []
    for pr in doc.get("prs") or []:
        if not isinstance(pr, dict):
            continue
        o = oprs.get(pr.get("id")) or {}
        if not trusted and o.get("state") in SHIPPED and pr.get("state") != o.get("state"):
            bad.append(f"{pr.get('id')} is {o['state']} (written by state.sh ship-pr); re-read status.json and keep it")
        if not trusted:  # prUrl is set only by state.sh (ship-pr / draft-pr / ready): a put never changes it
            if pr.get("prUrl") not in (None, o.get("prUrl")):
                notes.append(f"note: {pr.get('id')}.prUrl is set only by state.sh; kept the recorded value ({o.get('prUrl')})")
            pr["prUrl"] = o.get("prUrl")
        if not trusted:  # draft and comments are written only by state.sh's GitHub commands
            for k in ("draft", "comments"):
                if k in o:
                    pr[k] = o[k]
                else:
                    pr.pop(k, None)
        state = pr.get("state")
        if state == "reviewed" and o.get("state") != "reviewed" and run_dir:
            g = read_gate(run_dir, pr.get("id"))
            rnd = (pr.get("review") or {}).get("round") if isinstance(pr.get("review"), dict) else None
            if not g or g.get("gate") == "FAIL" or g.get("round") != rnd:
                why = (f"gate now {(g or {}).get('gate') or 'missing'}" if not g or g.get("gate") == "FAIL"
                       else f"the gate is for review round {g.get('round')}, the PR is at round {rnd}")
                bad.append(f"{pr.get('id')}: reviewed needs this round's gate first: `state.sh gate <run> {pr.get('id')} PASS|CONCERNS \"<reason>\"` "
                           f"from the PR Reviewer's reply ({why})")
        pr["agent"] = AGENT_FOR_STATE.get(state)
        pr["startedAt"] = o.get("startedAt") or (t if state not in (None, "pending") else None)
        if isinstance(pr.get("check"), dict):
            pr["check"].setdefault("cwd", pr.get("worktree"))
            pr["check"].setdefault("log", None)
        same = lambda d: {k: v for k, v in d.items() if k not in ("updatedAt", "startedAt", "agent")}
        pr["updatedAt"] = o["updatedAt"] if o.get("updatedAt") and same(o) == same(pr) else t
    return notes, bad


def max_implementers():
    v = os.environ.get("DCV2_MAX_IMPLEMENTERS", "").strip() or "3"
    if not re.fullmatch(r"[1-9]\d{0,2}", v):
        die(f"DCV2_MAX_IMPLEMENTERS must be a whole number 1-999 (got {v!r})")
    return int(v)


def read_gate(run, pid):
    """{gate, reason, updated, review, round} from gates/<pid>.yml, or None."""
    text = read(os.path.join(run, "gates", f"{pid}.yml"))
    if not text:
        return None
    g = {}
    for line in text.splitlines():
        m = re.match(r"^(\w+):\s*(.*?)\s*$", line)
        if m:
            v = m.group(2)
            try:
                v = json.loads(v)
            except ValueError:
                pass
            g[m.group(1)] = v
    return g if g.get("gate") in GATES else None


def busy_ids(doc):
    return [p.get("id") for p in (doc or {}).get("prs") or [] if isinstance(p, dict) and p.get("state") in ("building", "fixing")]


def gates(doc, run, old=None):
    """Refusals and reminders that depend on the run, not on the document's shape."""
    tm = testmode()
    out = []
    stage = doc.get("stage")
    cap = max_implementers()
    busy, was = busy_ids(doc), busy_ids(old if isinstance(old, dict) else {})
    # Refuse only a write that adds work over the cap, so a run already over it (cap lowered,
    # resume) can still record PRs finishing.
    if len(busy) > cap and (len(busy) > len(was) or set(busy) - set(was)):
        return (f"NOT WRITTEN: {len(busy)} PRs building/fixing ({', '.join(busy)}), over the cap of {cap} Implementers at once "
                f"(DCV2_MAX_IMPLEMENTERS). Keep the others pending and start them when one finishes."), out
    if tm["halt"] and not resumed(run):
        pid = tm["halt"]
        prs = {p.get("id"): p for p in doc.get("prs") or [] if isinstance(p, dict)}
        if prs.get(pid, {}).get("state") == "reviewed":
            others = [i for i, p in prs.items() if i != pid and p.get("state") != "pending"]
            if others or stage != "building":
                return f"NOT WRITTEN: test-mode halt-after={pid}: {pid} is reviewed, so the run must stop now with stage 'building' and every other PR 'pending'. Write that, log 'halted after {pid} (test-mode)', reply 'HALTED after {pid}' and stop.", out
            out.append(f"HALT (test-mode): {pid} is reviewed. Log 'halted after {pid} (test-mode)', reply 'HALTED after {pid}' and stop now. Launch nothing else.")
    if stage == "done":
        if os.path.isfile(os.path.join(run, "tickets.json")) and not os.path.isfile(os.path.join(run, "writeback.json")):
            return "NOT WRITTEN: tickets.json exists but writeback.json does not. Run `state.sh writeback <run>` first, then write stage done.", out
        if not doc.get("ship"):
            return "NOT WRITTEN: stage done needs \"ship\" set ({remote, pushed, note}). Do Step 4 first.", out
        early = [p.get("id") for p in doc.get("prs") or [] if isinstance(p, dict) and p.get("state") in ("reviewed", "building", "drafted", "reviewing", "fixing")]
        if early:
            how = "state.sh ready" if review_mode(run) == "github" else "state.sh ship-pr"
            return f"NOT WRITTEN: stage done but {', '.join(early)} not shipped or finished; run `{how} <run> <pr id>` for reviewed PRs first.", out
    if doc.get("waitingFor") and tm["auto"]:
        what = "plan" if doc["waitingFor"] == "plan-approval" else "ship"
        out.append(f"AUTO-APPROVE (test-mode): do not wait for the user. Run `state.sh approve <run> {what} test` and continue past {doc['waitingFor']} now.")
    return None, out


def write_status(run, doc, trusted=False):
    dest = os.path.join(run, "status.json")
    old = load_json(dest)
    notes, bad = prepare_status(doc, old, trusted, run)
    problems = bad + validate_status(doc, run)
    if problems:
        die(f"NOT WRITTEN: {dest}: " + "; ".join(problems))
    refusal, out = gates(doc, run, old)
    if refusal:
        die(refusal)
    atomic_write(dest, json.dumps(doc, indent=2) + "\n")
    if doc.get("stage") in ("done", "failed"):
        try:
            write_report(run)
            out.append(f"wrote {os.path.join(run, 'report.md')}")
        except Exception as e:  # status.json is written; a report problem must not look like a refused write
            print(f"note: report.md not written ({type(e).__name__}: {e}); run `state.sh report <run>`", file=sys.stderr)
    return notes + out


def load_status(run):
    st = load_json(os.path.join(run, "status.json"))
    if not isinstance(st, dict):
        die(f"no valid status.json in {run}")
    return st


# ---- commands ------------------------------------------------------------------------------

def cmd_put(dest):
    name = os.path.basename(dest)
    if name in ("writeback.json", ".guard.json"):
        die(f"NOT WRITTEN: {name} is written only by its own state.sh command")
    if name in APP_FILES:
        die(f"NOT WRITTEN: {name} is written only by the app")
    text = sys.stdin.read()
    try:
        doc = json.loads(text)
    except ValueError as e:
        die(f"NOT WRITTEN: invalid JSON for {dest}: {e}")
    if name != "status.json":
        atomic_write(dest, json.dumps(doc, indent=2) + "\n")
        print(f"wrote {dest}")
        return
    if not isinstance(doc, dict):
        die(f"NOT WRITTEN: {dest} must be a JSON object")
    run = os.path.dirname(os.path.abspath(dest))
    msgs = write_status(run, doc)
    print(f"wrote {dest}")
    for m in msgs:
        print(m)


def cmd_testmode():
    tm = testmode()
    print(f"auto-approve={'yes' if tm['auto'] else 'no'} halt-after={tm['halt'] or 'none'}")


def cmd_preflight(run, resume=False):
    problems = []
    if not os.path.isdir(run):
        die(f"PREFLIGHT FAILED: no run folder {run}")
    real = os.path.realpath(run)
    env = os.environ.get("DCV2_RUN_DIR", "")
    if not env or os.path.realpath(env) != real:
        problems.append(f"DCV2_RUN_DIR must be set to the run folder when claude is launched (got {env or 'nothing'}): "
                        f"the guard hook uses it to know this run. Relaunch with DCV2_RUN_DIR={real}")
    if run_cmd(["git", "--version"]).returncode != 0:
        problems.append("git does not run")
    req = read(os.path.join(run, "request.md"))
    if not req:
        problems.append("request.md is missing or empty")
    folders = sorted(set(re.findall(r"^folder:\s*(\S.*?)\s*$", req, re.M)))
    folder = folders[0] if len(folders) == 1 else None
    if not folders:
        problems.append("request.md has no folder: line")
    elif len(folders) > 1:
        problems.append("request.md has more than one folder: line")
    elif not (os.path.isabs(folder) and os.path.isdir(folder)):
        problems.append(f"folder {folder} is not an absolute path to a folder")
    repos = re.search(r"^repos:\s*(\S.*?)\s*$", req, re.M)
    tm = testmode()
    if problems:
        die("PREFLIGHT FAILED: " + "; ".join(problems))
    if re.search(r"^\s*test-mode\s*:", req, re.M | re.I):
        print("note: request.md has a test-mode line; it is ignored (test mode comes only from DCV2_TEST_MODE)")
    print(f"folder={folder}")
    print(f"repos={repos.group(1) if repos else '-'}")
    print(f"tickets={'yes' if os.path.isfile(os.path.join(run, 'tickets.json')) else 'no'}")
    print(f"reposJson={'yes' if os.path.isfile(os.path.join(run, 'repos.json')) else 'no'}")
    print(f"testmode=auto-approve:{'yes' if tm['auto'] else 'no'},halt-after:{tm['halt'] or 'none'}")
    print(f"maxImplementers={max_implementers()}")
    mem = os.environ.get("DCV2_MEMORY_DIR", "")
    print(f"memory={os.path.realpath(mem) if mem else 'none'}")
    print(f"reviewMode={review_mode(run)}")
    if resume:
        st = load_json(os.path.join(run, "status.json"))
        if isinstance(st, dict) and st.get("stage") in STAGES:
            append_log(run, st["stage"], f"resumed at stage {st['stage']}")
            print(f"resumed: logged 'resumed at stage {st['stage']}' (test-mode halt-after no longer applies)")
        else:
            print("resume: no valid status.json: start a fresh run (Step 0)")


def cmd_approve(run, what, by):
    if what not in ("plan", "ship") or by not in ("user", "test"):
        die("usage: state.sh approve <run> plan|ship user|test")
    if by == "test" and not testmode()["auto"]:
        die("NOT APPROVED: DCV2_TEST_MODE has no auto-approve. Ask the user; after their yes use `approve <run> " + what + " user`.")
    st = load_status(run)
    t = now()
    if what == "plan":
        if st.get("stage") not in ("planning", "plan-approval") or not isinstance(st.get("plan"), dict) \
                or not os.path.isfile(os.path.join(run, "plan.md")):
            die(f"NOT APPROVED: the plan can be approved only at stop #1 with plan.md written (stage {st.get('stage')})")
        st["plan"]["approval"] = {"by": by, "at": t}
        st["stage"] = "plan-approval"
        line = "plan approved by the user" if by == "user" else "plan auto-approved (test-mode)"
    else:
        if st.get("stage") != "ship-approval":
            die(f"NOT APPROVED: shipping can be approved only at stop #2 (stage {st.get('stage')})")
        st["shipApproval"] = {"by": by, "at": t}
        st["stage"] = "shipping"
        line = "ship approved by the user" if by == "user" else "ship auto-approved (test-mode)"
    st["waitingFor"] = None
    st["paused"] = None
    for m in write_status(run, st, trusted=True):
        print(m)
    append_log(run, st["stage"], line)
    print(f"approved: {line}")


def cmd_plan(run, what):
    text = read(os.path.join(run, "plan.md"))
    if not text:
        die(f"no plan.md in {run}")
    if what == "stories":
        print("\n".join(l for l in text.splitlines() if re.match(r"^###\s+S\d+", l)))
        return
    heading = {"prs": "Pull requests", "tickets": "Tickets"}.get(what)
    if not heading:
        die("usage: state.sh plan <run> prs|tickets|stories")
    m = re.search(rf"^##\s+{heading}\s*$(.*?)(?=^##\s|\Z)", text, re.M | re.S)
    print((f"## {heading}" + m.group(1)).rstrip() if m else f"(plan.md has no ## {heading} section)")


def cmd_base(repo):
    repo = os.path.realpath(repo)
    if git("-C", repo, "rev-parse", "--git-dir").returncode != 0:
        die(f"NO BASE: {repo} is not a git repo")
    remote = remote_of(repo)
    if remote:
        if git("-C", repo, "fetch", "-q", remote).returncode != 0:
            print(f"note: git fetch {remote} failed; using what is known locally", file=sys.stderr)
        base = None
        r = git("-C", repo, "symbolic-ref", "--quiet", f"refs/remotes/{remote}/HEAD")
        if r.returncode == 0:
            base = r.stdout.strip().split(f"refs/remotes/{remote}/", 1)[-1]
        if not base:
            r = run_cmd(["gh", "repo", "view", "--json", "defaultBranchRef", "-q", ".defaultBranchRef.name"], cwd=repo, timeout=60)
            base = r.stdout.strip() if r.returncode == 0 else None
        if not base:
            base = next((c for c in ("main", "master")
                         if git("-C", repo, "show-ref", "--verify", "--quiet", f"refs/remotes/{remote}/{c}").returncode == 0), None)
        if not base:
            die(f"NO BASE: cannot tell {remote}'s default branch for {repo}")
        if not on_remote(repo, remote, base):
            die(f"NO BASE: {base} is not on {remote} for {repo}")
        ref = f"refs/remotes/{remote}/{base}"
        if git("-C", repo, "show-ref", "--verify", "--quiet", ref).returncode != 0:
            ref = f"refs/heads/{base}"
    else:
        base = next((c for c in ("main", "master")
                     if git("-C", repo, "show-ref", "--verify", "--quiet", f"refs/heads/{c}").returncode == 0), None)
        if not base:
            cur = git("-C", repo, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
            if not cur or cur == "HEAD":
                die(f"NO BASE: {repo} is on a detached HEAD and has no main/master branch; check out a branch first")
            base = cur
        ref = f"refs/heads/{base}"
    commit = git("-C", repo, "rev-parse", "--verify", f"{ref}^{{commit}}").stdout.strip()
    if not commit:
        die(f"NO BASE: cannot resolve {ref} in {repo}")
    print(f"base={base} baseCommit={commit} remote={remote or 'none'}")


def record_guard(run, repo, pr_id, wt, copied=None, remote=None):
    path = os.path.join(run, ".guard.json")
    g = load_json(path)
    if not isinstance(g, dict) or g.get("runDir") != run:
        g = {"schema": 1, "runDir": run, "repos": [], "worktrees": {}}
    if repo not in g["repos"]:
        g["repos"].append(repo)
    g["worktrees"][pr_id] = wt
    if copied is not None:  # files copied in from the main checkout: never staged or committed (guard)
        old = g.setdefault("copied", {}).get(pr_id) or []
        g["copied"][pr_id] = sorted(set(old) | set(copied))
    if remote and pr_id not in (g.get("remotes") or {}):  # the push target, fixed when the worktree is first made
        g.setdefault("remotes", {})[pr_id] = remote
    atomic_write(path, json.dumps(g, indent=2) + "\n")


SLUG = re.compile(r"^(?:https?://[^/\s]+/|ssh://[^/\s]+/|[\w.-]+@[^:/\s]+:)([\w.-]+)/([\w.-]+?)(?:\.git)?/?$")


def remote_record(run, repo):
    """{name, url, repo: owner/name} of the repo's push remote now, or None (no remote)."""
    name = remote_of(repo)
    if not name:
        return None
    url = git("-C", repo, "remote", "get-url", "--push", name).stdout.strip() or git("-C", repo, "remote", "get-url", name).stdout.strip()
    m = SLUG.match(url)
    slug = f"{m.group(1)}/{m.group(2)}" if m else None
    if not slug and review_mode(run) == "github":  # e.g. a local mirror: ask gh which GitHub repo this is
        r = run_cmd(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"], cwd=repo, timeout=60)
        v = r.stdout.strip()
        slug = v if r.returncode == 0 and re.fullmatch(r"[\w.-]+/[\w.-]+", v) else None
    return {"name": name, "url": url, "repo": slug}


def guard_rec(run):
    g = load_json(os.path.join(run, ".guard.json"))
    return g if isinstance(g, dict) else {}


def repo_entry(run, repo):
    """repos.json's entry for this repo (matched by real path), or {}."""
    rj = load_json(os.path.join(run, "repos.json"))
    for r in (rj.get("repos") if isinstance(rj, dict) else None) or []:
        if isinstance(r, dict) and isinstance(r.get("path"), str) and os.path.realpath(r["path"]) == repo:
            return r
    return {}


def copy_files(repo, wt, paths):
    """Copy git-ignored files from the main checkout into a new worktree. Returns (copied, refused)."""
    copied, refused = [], []
    for rel in paths if isinstance(paths, list) else []:
        rel = str(rel).strip()
        parts = rel.replace("\\", "/").split("/")
        if not rel or os.path.isabs(rel) or ".." in parts or parts[0] == ".git":
            refused.append(f"{rel or '(empty)'}: not a plain relative path")
            continue
        src = os.path.join(repo, rel)
        if not os.path.lexists(src):
            continue  # nothing to copy (optional by design)
        # Never follow a symlink anywhere on the way (in particular out of the repo).
        d, link = repo, False
        for part in parts:
            d = os.path.join(d, part)
            link = link or os.path.islink(d)
        if link or not os.path.realpath(src).startswith(repo + os.sep):
            refused.append(f"{rel}: a symlink (not copied)")
            continue
        if not os.path.isfile(src):
            refused.append(f"{rel}: not a file")
            continue
        if git("-C", repo, "ls-files", "--error-unmatch", "--", rel).returncode == 0:
            refused.append(f"{rel}: tracked by git (only git-ignored files are copied)")
            continue
        if git("-C", repo, "check-ignore", "-q", "--", rel).returncode != 0:
            refused.append(f"{rel}: not git-ignored (only git-ignored files are copied)")
            continue
        dest = os.path.join(wt, rel)
        if os.path.lexists(dest):
            continue
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copy2(src, dest, follow_symlinks=False)
        copied.append(rel)
    return copied, refused


def detect_setup(wt):
    for f, cmd in (("package-lock.json", "npm ci"), ("pnpm-lock.yaml", "pnpm install --frozen-lockfile"),
                   ("yarn.lock", "yarn install --frozen-lockfile"), ("go.mod", "go mod download")):
        if os.path.isfile(os.path.join(wt, f)):
            return cmd
    return None


def run_setup(run, wt, pr_id, cmd):
    """Run the worktree's setup command (own process group, killed whole on timeout); never raises."""
    log = os.path.join(run, "logs", f"{pr_id}-setup.txt")
    if not cmd:
        return "none", None
    os.makedirs(os.path.dirname(log), exist_ok=True)
    timeout = int(os.environ.get("DCV2_SETUP_TIMEOUT", "") or 600)
    with open(log, "w") as out:
        out.write(f"$ {cmd}\n(in {wt})\n")
        out.flush()
        try:
            p = subprocess.Popen(["bash", "-c", cmd], cwd=wt, stdout=out, stderr=subprocess.STDOUT,
                                 stdin=subprocess.DEVNULL, start_new_session=True)
        except OSError as e:
            out.write(f"could not start: {e}\n")
            return "failed", log
        try:
            rc = p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except OSError:
                pass
            p.wait()
            out.write(f"\nsetup timed out after {timeout} s\n")
            return "failed", log
        out.write(f"\nexit code {rc}\n")
    return ("ok" if rc == 0 else "failed"), log


def cmd_worktree(run, repo, pr_id, base):
    run = os.path.realpath(run)
    run_id = os.path.basename(run)
    repo = os.path.realpath(repo)
    if not re.fullmatch(r"PR\d+", pr_id):
        die(f"pr id must be PR<n> (got {pr_id})")
    if git("-C", repo, "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}").returncode != 0:
        die(f"NOT MADE: base commit {base!r} is not a commit in {repo}; use baseCommit from status.json / state.sh base")
    branch, wt = canon(repo, run_id, pr_id)
    st = load_json(os.path.join(run, "status.json")) or {}
    rec = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pr_id), {})
    listed = f"worktree {wt}" in git("-C", repo, "worktree", "list", "--porcelain").stdout.splitlines()
    remade = made = False
    if listed and os.path.isdir(wt):
        head = git("-C", wt, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
        if head != branch:
            die(f"NEEDS-ATTENTION: {wt} is on {head}, not {branch}", 3)
    else:
        git("-C", repo, "worktree", "prune")
        os.makedirs(os.path.dirname(wt), exist_ok=True)
        if git("-C", repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}").returncode == 0:
            r = git("-C", repo, "worktree", "add", "-q", wt, branch)
            remade = True
        else:
            if (rec.get("commits") or 0) > 0:
                die(f"NEEDS-ATTENTION: branch {branch} is missing but {pr_id} recorded {rec.get('commits')} commits; not re-making it from the base", 3)
            r = git("-C", repo, "worktree", "add", "-q", "-b", branch, wt, base)
        if r.returncode != 0:
            die(f"NOT MADE: git worktree add failed: {first_line(r.stderr)}")
        made = True
    remote = remote_record(run, repo)
    if not made:
        record_guard(run, repo, pr_id, wt, remote=remote)
        print(f"branch={branch} worktree={wt}")
        return
    # A newly made worktree (fresh or re-made folder): local files, then the install step.
    entry = repo_entry(run, repo)
    copied, refused = copy_files(repo, wt, entry.get("copyFiles"))
    record_guard(run, repo, pr_id, wt, copied, remote)
    print(f"branch={branch} worktree={wt}" + (" remade=yes" if remade else ""))
    if copied or refused:
        print(f"copied={len(copied)}" + (f" refused: {'; '.join(refused)}" if refused else ""))
    setup = entry.get("setup")
    # Old runs (meta.json without reviewMode, plugin 0.2.x) get no detected install: only an explicit setup.
    meta = load_json(os.path.join(run, "meta.json"))
    new_run = isinstance(meta, dict) and "reviewMode" in meta
    cmd = setup.strip() if isinstance(setup, str) else (detect_setup(wt) if new_run else None)
    result, log = run_setup(run, wt, pr_id, cmd or None)
    print(f"setup={result} log={log or '-'}")


def cmd_ship_check(run):
    st = load_status(run)
    for pr in st.get("prs") or []:
        repo = pr.get("repoPath") or ""
        remote = remote_of(repo) if repo else None
        if not remote:
            print(f"{pr.get('id')} {pr.get('repo')}: no git remote: will not push or open a PR")
        elif on_remote(repo, remote, pr.get("base") or ""):
            print(f"{pr.get('id')} {pr.get('repo')}: remote {remote}, base {pr.get('base')} is on the remote")
        else:
            print(f"{pr.get('id')} {pr.get('repo')}: remote {remote}, base {pr.get('base')} is NOT on the remote: needs attention")


def pr_body(run, st, pr):
    titles = dict(re.findall(r"^###\s+(S\d+)\s*[:\-\u2014]+\s*(.+?)\s*$", read(os.path.join(run, "plan.md")), re.M))
    prs = {p.get("id"): p for p in st.get("prs") or []}
    chk, rev = pr.get("check") or {}, pr.get("review") or {}
    lines = ["## Stories", ""] + [f"- {s}: {titles.get(s, '')}".rstrip(": ") for s in (pr.get("stories") or {}).get("ids") or []]
    lines += ["", f"Check: `{chk.get('command')}`: {chk.get('result')}", f"Review: {rev.get('verdict')} (round {rev.get('round')})"]
    for dep in pr.get("dependsOn") or []:
        d = prs.get(dep) or {}
        lines.append(f"Depends on: {d.get('prUrl') or dep + ' (branch ' + str(d.get('branch')) + ')'}")
    lines += ["", f"Plan: Eagle-Eye run {st.get('runId')}", ""]
    return "\n".join(lines)


def cmd_ship_pr(run, pid):
    st = load_status(run)
    if review_mode(run) == "github":
        die("NOT SHIPPED: this run reviews on GitHub (meta.json reviewMode github): ship with `state.sh ready <run> <pr id>`")
    if st.get("stage") != "shipping" or not st.get("shipApproval"):
        die(f"NOT SHIPPED: shipping needs stage shipping and a ship approval (stage {st.get('stage')}, "
            f"shipApproval {'set' if st.get('shipApproval') else 'missing'})")
    pr = next((p for p in st.get("prs") or [] if p.get("id") == pid), None)
    if pr is None:
        die(f"NOT SHIPPED: no {pid} in status.json")
    if pr.get("state") in ("opened", "local-only"):
        print(f"{pid} already {pr['state']}" + (f": {pr.get('prUrl')}" if pr.get("prUrl") else ""))
        return

    def save(state, note=None, url=None):
        pr["state"], pr["note"] = state, note
        if url:
            pr["prUrl"] = url
        for m in write_status(run, st, trusted=True):
            print(m)
        print(f"{pid} {state}" + (f": {url}" if url else "") + (f" ({note})" if note else ""))

    if pr.get("state") not in ("reviewed", "pushed"):
        die(f"NOT SHIPPED: {pid} is {pr.get('state')}; only reviewed PRs ship")
    repo, wt, branch, base = pr.get("repoPath"), pr.get("worktree"), pr.get("branch"), pr.get("base")
    remote = remote_of(repo)
    if not remote:
        return save("local-only", "no git remote: nothing pushed, no PR opened")
    if pr["state"] == "reviewed":
        if not on_remote(repo, remote, base):
            return save("needs-attention", f"base {base} is not on {remote}")
        r = git("-C", wt, "push", "-u", remote, branch)
        if r.returncode != 0:
            return save("needs-attention", "push refused: " + first_line(r.stderr)[:200])
        save("pushed", "pushed; opening the PR")
    v = run_cmd(["gh", "pr", "view", branch, "--json", "url", "-q", ".url"], cwd=wt, timeout=60)
    if v.returncode == 0 and v.stdout.strip().startswith("http"):
        return save("opened", None, v.stdout.strip())
    body = os.path.join(run, "pr-bodies", f"{pid}.md")
    os.makedirs(os.path.dirname(body), exist_ok=True)
    atomic_write(body, pr_body(run, st, pr))
    title = str(pr.get("title") or pid)
    c = run_cmd(["gh", "pr", "create", "--base", base, "--head", branch, "--title", title, "--body-file", body], cwd=wt, timeout=120)
    url = next((l.strip() for l in reversed(c.stdout.splitlines()) if l.strip().startswith("http")), None)
    if c.returncode == 0 and url:
        return save("opened", None, url)
    save("pushed", "gh pr create failed: " + (first_line(c.stderr) or first_line(c.stdout))[:200])


# ---- github review mode (draft PRs + review comments) ---------------------------------------
# Every GitHub call names the repo (`gh -R <owner/name>` recorded at `state.sh worktree` time), every push
# goes to the remote URL recorded then (never a remote name the worktree could re-point), without hooks,
# and every write to an existing PR first checks that the PR is this branch's PR in that repo.

GH_URL = re.compile(r"^https://[^\s/]+/([^\s/]+)/([^\s/]+)/pull/(\d+)$")
TITLE_CAP = 256
ZWJ = "‍"


def http_line(out):
    return next((l.strip() for l in reversed((out or "").splitlines()) if l.strip().startswith("https://")), None)


def gh(args, cwd, slug, timeout=120):
    return run_cmd(["gh", *args, "-R", slug], cwd=cwd, timeout=timeout)


def gh_err(r, what):
    return f"{what} failed: " + (first_line(r.stderr) or first_line(r.stdout))[:200]


def neutral(text):
    """No @mentions and no cross-repo references in anything posted (they notify or backlink other people)."""
    text = re.sub(r"(?<![\w`@" + ZWJ + r"])@(?=[A-Za-z0-9])", "@" + ZWJ, text)
    return re.sub(r"(?<![\w/])([A-Za-z0-9][\w.-]*/[\w.-]+)#(?=\d)", r"\1#" + ZWJ, text)


def remote_sha(url, branch):
    r = git("ls-remote", "--heads", url, f"refs/heads/{branch}")
    return (r.stdout.split() or [None])[0] if r.returncode == 0 else None


def push_branch(wt, url, branch):
    """Push the branch to the recorded URL: never force, no hooks."""
    return git("-c", "core.hooksPath=/dev/null", "-C", wt, "push", url, f"refs/heads/{branch}:refs/heads/{branch}")


def scan_secrets(text):
    """memory.sh's scanner: (clean, detail). Fails closed when the scanner cannot run."""
    sh = os.path.join(PLUGIN_ROOT, "scripts", "memory.sh")
    try:
        r = subprocess.run(["bash", sh, "scan"], input=text, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as e:
        return None, f"the secret scanner did not run ({e})"
    if r.returncode == 0:
        return True, None
    if r.returncode == 1 and r.stdout.startswith("looks like a secret"):
        return False, first_line(r.stdout)
    return None, "the secret scanner did not run: " + (first_line(r.stderr) or f"exit {r.returncode}")


def capped(text, tail, cap=COMMENT_CAP):
    if len(text) <= cap:
        return text
    tail = f"\n\n… (cut at {cap} characters) {tail}"
    return text[:cap - len(tail)] + tail


def github_pr(run, pid, stages=GITHUB_STAGES):
    """Status, the PR entry and its recorded remote for a GitHub write; refuses outside github mode, before the
    plan approval, or off its canonical branch and worktree."""
    st = load_status(run)
    if review_mode(run) != "github":
        die("NOT DONE: this run's review mode is local (meta.json): nothing goes to GitHub before stop #2 (`state.sh ship-pr`)")
    if not (st.get("plan") or {}).get("approval"):
        die("NOT DONE: GitHub writes need the plan approval first (stop #1: `state.sh approve <run> plan user`)")
    if st.get("stage") not in stages:
        die(f"NOT DONE: stage {st.get('stage')}: this command runs only at stage {' / '.join(stages)}")
    pr = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pid), None)
    if pr is None:
        die(f"NOT DONE: no {pid} in status.json")
    branch, wt = canon(pr.get("repoPath") or "/", st.get("runId"), pid)
    if pr.get("branch") != branch or os.path.realpath(str(pr.get("worktree") or "")) != wt or not os.path.isdir(wt):
        die(f"NOT DONE: {pid} must be on {branch} in its worktree {wt} (made by state.sh worktree)")
    if git("-C", wt, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() != branch:
        die(f"NOT DONE: {wt} is not on {branch}")
    g = guard_rec(run)
    if (g.get("worktrees") or {}).get(pid) != wt:
        die(f"NOT DONE: {pid}'s worktree is not recorded in .guard.json (run `state.sh worktree` for it)")
    rem = (g.get("remotes") or {}).get(pid)
    if rem and not (isinstance(rem, dict) and rem.get("url")):
        rem = None
    return st, pr, rem


def save_pr(run, st, pr, msg):
    for m in write_status(run, st, trusted=True):
        print(m)
    print(msg)


def attention(run, st, pr, why, what="NOT DONE"):
    """Mark the PR needs-attention, log it, exit 3."""
    pr["state"], pr["note"] = "needs-attention", why[:300]
    append_log(run, st.get("stage") or "building", f"{pr.get('id')} needs attention: {why}")
    for m in write_status(run, st, trusted=True):
        print(m)
    die(f"{what}: {why}; {pr.get('id')} needs attention (nothing was sent)", 3)


def copied_committed(run, pr):
    """Files copied in from the main checkout (recorded in .guard.json) that the branch's commits touch."""
    files = (guard_rec(run).get("copied") or {}).get(pr.get("id")) or []
    if not files:
        return []
    r = git("-C", pr["worktree"], "diff", "--name-only", f"{pr.get('baseCommit')}..HEAD")
    changed = set(r.stdout.split("\n")) if r.returncode == 0 else set()
    return [f for f in files if f in changed]


def no_copied(run, st, pr, what):
    bad = copied_committed(run, pr)
    if bad:
        attention(run, st, pr, f"the branch commits {', '.join(bad)}, copied in from the main checkout (copyFiles); remove it from the commits", what)


def clean_text(run, st, pr, text, label, what):
    """Secret scan before anything is sent; a hit or a scanner that cannot run refuses (a hit: needs-attention)."""
    clean, why = scan_secrets(text)
    if clean is None:
        die(f"{what}: {why}")
    if not clean:
        attention(run, st, pr, f"{label} not sent to GitHub: {why}", what)


def verify_pr(pr, rem, url):
    """None when url is this branch's own PR in the recorded repo, else why not."""
    m = GH_URL.match(str(url or ""))
    slug = rem.get("repo") or ""
    if not m or f"{m.group(1)}/{m.group(2)}".lower() != slug.lower():
        return f"{url} is not a pull request of {slug or 'the recorded repo'}"
    r = gh(["pr", "view", url, "--json", "url,headRefName,headRepository,headRepositoryOwner,isCrossRepository"], pr["worktree"], slug, 60)
    if r.returncode != 0:
        return gh_err(r, "gh pr view")
    try:
        v = json.loads(r.stdout or "{}")
    except ValueError:
        return "gh pr view printed no JSON"
    owner = ((v.get("headRepositoryOwner") or {}).get("login") or "")
    name = ((v.get("headRepository") or {}).get("name") or "")
    if v.get("headRefName") != pr.get("branch") or v.get("isCrossRepository") or f"{owner}/{name}".lower() != slug.lower():
        return f"{url} is not this run's PR (head {owner}/{name}:{v.get('headRefName')}, want {slug}:{pr.get('branch')})"
    return None


def find_pr(pr, rem):
    """The branch's PR in the recorded repo: ({url, isDraft, state} or {} when none, error or None)."""
    r = gh(["pr", "list", "--head", pr["branch"], "--state", "all", "--json", "url,isDraft,state", "--limit", "10"], pr["worktree"], rem["repo"], 60)
    if r.returncode != 0:
        return None, gh_err(r, "gh pr list")
    try:
        rows = json.loads(r.stdout or "[]")
    except ValueError:
        return None, "gh pr list printed no JSON"
    rows = [x for x in rows if isinstance(x, dict) and GH_URL.match(str(x.get("url") or ""))] if isinstance(rows, list) else []
    rows = [x for x in rows if verify_pr(pr, rem, x["url"]) is None]
    if not rows:
        return {}, None
    return (next((x for x in rows if x.get("state") == "OPEN"), None) or rows[0]), None


def need_remote(rem, what):
    if not rem:
        die(f"{what}: the repo had no git remote when its worktree was made")
    if not rem.get("repo"):
        die(f"{what}: cannot tell which GitHub repo {rem.get('url')} is (no owner/name in the URL, and `gh repo view` did not say)")


def story_lines(run, pr):
    titles = dict(re.findall(r"^###\s+(S\d+)\s*[:\-—]+\s*(.+?)\s*$", read(os.path.join(run, "plan.md")), re.M))
    return [f"- {s}: {titles.get(s, '')}".rstrip(": ") for s in (pr.get("stories") or {}).get("ids") or []]


def draft_body(run, st, pr):
    lines = ["## Stories", ""] + story_lines(run, pr)
    lines += ["", f"\U0001F916 Eagle-Eye run {st.get('runId')} — review in progress. Each review round is posted as a comment; "
              "this PR is marked ready for review only after the run's ship approval.", ""]
    return "\n".join(lines)


def final_body(run, st, pr):
    lines = pr_body(run, st, pr).rstrip("\n").splitlines()
    rounds = sorted(pr.get("comments") or [], key=lambda c: c.get("round", 0))
    if rounds:
        lines += ["", "## Eagle-Eye review rounds", ""] + [f"- Round {c['round']}: {c['url']}" + (f" (fixes: {c['fixesUrl']})" if c.get("fixesUrl") else "") for c in rounds]
    return "\n".join(lines) + "\n"


def title_and_body(run, st, pr, body, what):
    """Neutralised, capped, secret-scanned title + body; the body is written to pr-bodies/<PR>.md."""
    title = capped(neutral(" ".join(str(pr.get("title") or pr["id"]).split())), "", TITLE_CAP).replace("\n", " ")
    body = capped(neutral(body), "The full text is in the run folder.")
    clean_text(run, st, pr, title + "\n" + body, "the PR title/body", what)
    f = os.path.join(run, "pr-bodies", f"{pr['id']}.md")
    os.makedirs(os.path.dirname(f), exist_ok=True)
    atomic_write(f, body)
    return title, f


def cmd_draft_pr(run, pid):
    st, pr, rem = github_pr(run, pid, ("building", "ship-approval"))
    if pr.get("prUrl"):
        print(f"draft=exists url={pr['prUrl']}")
        return
    # Normally called right after the Implementer's first round (state building); a late call still
    # opens the draft (commits + a passing check are what count) and keeps a later state as it is.
    if pr.get("state") not in ("pending", "building", "drafted", "reviewing", "fixing", "reviewed"):
        die(f"NOT DRAFTED: {pid} is {pr.get('state')}")
    result = (pr.get("check") or {}).get("result")
    if result not in ("pass", "none"):
        die(f"NOT DRAFTED: the check must pass first (check.result {result})")
    wt, branch, base = pr["worktree"], pr["branch"], pr.get("base")
    n = git("-C", wt, "rev-list", "--count", f"{pr.get('baseCommit')}..HEAD").stdout.strip()
    if not n.isdigit() or int(n) == 0:
        die(f"NOT DRAFTED: {pid} has no commits since its base")

    def failed(why):
        pr["note"] = ("draft PR not opened: " + why)[:300]
        save_pr(run, st, pr, f"draft=failed reason={why}")

    if not rem:
        print("draft=no-remote: no git remote: the reviews stay in the run folder; stop #2 marks it local-only")
        return
    if not rem.get("repo"):
        return failed(f"cannot tell which GitHub repo {rem.get('url')} is")
    no_copied(run, st, pr, "NOT DRAFTED")
    if not remote_sha(rem["url"], base):
        return failed(f"base {base} is not on {rem['url']}")
    found, err = find_pr(pr, rem)
    if err:
        return failed(err)
    if found and found.get("state") != "OPEN":
        return failed(f"the branch already has a {str(found.get('state')).lower()} PR ({found.get('url')}); not opening a second one")
    title, body = title_and_body(run, st, pr, draft_body(run, st, pr), "NOT DRAFTED") if not found else (None, None)
    r = push_branch(wt, rem["url"], branch)
    if r.returncode != 0:
        return failed("push refused: " + first_line(r.stderr)[:200])
    if found:
        url, draft, how = found["url"], bool(found.get("isDraft")), "found"
    else:
        c = gh(["pr", "create", "--draft", "--base", base, "--head", branch, "--title", title, "--body-file", body], wt, rem["repo"])
        url, draft, how = http_line(c.stdout), True, "opened"
        if c.returncode != 0 or not url or not GH_URL.match(url):
            return failed(gh_err(c, "gh pr create --draft"))
    pr.update(prUrl=url, draft=draft, note=None)
    if pr.get("state") in ("pending", "building"):
        pr["state"] = "drafted"
    pr["comments"] = pr.get("comments") or []
    append_log(run, st.get("stage") or "building", f"{pid} draft PR {how}: {url}")
    save_pr(run, st, pr, f"draft={how} url={url}")


def cmd_push(run, pid):
    st, pr, rem = github_pr(run, pid)
    if not pr.get("prUrl"):
        print("push=skipped: no draft PR (the review loop stays local)")
        return
    if pr.get("state") in ("opened", "local-only"):
        die(f"NOT PUSHED: {pid} is {pr['state']}")
    need_remote(rem, "NOT PUSHED")
    wt, branch = pr["worktree"], pr["branch"]
    head = git("-C", wt, "rev-parse", "HEAD").stdout.strip()
    if remote_sha(rem["url"], branch) == head:
        print(f"push=up-to-date head={head[:12]}")
        return
    no_copied(run, st, pr, "NOT PUSHED")
    r = push_branch(wt, rem["url"], branch)
    if r.returncode != 0:
        pr["note"] = ("push refused: " + first_line(r.stderr))[:300]
        save_pr(run, st, pr, f"push=failed reason={pr['note']}")
        return
    append_log(run, st.get("stage") or "building", f"{pid} pushed {head[:12]}")
    print(f"push=ok head={head[:12]}")


def posted_already(pr, rem, footer):
    """The URL of a comment on the PR that carries this footer (a post whose record was lost), or None."""
    r = gh(["pr", "view", pr["prUrl"], "--json", "comments"], pr["worktree"], rem["repo"], 60)
    if r.returncode != 0:
        return None
    try:
        cs = json.loads(r.stdout or "{}").get("comments") or []
    except (ValueError, AttributeError):
        return None
    hit = next((c for c in cs if isinstance(c, dict) and footer in str(c.get("body") or "")), None)
    return (hit.get("url") or pr["prUrl"]) if hit else None


def post_comment(run, st, pr, rem, text, name):
    """Write pr-comments/<name>.md and post it with gh pr comment. (url, error)."""
    f = os.path.join(run, "pr-comments", f"{name}.md")
    os.makedirs(os.path.dirname(f), exist_ok=True)
    atomic_write(f, text)
    c = gh(["pr", "comment", pr["prUrl"], "--body-file", f], pr["worktree"], rem["repo"])
    if c.returncode != 0:
        return None, gh_err(c, "gh pr comment")
    return http_line(c.stdout) or pr["prUrl"], None


def checked_pr(run, st, pr, rem, what):
    need_remote(rem, what)
    bad = verify_pr(pr, rem, pr["prUrl"])
    if bad:
        die(f"{what}: {bad}")


def cmd_post_review(run, pid, rnd):
    if not re.fullmatch(r"[1-3]", rnd):
        die("NOT POSTED: round must be 1, 2 or 3")
    rnd = int(rnd)
    st, pr, rem = github_pr(run, pid)
    if not pr.get("prUrl"):
        print("posted=no: no draft PR; the review stays in the run folder")
        return
    comments = pr.get("comments") or []
    old = next((c for c in comments if c.get("round") == rnd), None)
    if old:
        print(f"posted=already url={old.get('url')}")
        return
    rel = f"reviews/{pid}-round{rnd}.md"
    text = read(os.path.join(run, rel)).strip()
    if not text:
        die(f"NOT POSTED: {rel} is missing or empty")
    checked_pr(run, st, pr, rem, "NOT POSTED")
    footer = f"_Eagle-Eye run {st.get('runId')}, review round {rnd}._"
    head = git("-C", pr["worktree"], "rev-parse", "HEAD").stdout.strip()
    url = posted_already(pr, rem, footer)
    how = "found"
    if not url:
        body = capped(neutral(text), f"The full review is in the run folder: {rel}.") + f"\n\n{footer}\n"
        clean_text(run, st, pr, body, f"review round {rnd}", "NOT POSTED")
        url, err = post_comment(run, st, pr, rem, body, f"{pid}-round{rnd}")
        if err:
            print(f"posted=failed reason={err}")
            return
        how = "ok"
    pr["comments"] = comments + [{"round": rnd, "url": url, "head": head}]
    append_log(run, st.get("stage") or "building", f"{pid} review round {rnd} posted: {url}")
    save_pr(run, st, pr, f"posted={how} url={url}")


def cmd_comment(run, pid, kind, rnd):
    if kind != "fixes" or not re.fullmatch(r"[1-3]", rnd):
        die("usage: state.sh comment <run> <pr id> fixes <round 1-3> (the only template)")
    rnd = int(rnd)
    st, pr, rem = github_pr(run, pid)
    if not pr.get("prUrl"):
        print("posted=no: no draft PR")
        return
    entry = next((c for c in pr.get("comments") or [] if c.get("round") == rnd), None)
    if not entry or not entry.get("head"):
        die(f"NOT POSTED: round {rnd}'s review was not posted (`state.sh post-review <run> {pid} {rnd}` first)")
    if entry.get("fixesUrl"):
        print(f"posted=already url={entry['fixesUrl']}")
        return
    wt = pr["worktree"]
    log = git("-C", wt, "log", "--reverse", "--format=%h %s", f"{entry['head']}..HEAD")
    commits = [l for l in log.stdout.splitlines() if l.strip()] if log.returncode == 0 else []
    if not commits:
        die(f"NOT POSTED: no commits since round {rnd}'s review")
    checked_pr(run, st, pr, rem, "NOT POSTED")
    if remote_sha(rem["url"], pr["branch"]) != git("-C", wt, "rev-parse", "HEAD").stdout.strip():
        die("NOT POSTED: push the fixes first (`state.sh push <run> <pr id>`)")
    footer = f"_Eagle-Eye run {st.get('runId')}, round {rnd} fixes._"
    url, how = posted_already(pr, rem, footer), "found"
    if not url:
        clean, _ = scan_secrets("\n".join(commits))
        items = [f"- `{c.split(' ', 1)[0]}` {c.split(' ', 1)[1] if ' ' in c and clean else ''}".rstrip() for c in commits]
        body = capped(neutral(f"Round {rnd} fixes: {len(commits)} commit(s)\n\n" + "\n".join(items)), "See the branch for the rest.") + \
            f"\n\nReview round {rnd + 1} follows.\n\n{footer}\n"
        clean_text(run, st, pr, body, f"round {rnd} fixes comment", "NOT POSTED")
        url, err = post_comment(run, st, pr, rem, body, f"{pid}-round{rnd}-fixes")
        if err:
            print(f"posted=failed reason={err}")
            return
        how = "ok"
    entry["fixesUrl"] = url
    append_log(run, st.get("stage") or "building", f"{pid} round {rnd} fixes posted: {url}")
    save_pr(run, st, pr, f"posted={how} url={url}")


def cmd_ready(run, pid):
    st = load_status(run)
    if st.get("stage") != "shipping" or not st.get("shipApproval"):
        die(f"NOT READY: needs stage shipping and the ship approval (stage {st.get('stage')}, "
            f"shipApproval {'set' if st.get('shipApproval') else 'missing'})")
    if review_mode(run) != "github":
        die("NOT READY: this run's review mode is local: ship with `state.sh ship-pr <run> <pr id>`")
    pr = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pid), None)
    if pr is None:
        die(f"NOT READY: no {pid} in status.json")
    if pr.get("state") in ("opened", "local-only"):
        print(f"{pid} already {pr['state']}" + (f": {pr.get('prUrl')}" if pr.get("prUrl") else ""))
        return
    if pr.get("state") not in ("reviewed", "pushed"):
        die(f"NOT READY: {pid} is {pr.get('state')}; only reviewed PRs are marked ready")
    st, pr, rem = github_pr(run, pid, ("shipping",))

    def save(state, note=None, url=None):
        pr["state"], pr["note"] = state, note
        if url:
            pr["prUrl"] = url
        save_pr(run, st, pr, f"{pid} {state}" + (f": {url}" if url else "") + (f" ({note})" if note else ""))

    wt, branch, base = pr["worktree"], pr["branch"], pr.get("base")
    if not rem:
        return save("local-only", "no git remote: nothing pushed, no PR opened")
    if not rem.get("repo"):
        return save("needs-attention", f"cannot tell which GitHub repo {rem.get('url')} is")
    if not remote_sha(rem["url"], base):
        return save("needs-attention", f"base {base} is not on {rem['url']}")
    no_copied(run, st, pr, "NOT READY")
    url, draft = pr.get("prUrl"), pr.get("draft")
    if url:
        bad = verify_pr(pr, rem, url)
        if bad:
            return save("needs-attention", bad)
    else:
        found, err = find_pr(pr, rem)
        if err:
            return save("pushed" if remote_sha(rem["url"], branch) else "needs-attention", err)
        if found and found.get("state") != "OPEN":
            return save("needs-attention", f"the branch's PR is {str(found.get('state')).lower()}: {found.get('url')}")
        if found:
            url, draft = found["url"], bool(found.get("isDraft"))
    title, body = title_and_body(run, st, pr, final_body(run, st, pr), "NOT READY")
    head = git("-C", wt, "rev-parse", "HEAD").stdout.strip()
    if remote_sha(rem["url"], branch) != head:
        r = push_branch(wt, rem["url"], branch)
        if r.returncode != 0:
            return save("needs-attention", "push refused: " + first_line(r.stderr)[:200])
    if not url:
        c = gh(["pr", "create", "--base", base, "--head", branch, "--title", title, "--body-file", body], wt, rem["repo"])
        new = http_line(c.stdout)
        if c.returncode == 0 and new and GH_URL.match(new):
            pr["draft"] = False
            return save("opened", None, new)
        return save("pushed", gh_err(c, "gh pr create"))
    note = None
    e = gh(["pr", "edit", url, "--body-file", body], wt, rem["repo"])
    if e.returncode != 0:
        note = gh_err(e, "final PR body: gh pr edit")[:200]
    if draft is not False:
        r = gh(["pr", "ready", url], wt, rem["repo"])
        if r.returncode != 0:
            return save("pushed", gh_err(r, "gh pr ready"), url)
    pr["draft"] = False
    append_log(run, "shipping", f"{pid} marked ready for review: {url}")
    save("opened", note, url)


def cmd_mode(run):
    print(f"reviewMode={review_mode(run)}")


def ticket_ship_statuses(plan):
    """From plan.md's ## Tickets lines: ticket id / custom id (lowercased) -> the ship status shown at stop #2."""
    m = re.search(r"^##\s+Tickets\s*$(.*?)(?=^##\s|\Z)", plan, re.M | re.S)
    out = {}
    for line in (m.group(1) if m else "").splitlines():
        if not line.strip().startswith("-"):
            continue
        cells = [c.strip() for c in line.strip()[1:].split("|")]
        status = next((c.split(":", 1)[1].strip() for c in cells if c.lower().startswith("ship status:")), "-")
        for key in cells[:2]:
            if key and key != "-":
                out[key.lower()] = None if status in ("", "-") else status
    return out


def cmd_writeback(run, force):
    if not os.path.isfile(os.path.join(run, "tickets.json")):
        print("no tickets.json: no writeback needed")
        return
    dest = os.path.join(run, "writeback.json")
    if os.path.exists(dest) and not force:
        die("NOT WRITTEN: writeback.json exists (the app may already be using it); not overwriting. Pass --force only to replace it on purpose.")
    plan = read(os.path.join(run, "plan.md"))
    st = load_status(run)
    titles = dict(re.findall(r"^###\s+(S\d+)\s*[:\-\u2014]+\s*(.+?)\s*$", plan, re.M))
    story_refs = {}
    for m in re.finditer(r"^###\s+(S\d+)\b(.*?)(?=^###\s|^##\s|\Z)", plan, re.M | re.S):
        tl = re.search(r"^\s*-\s*Tickets:\s*(.+)$", m.group(2), re.M)
        if tl:
            story_refs[m.group(1)] = {x.strip().lower() for x in re.split(r"[,\s]+", tl.group(1)) if x.strip()}
    ship_status = ticket_ship_statuses(plan)
    snap = load_json(os.path.join(run, "tickets.json"))
    tasks = snap.get("tasks", snap) if isinstance(snap, dict) else snap
    story_pr = {}
    for pr in st.get("prs") or []:
        for sid in (pr.get("stories") or {}).get("ids") or []:
            story_pr[sid] = pr
    drafts = []
    for task in tasks if isinstance(tasks, list) else []:
        tid, cid = str(task.get("id", "")), task.get("custom_id")
        keys = {tid.lower()} | ({str(cid).lower()} if cid else set())
        stories = sorted(s for s, refs in story_refs.items() if refs & keys)
        prs = []
        for sid in stories:
            pr = story_pr.get(sid)
            if pr and not any(p["id"] == pr["id"] for p in prs):
                prs.append({"id": pr["id"], "repo": pr.get("repo"), "url": pr.get("prUrl"), "opened": pr.get("state") == "opened"})
        lines = []
        for p in prs:
            pr = next(x for x in st["prs"] if x["id"] == p["id"])
            where = pr.get("prUrl") or f"local branch `{pr.get('branch')}` (not pushed: {pr.get('note') or 'no PR opened'})"
            if pr.get("prUrl") and pr.get("state") != "opened":
                where += " (draft)"
            chk, rev = pr.get("check") or {}, pr.get("review") or {}
            lines.append(f"- {pr['id']} ({pr.get('repo')}): {where}. Check `{chk.get('command')}`: {chk.get('result')}. Review: {rev.get('verdict')} (round {rev.get('round')}).")
        done = [f"- {s}: {titles.get(s, '')}".rstrip(": ") for s in stories]
        comment = "Eagle-Eye run " + str(st.get("runId", "")) + "\n\nDone:\n" + "\n".join(done or ["- (no story covers this ticket)"]) + "\n\nPull requests:\n" + "\n".join(lines or ["- none"])
        # Opened = ready for review (a PR left as a draft has a url but does not count).
        flags = [bool(p.pop("opened") and p["url"]) for p in prs]
        opened = bool(prs) and all(flags)
        planned = next((ship_status[k] for k in sorted(keys) if ship_status.get(k)), None)
        if not prs:
            note = "No PR covers this ticket."
        elif not opened:
            note = "Status left unchanged: not every PR was opened."
        elif not planned:
            note = "Status left unchanged: the plan named no ship status for this ticket."
        else:
            note = None
        drafts.append({
            "ticket": {"id": tid, "customId": cid, "title": task.get("name"), "url": task.get("url")},
            "prs": prs,
            "comment": comment,
            "status": planned if opened else None,
            "statusNote": note,
        })
    atomic_write(dest, json.dumps({"schema": 1, "runId": st.get("runId"), "createdAt": now(), "drafts": drafts}, indent=2) + "\n")
    print(f"wrote {dest} ({len(drafts)} drafts)")


def cmd_gate(run, pid, gate, reason, review=None):
    if gate not in GATES:
        die(f"NOT WRITTEN: gate must be one of {', '.join(GATES)} (got {gate!r})")
    reason = " ".join((reason or "").split())
    if not reason:
        die("NOT WRITTEN: a gate needs a one-line reason")
    if len(reason) > 300:
        die("NOT WRITTEN: keep the reason to one line (at most 300 characters)")
    st = load_status(run)
    pr = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pid), None)
    if pr is None:
        die(f"NOT WRITTEN: no {pid} in status.json")
    rev = pr.get("review") or {}
    rnd = rev.get("round") or 0
    review = review or (f"reviews/{pid}-round{rnd}.md" if rnd else rev.get("path"))
    old = read_gate(run, pid)
    if gate in ("PASS", "CONCERNS"):
        text = read(review if review and os.path.isabs(review) else os.path.join(run, review or "-"))
        m = re.search(r"^#+\s*Verdict\b[:\s]*([^\n]*)\n*([^\n]*)", text or "", re.M | re.I)
        line = ((m.group(1).strip() or m.group(2)) if m else "")
        if not m:  # the comment-friendly first line: "Eagle-Eye review \u2014 round n: APPROVED \u2014 ..."
            m1 = re.match(r"^\s*\**Eagle-Eye review\s*[\u2014\u2013-]+\s*round\s*\d+\s*:\s*(APPROVED|REQUEST CHANGES)\b", text or "", re.I)
            line = m1.group(1) if m1 else ""
        verdict = re.sub(r"[*`_#>]", "", line).strip().upper() or None
        if not (verdict or "").startswith("APPROVED"):
            die(f"NOT WRITTEN: {gate} needs the review file {review} to exist with verdict APPROVED (found {verdict or 'no review'}); "
                "a review asking for changes is FAIL")
    if gate == "WAIVED":
        if not old or old.get("gate") not in ("CONCERNS", "FAIL"):
            die(f"NOT WRITTEN: WAIVED only replaces a CONCERNS or FAIL gate (now {(old or {}).get('gate') or 'missing'}), "
                "and only when the user said so, with their reason")
    os.makedirs(os.path.join(run, "gates"), exist_ok=True)
    body = (f"gate: {gate}\nreason: {json.dumps(reason)}\nupdated: {now()}\nreview: {json.dumps(review)}\nround: {rnd}\n"
            + (f"waived: {json.dumps(old.get('gate'))}\n" if gate == "WAIVED" else ""))
    atomic_write(os.path.join(run, "gates", f"{pid}.yml"), body)
    append_log(run, st.get("stage") or "building", f"{pid} gate {gate}" + (" (waived by the user)" if gate == "WAIVED" else "") + f": {reason}")
    print(f"wrote gates/{pid}.yml: {gate}")


def plan_pr_stories(plan):
    out = {}
    for m in re.finditer(r"^###\s+(PR\d+)\b(.*?)(?=^###\s|^##\s|\Z)", plan, re.M | re.S):
        sl = re.search(r"^\s*-\s*Stories:\s*(.+)$", m.group(2), re.M)
        out[m.group(1)] = re.findall(r"S\d+", sl.group(1)) if sl else []
    return out


def write_report(run):
    st = load_json(os.path.join(run, "status.json")) or {}
    plan = read(os.path.join(run, "plan.md"))
    log = read(os.path.join(run, "log.md")).splitlines()
    planned = plan_pr_stories(plan)
    started = log[0].split(" ", 1)[0] if log else None
    pl = st.get("plan") or {}
    pr_word = re.compile(r"\bPR\d+\b")
    plan_retries = sum(1 for l in log if "retried" in l and re.search("plan", l, re.I) and not pr_word.search(l))
    L = [f"# Run report: {st.get('runId')}", "",
         f"- Stage: **{st.get('stage')}**" + (f": {st.get('error')}" if st.get("error") else ""),
         f"- Folder: `{st.get('folder')}`",
         f"- Started: {started or '-'}; last update: {st.get('updatedAt') or '-'}"]
    ver = read(os.path.join(os.environ.get("DCV2_PLUGIN_ROOT", ""), "VERSION")).strip()
    if ver:
        L.append(f"- Roles version: {ver}")
    appr, ship = pl.get("approval") or {}, st.get("shipApproval") or {}
    L += [f"- Plan: {pl.get('reviewRound', 0)} review round(s), last verdict {pl.get('verdict') or '-'}, "
          f"{pl.get('openFindings', 0)} MEDIUM+ open; approved by {appr.get('by') or '-'}" + (f" at {appr['at']}" if appr.get("at") else ""),
          f"- Ship: approved by {ship.get('by') or '-'}; " + json.dumps(st.get("ship")) if st.get("ship") else f"- Ship: approved by {ship.get('by') or '-'}; not shipped",
          f"- Plan retries: {plan_retries}",
          ""]
    prs = st.get("prs") or []
    L += ["## Pull requests", ""]
    if not prs:
        L += ["No PRs (the plan was not approved or the run stopped before building).", ""]
    else:
        L += ["| PR | Repo | Stories planned | Built | Commits | Review rounds | Retries | Gate | Check | State |",
              "|---|---|---|---|---|---|---|---|---|---|"]
        for pr in prs:
            pid = pr.get("id")
            ids = (pr.get("stories") or {}).get("ids") or []
            want = planned.get(pid) or ids
            g = read_gate(run, pid) or {}
            chk, rev = pr.get("check") or {}, pr.get("review") or {}
            pid_word = re.compile(r"\b" + re.escape(str(pid)) + r"\b")
            retries = sum(1 for l in log if "retried" in l and pid_word.search(l))
            L.append(f"| {pid} | {pr.get('repo')} | {', '.join(want) or '-'} | {(pr.get('stories') or {}).get('done', 0)}/{(pr.get('stories') or {}).get('total', len(ids))} "
                     f"| {pr.get('commits', 0)} | {rev.get('round', 0)} | {retries} | {g.get('gate') or 'missing'} | {chk.get('result')} | {pr.get('state')} |")
        L.append("")
        for pr in prs:
            pid = pr.get("id")
            g = read_gate(run, pid) or {}
            chk, rev = pr.get("check") or {}, pr.get("review") or {}
            ids = (pr.get("stories") or {}).get("ids") or []
            miss = [x for x in planned.get(pid, []) if x not in ids]
            extra = [x for x in ids if planned.get(pid) and x not in planned[pid]]
            L += [f"### {pid}: {pr.get('title')}", "",
                  f"- Repo `{pr.get('repo')}`, branch `{pr.get('branch')}`, worktree `{pr.get('worktree')}`",
                  f"- PR: {pr.get('prUrl') or '-'}" + (f"; note: {pr.get('note')}" if pr.get("note") else ""),
                  f"- Planned vs built: planned {', '.join(planned.get(pid, [])) or '-'}; recorded {', '.join(ids) or '-'}; "
                  f"done {(pr.get('stories') or {}).get('done', 0)}/{(pr.get('stories') or {}).get('total', len(ids))}"
                  + (f"; missing from the record: {', '.join(miss)}" if miss else "") + (f"; not in the plan: {', '.join(extra)}" if extra else ""),
                  f"- Gate: {g.get('gate') or 'missing'}" + (f": {g.get('reason')}" if g.get("reason") else "") + (f" ({g.get('updated')})" if g.get("updated") else ""),
                  f"- Check: `{chk.get('command')}`: {chk.get('result')}" + (f": {chk.get('summary')}" if chk.get("summary") else "") + (f" (log `{chk.get('log')}`)" if chk.get("log") else ""),
                  f"- Review: {rev.get('verdict') or '-'} after {rev.get('round', 0)} round(s), {rev.get('openFindings', 0)} MEDIUM+ open" + (f" (`{rev.get('path')}`)" if rev.get("path") else ""),
                  ""]
    mu = read(os.path.join(run, "memory-update.md")).strip()
    L += ["## Memory update", "", "\n".join(mu.splitlines()[:15]) if mu else "No memory-update.md.", ""]
    L += ["## Events", "", f"{len(log)} log lines; retries {sum(1 for l in log if 'retried' in l)}; "
          f"needs-attention {sum(1 for p in prs if p.get('state') == 'needs-attention')}. Full timeline: `log.md`.", ""]
    atomic_write(os.path.join(run, "report.md"), "\n".join(L))


def cmd_report(run):
    if not isinstance(load_json(os.path.join(run, "status.json")), dict):
        die(f"no valid status.json in {run}")
    write_report(run)
    print(f"wrote {os.path.join(run, 'report.md')}")


def main(argv):
    cmd, args = (argv[0] if argv else ""), argv[1:]
    need = {"preflight": 1, "put": 1, "approve": 3, "plan": 2, "base": 1, "worktree": 4, "ship-check": 1, "ship-pr": 2, "writeback": 1, "testmode": 0,
            "gate": 4, "report": 1, "mode": 1, "draft-pr": 2, "push": 2, "post-review": 3, "comment": 4, "ready": 2}
    if cmd not in need or len(args) < need[cmd]:
        die("usage: see the header of state.sh", 2)
    if cmd == "preflight": cmd_preflight(args[0], "resume" in args[1:])
    elif cmd == "testmode": cmd_testmode()
    elif cmd == "put": cmd_put(args[0])
    elif cmd == "approve": cmd_approve(args[0], args[1], args[2])
    elif cmd == "plan": cmd_plan(args[0], args[1])
    elif cmd == "base": cmd_base(args[0])
    elif cmd == "worktree": cmd_worktree(args[0], args[1], args[2], args[3])
    elif cmd == "ship-check": cmd_ship_check(args[0])
    elif cmd == "ship-pr": cmd_ship_pr(args[0], args[1])
    elif cmd == "writeback": cmd_writeback(args[0], "--force" in args[1:])
    elif cmd == "gate":
        rv = args[args.index("--review") + 1] if "--review" in args[4:-1] else None
        cmd_gate(args[0], args[1], args[2], args[3], rv)
    elif cmd == "report": cmd_report(args[0])
    elif cmd == "mode": cmd_mode(args[0])
    elif cmd == "draft-pr": cmd_draft_pr(args[0], args[1])
    elif cmd == "push": cmd_push(args[0], args[1])
    elif cmd == "post-review": cmd_post_review(args[0], args[1], args[2])
    elif cmd == "comment": cmd_comment(args[0], args[1], args[2], args[3])
    elif cmd == "ready": cmd_ready(args[0], args[1])


main(sys.argv[1:])
PY

case "${1:-}" in
  now)
    date -u +%FT%TZ
    ;;
  log)
    [[ $# -eq 2 || $# -eq 3 ]] || { echo "usage: state.sh log <run dir> [<stage>] <<'EOF' ... EOF (the text on stdin, never as an argument)" >&2; exit 2; }
    run="$2"; stage="${3:-}"
    if [[ -z "$stage" ]]; then
      stage="$(python3 -c 'import json,sys
try: print(json.load(open(sys.argv[1])).get("stage") or "run")
except Exception: print("run")' "$run/status.json" 2>/dev/null || echo run)"
    fi
    [[ "$stage" =~ ^[a-z-]+$ ]] || { echo "log: stage must be a stage name" >&2; exit 2; }
    text="$(cat)"; text="${text//$'\r'/ }"; text="${text//$'\n'/ }"
    [[ -n "${text// /}" ]] || { echo "log: empty text on stdin" >&2; exit 2; }
    printf '%s %s %s\n' "$(date -u +%FT%TZ)" "$stage" "$text" >> "$run/log.md"
    ;;
  preflight)
    py_works || { echo "PREFLIGHT FAILED: python3 is missing or does not run; the run cannot validate or write its state. Install python3, then resume." >&2; exit 1; }
    python3 -c "$PY" "$@"
    ;;
  "")
    echo "usage: see the header of state.sh" >&2; exit 2
    ;;
  *)
    py_works || { echo "NOT WRITTEN: python3 is missing or does not run (state.sh needs it)" >&2; exit 1; }
    DCV2_PLUGIN_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" python3 -c "$PY" "$@"
    ;;
esac
