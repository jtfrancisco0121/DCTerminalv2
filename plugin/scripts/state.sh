#!/usr/bin/env bash
# Run-folder state for the eagle-eye orchestrator. Every write is validated and
# atomic (a mktemp file in the same folder, then mv). Needs python3 (preflight checks it).
#
#   state.sh preflight <run> [resume]         check python3, git, DCV2_RUN_DIR and request.md;
#                                             with resume, also log "resumed at stage <stage>";
#                                             print folder= / repos= / tickets= / reposJson= / testmode=
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
#   state.sh ship-check <run>                 per PR: remote, and whether its base is on the remote
#   state.sh ship-pr <run> <pr id>            push the PR's branch and open its GitHub PR (stage
#                                             shipping + ship approval only); updates status.json
#   state.sh writeback <run> [--force]        write writeback.json from tickets.json, plan.md and
#                                             status.json (refuses to overwrite it without --force)
#
# Exit 1 = refused (the message says why; nothing was written). Exit 3 = needs attention.
set -euo pipefail

py_works() { command -v python3 >/dev/null 2>&1 && python3 -c 'import json, sys; sys.exit(0)' >/dev/null 2>&1; }

IFS= read -r -d '' PY <<'PY' || true
import datetime, json, os, re, subprocess, sys, tempfile

STAGES = ["planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"]
STATES = ["pending", "building", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"]
CHECK_RESULTS = ["not-run", "pass", "fail", "none", "error"]
POST_APPROVAL = ("building", "ship-approval", "shipping", "done")
SHIPPED = ("opened", "pushed", "local-only")
ISO = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
AGENT_FOR_STATE = {"building": "implementer", "fixing": "implementer", "reviewing": "pr-reviewer"}


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


def prepare_status(doc, old, trusted):
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
        if not trusted and o.get("prUrl") and pr.get("prUrl") != o.get("prUrl"):
            bad.append(f"{pr.get('id')}.prUrl is {o['prUrl']} (written by state.sh ship-pr); keep it")
        state = pr.get("state")
        pr["agent"] = AGENT_FOR_STATE.get(state)
        pr["startedAt"] = o.get("startedAt") or (t if state not in (None, "pending") else None)
        if isinstance(pr.get("check"), dict):
            pr["check"].setdefault("cwd", pr.get("worktree"))
            pr["check"].setdefault("log", None)
        same = lambda d: {k: v for k, v in d.items() if k not in ("updatedAt", "startedAt", "agent")}
        pr["updatedAt"] = o["updatedAt"] if o.get("updatedAt") and same(o) == same(pr) else t
    return notes, bad


def gates(doc, run):
    """Refusals and reminders that depend on the run, not on the document's shape."""
    tm = testmode()
    out = []
    stage = doc.get("stage")
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
        early = [p.get("id") for p in doc.get("prs") or [] if isinstance(p, dict) and p.get("state") in ("reviewed", "building", "reviewing", "fixing")]
        if early:
            return f"NOT WRITTEN: stage done but {', '.join(early)} not shipped or finished; run `state.sh ship-pr <run> <pr id>` for reviewed PRs first.", out
    if doc.get("waitingFor") and tm["auto"]:
        what = "plan" if doc["waitingFor"] == "plan-approval" else "ship"
        out.append(f"AUTO-APPROVE (test-mode): do not wait for the user. Run `state.sh approve <run> {what} test` and continue past {doc['waitingFor']} now.")
    return None, out


def write_status(run, doc, trusted=False):
    dest = os.path.join(run, "status.json")
    old = load_json(dest)
    notes, bad = prepare_status(doc, old, trusted)
    problems = bad + validate_status(doc, run)
    if problems:
        die(f"NOT WRITTEN: {dest}: " + "; ".join(problems))
    refusal, out = gates(doc, run)
    if refusal:
        die(refusal)
    atomic_write(dest, json.dumps(doc, indent=2) + "\n")
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


def record_guard(run, repo, pr_id, wt):
    path = os.path.join(run, ".guard.json")
    g = load_json(path)
    if not isinstance(g, dict) or g.get("runDir") != run:
        g = {"schema": 1, "runDir": run, "repos": [], "worktrees": {}}
    if repo not in g["repos"]:
        g["repos"].append(repo)
    g["worktrees"][pr_id] = wt
    atomic_write(path, json.dumps(g, indent=2) + "\n")


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
    remade = False
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
    record_guard(run, repo, pr_id, wt)
    print(f"branch={branch} worktree={wt}" + (" remade=yes" if remade else ""))


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
                prs.append({"id": pr["id"], "repo": pr.get("repo"), "url": pr.get("prUrl")})
        lines = []
        for p in prs:
            pr = next(x for x in st["prs"] if x["id"] == p["id"])
            where = pr.get("prUrl") or f"local branch `{pr.get('branch')}` (not pushed: {pr.get('note') or 'no PR opened'})"
            chk, rev = pr.get("check") or {}, pr.get("review") or {}
            lines.append(f"- {pr['id']} ({pr.get('repo')}): {where}. Check `{chk.get('command')}`: {chk.get('result')}. Review: {rev.get('verdict')} (round {rev.get('round')}).")
        done = [f"- {s}: {titles.get(s, '')}".rstrip(": ") for s in stories]
        comment = "Eagle-Eye run " + str(st.get("runId", "")) + "\n\nDone:\n" + "\n".join(done or ["- (no story covers this ticket)"]) + "\n\nPull requests:\n" + "\n".join(lines or ["- none"])
        opened = bool(prs) and all(p["url"] for p in prs)
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


def main(argv):
    cmd, args = (argv[0] if argv else ""), argv[1:]
    need = {"preflight": 1, "put": 1, "approve": 3, "plan": 2, "base": 1, "worktree": 4, "ship-check": 1, "ship-pr": 2, "writeback": 1, "testmode": 0}
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
    python3 -c "$PY" "$@"
    ;;
esac
