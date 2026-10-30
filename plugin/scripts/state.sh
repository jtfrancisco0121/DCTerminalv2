#!/usr/bin/env bash
# Run-folder state for the eagle-eye orchestrator. Every write is validated and
# atomic (a mktemp file in the same folder, then mv). Needs python3 (preflight checks it).
#
#   state.sh preflight <run> [resume]         check python3, git, DCV2_RUN_DIR and request.md;
#                                             with resume, also log "resumed at stage <stage>";
#                                             print folder= / repos= / tickets= / reposJson= / testmode= /
#                                             maxImplementers= / memory= / reviewMode= / bmad= / node=
#   state.sh testmode                         print the test mode, from the env DCV2_TEST_MODE only
#   state.sh put <run>/status.json  < json    validate the whole document, then write it (only $DCV2_RUN_DIR/status.json)
#   state.sh log <run> [<stage>]    < text    append "<UTC time> <stage> <text>" to log.md (text on stdin;
#                                             stage defaults to status.json's)
#   state.sh now                              print the UTC time (ISO-8601)
#   state.sh approve <run> plan|ship user|test
#                                             record the user's yes at stop #1 / #2 (test: only when
#                                             DCV2_TEST_MODE has auto-approve); ship also sets stage shipping
#   state.sh plan <run> prs|tickets|stories   print only that part of plan.md; prs prints one line per PR:
#                                             PR<n> | <repo> | epic <id> <title> | <stories> | depends <ids|-> | stacked on <id|-> | check <full command>
#                                             ("stacked on" is the PR's one same-repo Depends on; the check is never cut)
#   state.sh base <repo path>                 print "base=<b> baseCommit=<sha> remote=<name|none>"
#   state.sh worktree <run> <repo path> <pr id> <base commit|PR<n>>
#                                             make (or reuse / re-make) the PR's worktree beside the
#                                             repo, record it in <run>/.guard.json, print
#                                             "branch=<b> worktree=<path>". Never touches the main checkout.
#                                             A PR with a same-repo Depends on is stacked: its base is that parent
#                                             PR<n> (a reviewed PR of the same repo), never a commit; the worktree is made
#                                             from the parent's head and "base=<parent branch> baseCommit=<sha> stackedOn=PR<n>"
#                                             is printed (record them in status.json; .guard.json gets stacks and bases)
#                                             A newly made worktree also gets repos.json's copyFiles
#                                             (git-ignored files only) and its setup (or the lockfile's
#                                             install), printing "setup=ok|failed|none log=<path>"
#   state.sh restack <run> <pr id>            a stacked PR after its parent changed: merge the parent's head into its worktree
#                                             (git merge, never a rebase or a force), move baseCommit and .guard.json bases, set the PR
#                                             pending with no check result, review round 0 and restack + 1 (the old gate no longer
#                                             counts). A conflict: merge aborted, nothing moves, the PR is needs-attention (exit 3)
#   state.sh mode <run>                       print "reviewMode=github|local" (meta.json; missing = local)
#   state.sh bmad-size <run> Small|Medium|Large [--docs prd,ux] [--by planner|plan-reviewer]
#                                             BMAD mode (meta.json bmad auto|on): record the sizing in status.json
#                                             "bmad" (reasons on stdin). Planning stage only; the size only goes up
#   state.sh bmad-init <run>                  make <run>/bmad/_bmad/config.toml and output/ (BMAD applied; Node 18+)
#   state.sh bmad-check <run>                 check the BMAD documents; prints "bmad-check=pass|fail <reasons>"
#                                             (exit 0 pass, 3 fail). plan-approval is refused until it passes on
#                                             the current documents (put and approve)
#   state.sh ship-check <run>                 per PR: remote, and whether its base is on the remote
#   state.sh ship-pr <run> <pr id>            local mode: push the PR's branch and open its GitHub PR, with the
#                                             github-mode protections (canonical worktree, no copied file, push to the
#                                             recorded URL without hooks, gh -R <owner/name>); a recorded remote the run
#                                             may not push (not an owner/name https/ssh URL) ends needs-attention.
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
#   state.sh pr-body <run> <pr id> [draft]    print the PR body ship-pr / ready would write (draft: the draft PR's body); read-only,
#                                             sections: Why, What it adds, Scope, Review, Verification, Tickets (a section with no data is left out)
#   state.sh writeback <run> [--force]        write writeback.json from tickets.json, plan.md and
#                                             status.json (refuses to overwrite it without --force)
#   state.sh gate <run> <pr id> PASS|CONCERNS|FAIL|WAIVED "<reason>" [--review <file>]
#                                             write gates/<pr id>.yml (A1). PASS/CONCERNS need the review
#                                             file (default reviews/<pr>-round<review.round>.md) to say
#                                             APPROVED; WAIVED only over a CONCERNS/FAIL gate, with the
#                                             user's reason
#   state.sh merge-check <run>                before stop #2: for each repo with 2 or more reviewed PRs, run the repo's check
#                                             (repos.json only, never status.json's) once on a scratch merge of their branches
#                                             (<run>/merge-check/, removed after); records status.json "mergeChecks" and prints
#                                             "merge-check <repo>: pass|fail|conflict|error|none (PR1+PR2) log=<path>", or
#                                             "merge-check: none". Stops within DCV2_MERGE_CHECK_TIMEOUT s (default 540); never pushes
#   state.sh report <run>                     write report.md from status.json, plan.md, log.md and the
#                                             gates (R10); put does it itself at stage done / failed
#   state.sh cleanup <run>                    stage done / failed only: remove the run's clean worktrees (no --force),
#                                             `git branch -d` (never -D), and delete the remote dcv2/ branch only of a
#                                             merged PR whose remote tip is the PR's head (recorded remote URL, no force)
#   state.sh request <run> recheck|cleanup|tickets|selftest
#                                             ask the app for one of its fixed actions (requests/<id>.json); prints request=<id>
#   state.sh request-wait <run> <id> [--timeout <s>]
#                                             wait (default 90 s, at most 600) for the app's requests/<id>.result.json;
#                                             prints "result=<state> ok=yes|no message=<one line>" (exit 3: still pending)
#
# Limits put enforces: at most DCV2_MAX_IMPLEMENTERS (default 3) PRs building/fixing at once (R6);
# a PR becomes reviewed only with a PASS, CONCERNS or WAIVED gate file (A1).
#
# Exit 1 = refused (the message says why; nothing was written). Exit 3 = needs attention.
set -euo pipefail

py_works() { command -v python3 >/dev/null 2>&1 && python3 -c 'import json, sys; sys.exit(0)' >/dev/null 2>&1; }

IFS= read -r -d '' PY <<'PY' || true
import contextlib, datetime, fcntl, fnmatch, glob, io, json, os, re, shutil, signal, subprocess, sys, tempfile, time

STAGES = ["planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"]
STATES = ["pending", "building", "drafted", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"]
COMMENT_CAP = 60000
GITHUB_STAGES = ("building", "ship-approval", "shipping")
PLUGIN_ROOT = os.environ.get("DCV2_PLUGIN_ROOT", "")
CHECK_RESULTS = ["not-run", "pass", "fail", "none", "error"]
POST_APPROVAL = ("building", "ship-approval", "shipping", "done")
SHIPPED = ("opened", "pushed", "local-only")
# Run files only the app writes (docs/run-folder.md); `put` refuses them.
APP_FILES = ("tickets.json", "meta.json", "repos.json", "writeback-posted.json", "tickets-pulls.json", "tickets-update.md", "app-checks.json", "app-usage.json",
             "app-cleanup.json", "app-github.json")
REQUEST_KINDS = ("recheck", "cleanup", "tickets", "selftest")
REQUEST_ID = re.compile(r"^\d{8}T\d{6}Z(?:-\d+)?-(" + "|".join(REQUEST_KINDS) + r")$")
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


def norm(p):
    """A path's identity for every allow / deny decision: the real path, casefolded (the default macOS volume ignores case)."""
    return os.path.realpath(p).casefold() if p else ""


def same_path(a, b):
    return bool(a) and bool(b) and norm(a) == norm(b)


def under(p, root):
    if not p or not root:
        return False
    p, root = norm(p), norm(root)
    return p == root or p.startswith(root.rstrip(os.sep) + os.sep)


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


DEDUPE_SECONDS = 30


def last_log_text(run, stage=None):
    """The text of log.md's last line, whitespace collapsed, when it has this stage and was written in the last
    DEDUPE_SECONDS (else None)."""
    try:
        with open(os.path.join(run, "log.md"), "rb") as f:
            f.seek(0, os.SEEK_END)
            f.seek(max(0, f.tell() - 4096))
            lines = f.read().decode("utf-8", "replace").splitlines()
    except OSError:
        return None
    parts = lines[-1].split(" ", 2) if lines else []
    if len(parts) != 3 or (stage is not None and parts[1] != stage):
        return None
    try:
        at = datetime.datetime.strptime(parts[0], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc)
    except ValueError:
        return None
    if (datetime.datetime.now(datetime.timezone.utc) - at).total_seconds() > DEDUPE_SECONDS:
        return None
    return " ".join(parts[2].split())


def append_log(run, stage, text):
    text = " ".join(text.split())
    # The same event logged twice in a row within seconds (state.sh logged it, then the orchestrator repeated it).
    # Never a retry line: each retry counts in report.md.
    if "retried" not in text and text == last_log_text(run, stage):
        return
    with open(os.path.join(run, "log.md"), "a") as f:
        f.write(f"{now()} {stage} {text}\n")


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


BMAD_MODES = ("auto", "on", "off")
BMAD_SIZES = ("Small", "Medium", "Large")
BMAD_DOCS = ("prd", "ux")


def bmad_mode(run):
    """meta.json's bmad.mode (written by the app at Start). Missing or unknown = off (old runs keep their flow)."""
    m = load_json(os.path.join(run, "meta.json"))
    b = m.get("bmad") if isinstance(m, dict) else None
    mode = b.get("mode") if isinstance(b, dict) else None
    return mode if mode in BMAD_MODES else "off"


def bmad_obj(doc):
    b = doc.get("bmad") if isinstance(doc, dict) else None
    return b if isinstance(b, dict) else {}


def bmad_applied(run, doc=None):
    """on: always. auto: status.json bmad.applied (set by bmad-size, never cleared). off: never."""
    mode = bmad_mode(run)
    if mode == "on":
        return True
    if mode == "off":
        return False
    if not isinstance(doc, dict):
        doc = load_json(os.path.join(run, "status.json"))
    return bmad_obj(doc).get("applied") is True


def bmad_find(run):
    """BMAD documents under <run>/bmad/output/<initiative>/: kind -> sorted absolute paths."""
    found = {"spec": [], "architecture": [], "prd": [], "ux": []}
    base = os.path.join(run, "bmad", "output")
    if not os.path.isdir(base):
        return found
    for init in sorted(os.listdir(base)):
        ip = os.path.join(base, init)
        if not os.path.isdir(ip) or os.path.islink(ip):
            continue
        for d in sorted(os.listdir(ip)):
            dp = os.path.join(ip, d)
            if not os.path.isdir(dp) or os.path.islink(dp):
                continue
            for kind in ("spec", "architecture", "prd"):
                if d.startswith(kind + "-") and os.path.isfile(os.path.join(dp, d + ".md")):
                    found[kind].append(os.path.join(dp, d + ".md"))
            if d.startswith("ux-"):
                for name in ("DESIGN.md", "EXPERIENCE.md"):
                    if os.path.isfile(os.path.join(dp, name)):
                        found["ux"].append(os.path.join(dp, name))
    return found


def bmad_digest(run, found):
    """sha-256 over every document found (path relative to the run, then bytes), in path order."""
    import hashlib
    h = hashlib.sha256()
    for f in sorted(f for fs in found.values() for f in fs):
        h.update(os.path.relpath(f, run).encode() + b"\0")
        try:
            with open(f, "rb") as fh:
                h.update(fh.read())
        except OSError:
            h.update(b"?")
        h.update(b"\0")
    return h.hexdigest()


def bmad_frontmatter(text):
    m = re.match(r"\A---\r?\n(.*?)\r?\n---[ \t]*(?:\r?\n|\Z)", text, re.S)
    d = {}
    for line in (m.group(1).splitlines() if m else []):
        mm = re.match(r"^([A-Za-z_][\w-]*):\s*(.*?)\s*$", line)
        if mm:
            d[mm.group(1)] = re.sub(r"\s+#.*$", "", mm.group(2)).strip().strip("'\"")
    return d


BRACE_TOKEN = re.compile(r"\{[A-Za-z][^{}\n]*\}")
_TEMPLATE_TOKENS = []


def bmad_text_outside_code(text):
    t = re.sub(r"(```|~~~).*?\1", "", text, flags=re.S)
    t = re.sub(r"<!--.*?-->", "", t, flags=re.S)
    return re.sub(r"`[^`\n]*`", "", t)


def bmad_template_tokens():
    """The {Word…} tokens the vendored templates (plugin/bmad/skills/*/assets/*template*.md) contain, collected at check
    time: only these count as unfilled, so prose such as a REST path with a brace-wrapped parameter passes."""
    if not _TEMPLATE_TOKENS:
        tokens = set()
        for path in glob.glob(os.path.join(PLUGIN_ROOT, "bmad", "skills", "*", "assets", "*template*.md")) if PLUGIN_ROOT else []:
            tokens.update(BRACE_TOKEN.findall(read(path)))  # the whole template: its tokens sit in prose, comments and examples
        _TEMPLATE_TOKENS.append(tokens)
    return _TEMPLATE_TOKENS[0]


def bmad_placeholders(text):
    """Unfilled template leftovers outside code: tokens taken from the vendored templates, any double-brace token, and
    template guide comments."""
    guide = bool(re.search(r"<!--\s*TEMPLATE GUIDE", re.sub(r"(```|~~~).*?\1", "", text, flags=re.S)))
    t = bmad_text_outside_code(text)
    known = bmad_template_tokens()
    hits = [h for h in BRACE_TOKEN.findall(t) if h in known]
    hits += re.findall(r"\{\{[^{}\n]*\}\}", t)
    return (["<!-- TEMPLATE GUIDE -->"] if guide else []) + hits[:3]


def toml_str(v):
    out = []
    for ch in str(v):
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ord(ch) < 0x20 or ord(ch) == 0x7f:
            out.append("\\u%04x" % ord(ch))
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def node_major():
    r = run_cmd(["node", "--version"], timeout=20)
    m = re.match(r"v?(\d+)\.", (r.stdout or "").strip()) if r.returncode == 0 else None
    return (int(m.group(1)), (r.stdout or "").strip().lstrip("v")) if m else (None, None)


def remote_of(repo):
    remotes = git("-C", repo, "remote").stdout.split()
    return "origin" if "origin" in remotes else (remotes[0] if remotes else None)


def on_remote(repo, remote, branch):
    return bool(branch) and git("-C", repo, "ls-remote", "--exit-code", "--heads", remote, branch).returncode == 0


# ---- status.json -------------------------------------------------------------------------

RISK_VERDICTS = ("NO BLOCKING RISKS", "RISKS FOUND")


def check_risk(r, p):
    """plan.risk: null, or the last Risk Reviewer round: {round, verdict, openFindings, path}."""
    if r is None:
        return
    if not isinstance(r, dict):
        p.append("plan.risk must be null or {round, verdict, openFindings, path}")
        return
    if not (isinstance(r.get("round"), int) and not isinstance(r.get("round"), bool) and r["round"] >= 1):
        p.append("plan.risk.round must be an integer >= 1")
    if r.get("verdict") not in RISK_VERDICTS:
        p.append(f"plan.risk.verdict must be one of {RISK_VERDICTS}")
    n = r.get("openFindings")
    if not (isinstance(n, int) and not isinstance(n, bool) and n >= 0):
        p.append("plan.risk.openFindings must be an integer >= 0 (the MEDIUM+ risks)")
    elif r.get("verdict") == "NO BLOCKING RISKS" and n:
        p.append("plan.risk: verdict NO BLOCKING RISKS needs openFindings 0")
    path = r.get("path")
    if path is not None and not (isinstance(path, str) and re.fullmatch(r"reviews/risk-round\d+\.md", path)):
        p.append("plan.risk.path must be null or reviews/risk-round<n>.md (relative to the run)")


def check_approval(v, name, problems):
    if v is None:
        return
    if not (isinstance(v, dict) and v.get("by") in ("user", "test") and ISO.match(str(v.get("at")))):
        problems.append(f"{name} must be null or {{by: user|test, at: <UTC>}}")


def size_shape_ok(z):
    ints = ("lines", "added", "deleted", "files", "excluded", "limit")
    return (isinstance(z, dict) and all(type(z.get(k)) is int and z[k] >= 0 for k in ints) and isinstance(z.get("over"), bool)
            and (z.get("reason") is None or isinstance(z.get("reason"), str)) and isinstance(z.get("overlaps"), list)
            and isinstance(z.get("head"), str) and isinstance(z.get("at"), str))


def same_repo_ids(prs, pr):
    """The dependsOn entries of this prs[] entry that are PRs of the same repo (its stack parent; at most one is valid)."""
    repo = os.path.realpath(str(pr.get("repoPath") or "/"))
    by = {q.get("id"): q for q in prs if isinstance(q, dict)}
    return [d for d in pr.get("dependsOn") or [] if d in by and d != pr.get("id") and os.path.realpath(str(by[d].get("repoPath") or "/")) == repo]


def validate_status(doc, run, guard=None):
    p = []
    schema = doc.get("schema")
    if not (type(schema) is int and schema == 1):  # the integer 1 only: not 1.0, true or "1"
        p.append("schema must be the integer 1")
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
    mc = doc.get("mergeChecks")
    if mc is not None and not (isinstance(mc, dict) and all(
            isinstance(k, str) and isinstance(v, dict) and isinstance(v.get("prs"), list) and all(isinstance(i, str) for i in v["prs"])
            and v.get("result") in MERGE_RESULTS and (v.get("error") is None or isinstance(v.get("error"), str))
            and (v.get("log") is None or isinstance(v.get("log"), str)) and isinstance(v.get("at"), str) for k, v in mc.items())):
        p.append(f"mergeChecks must be {{repo: {{prs: [PR ids], result: one of {list(MERGE_RESULTS)}, error, log, at}}}} (written only by state.sh merge-check)")
    ol, pf = (plan or {}).get("overLimit"), (plan or {}).get("prFiles")
    if ol is not None and not (isinstance(ol, dict) and all(isinstance(k, str) and isinstance(v, str) for k, v in ol.items())):
        p.append("plan.overLimit must be an object {PR id: reason} (written only by state.sh approve)")
    if pf is not None and not (isinstance(pf, dict) and all(isinstance(k, str) and isinstance(v, list) and all(isinstance(x, str) for x in v) for k, v in pf.items())):
        p.append("plan.prFiles must be an object {PR id: [path, ...]} (written only by state.sh approve)")
    check_approval((plan or {}).get("approval"), "plan.approval", p)
    check_risk((plan or {}).get("risk"), p)
    check_approval(doc.get("shipApproval"), "shipApproval", p)
    if doc.get("bmad") is not None and not isinstance(doc.get("bmad"), dict):
        p.append("bmad must be an object (written only by state.sh bmad-size / bmad-init / bmad-check)")
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
                                                                  and isinstance(c.get("url"), str)
                                                                  and (c.get("restack") is None or (type(c.get("restack")) is int and c["restack"] >= 0))
                                                                  for c in cm)):
            p.append(f"{pid}: comments must be a list of {{round: <n>, url: <comment url>, restack: <n>}}")
        if "restack" in pr and not (type(pr.get("restack")) is int and pr["restack"] >= 0):
            p.append(f"{pid}: restack must be an int >= 0 (written only by state.sh restack)")
        so = pr.get("stackedOn")
        if so is not None and not re.fullmatch(r"PR\d+", str(so)):
            p.append(f"{pid}: stackedOn must be null or the PR id this PR is stacked on (as printed by state.sh worktree)")
        elif stage in POST_APPROVAL:
            sd = same_repo_ids(prs, pr)
            g = guard if isinstance(guard, dict) else guard_rec(run)
            if len(sd) > 1:
                p.append(f"{pid}: dependsOn has more than one PR of the same repo ({', '.join(sd)}): a stack has one parent")
            elif pr.get("worktree") and sd and so != sd[0]:
                p.append(f"{pid}: depends on {sd[0]} in the same repo, so it is stacked on it: stackedOn must be {sd[0]} "
                         f"(`state.sh worktree <run> <repo> {pid} {sd[0]}` makes the worktree and prints it)")
            elif so and sd and so != sd[0]:
                p.append(f"{pid}: stackedOn {so} is not its same-repo dependsOn entry {sd[0]}")
            elif so and not sd:
                p.append(f"{pid}: stackedOn {so} must be one of its dependsOn entries, a PR of the same repo")
            if so and so in sd and (g.get("worktrees") or {}).get(pid):
                par = next(q for q in prs if isinstance(q, dict) and q.get("id") == so)
                if (g.get("stacks") or {}).get(pid) != so:
                    p.append(f"{pid}: stackedOn {so} disagrees with .guard.json stacks ({(g.get('stacks') or {}).get(pid)!r})")
                if pr.get("base") != canon(par.get("repoPath") or "/", run_id, so)[0]:
                    p.append(f"{pid}: base must be the parent's branch {canon(par.get('repoPath') or '/', run_id, so)[0]!r} (as printed by state.sh worktree)")
                gb = (g.get("bases") or {}).get(pid)
                if gb and pr.get("baseCommit") != gb:
                    p.append(f"{pid}: baseCommit {str(pr.get('baseCommit'))[:12]} disagrees with .guard.json bases {gb[:12]}: "
                             f"re-run `state.sh restack <run> {pid}` (it writes both)")
        if pr.get("size") is not None and not size_shape_ok(pr.get("size")):
            p.append(f"{pid}: size must be {{lines, added, deleted, files, excluded, limit: ints, over: bool, reason: string|null, overlaps: list, head, at}} "
                     "(0.7.0 runs only: read, never written)")
        ep = pr.get("epic")
        if ep is not None and not (isinstance(ep, dict) and isinstance(ep.get("id"), str) and ep["id"]
                                   and (ep.get("title") is None or isinstance(ep.get("title"), str))):
            p.append(f"{pid}: epic must be null or {{id, title}} (set by state.sh from plan.md's Epic lines)")
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
            for k in ("overLimit", "prFiles"):  # frozen by `state.sh approve <run> plan` only
                if k in doc["plan"] and doc["plan"][k] != oplan.get(k):
                    notes.append(f"note: plan.{k} is set only by `state.sh approve`; kept the recorded value")
                if k in oplan:
                    doc["plan"][k] = oplan[k]
                else:
                    doc["plan"].pop(k, None)
        if doc.get("shipApproval") and doc.get("shipApproval") != old.get("shipApproval"):
            notes.append("note: the ship approval is set only by `state.sh approve`; kept the recorded value")
        doc["shipApproval"] = old.get("shipApproval")
        if doc.get("bmad") is not None and doc.get("bmad") != old.get("bmad"):
            notes.append("note: status.json bmad is set only by `state.sh bmad-size|bmad-init|bmad-check`; kept the recorded value")
        if old.get("bmad") is not None:
            doc["bmad"] = old["bmad"]
        else:
            doc.pop("bmad", None)
    if isinstance(doc.get("plan"), dict):
        doc["plan"]["approved"] = bool(doc["plan"].get("approval"))
    if not trusted:  # mergeChecks is written only by `state.sh merge-check`
        if doc.get("mergeChecks") is not None and doc.get("mergeChecks") != old.get("mergeChecks"):
            notes.append("note: mergeChecks is set only by `state.sh merge-check`; kept the recorded value")
        if old.get("mergeChecks") is not None:
            doc["mergeChecks"] = old["mergeChecks"]
        else:
            doc.pop("mergeChecks", None)
    doc.setdefault("shipApproval", None)
    doc.setdefault("paused", None)
    if isinstance(doc.get("paused"), dict) and not doc["paused"].get("at"):
        doc["paused"]["at"] = t
    oprs = {p.get("id"): p for p in old.get("prs") or [] if isinstance(p, dict)}
    bad = []
    pepics = plan_epics(run_dir) if run_dir else {}
    for pr in doc.get("prs") or []:
        if not isinstance(pr, dict):
            continue
        o = oprs.get(pr.get("id")) or {}
        if run_dir:  # the PR's epic comes from the plan's Epic lines (null: no epic), never from the document
            pr["epic"] = pepics.get(pr.get("id"))
        if not trusted and o.get("state") in SHIPPED and pr.get("state") != o.get("state"):
            bad.append(f"{pr.get('id')} is {o['state']} (written by state.sh ship-pr); re-read status.json and keep it")
        if not trusted:  # prUrl is set only by state.sh (ship-pr / draft-pr / ready): a put never changes it
            if pr.get("prUrl") not in (None, o.get("prUrl")):
                notes.append(f"note: {pr.get('id')}.prUrl is set only by state.sh; kept the recorded value ({o.get('prUrl')})")
            pr["prUrl"] = o.get("prUrl")
        if not trusted:  # restack is written only by `state.sh restack`
            if "restack" in pr and pr["restack"] != o.get("restack"):
                notes.append(f"note: {pr.get('id')}.restack is set only by `state.sh restack`; kept the recorded value")
            if "restack" in o:
                pr["restack"] = o["restack"]
            else:
                pr.pop("restack", None)
        if not trusted:  # draft and comments are written only by state.sh's GitHub commands
            for k in ("draft", "comments"):
                if k in o:
                    pr[k] = o[k]
                else:
                    pr.pop(k, None)
        if not trusted:  # size was written only by state.sh 0.7.0 (runs of that version only): kept, never written now
            if "size" in pr and pr["size"] != o.get("size"):
                notes.append(f"note: {pr.get('id')}.size is not written any more (0.7.0 runs only); kept the recorded value")
            if "size" in o:
                pr["size"] = o["size"]
            else:
                pr.pop("size", None)
        state = pr.get("state")
        if state == "reviewed" and o.get("state") != "reviewed" and run_dir:
            g = read_gate(run_dir, pr.get("id"))
            rnd = (pr.get("review") or {}).get("round") if isinstance(pr.get("review"), dict) else None
            rs = pr.get("restack") if type(pr.get("restack")) is int else 0
            if not g or g.get("gate") == "FAIL" or g.get("round") != rnd or (g.get("restack") or 0) != rs:
                why = (f"gate now {(g or {}).get('gate') or 'missing'}" if not g or g.get("gate") == "FAIL"
                       else f"the gate is for review round {g.get('round')}, the PR is at round {rnd}" if g.get("round") != rnd
                       else f"the gate is from before restack {rs} (restack voids the old gate): review again, then write the gate")
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


ID_RE = re.compile(r"\b(?:(?:CAP|AD|FR|NFR|UJ)-\d+|SM-C?\d+)\b")
NONE_RE = re.compile(r"(?i)^none\s*(?::|\s-)\s*(\S.*)$")


def bmad_plan_trace(run):
    """Problems that keep plan.md from tracing to the run's BMAD documents (empty list = traces)."""
    plan = read(os.path.join(run, "plan.md"))
    if not plan:
        return ["no plan.md"]
    found = bmad_find(run)
    known = set()
    for fs in found.values():
        for f in fs:
            known.update(ID_RE.findall(read(f) or ""))
    spec = [read(f) or "" for f in found["spec"]]
    caps = set()
    for t in spec:
        caps.update(re.findall(r"(?m)^\s*[-*]\s*\*\*(CAP-\d+)\*\*", t))
    if not caps:
        for t in spec:
            caps.update(re.findall(r"\bCAP-\d+\b", t))
    problems, covered = [], set()
    for m in re.finditer(r"(?ms)^###\s+(S\d+)\b(.*?)(?=^###\s|^##\s|\Z)", plan):
        sid, body = m.group(1), m.group(2)
        cl = re.search(r"(?m)^\s*-\s*Covers:\s*(.+?)\s*$", body)
        if not cl:
            problems.append(f"{sid} has no Covers: line")
            continue
        val = cl.group(1)
        ids = ID_RE.findall(val)
        if not ids:
            if not NONE_RE.match(val):
                problems.append(f"{sid} has a Covers: line with no ID (use IDs from the documents, or 'none: <reason>')")
            continue
        unknown = sorted({i for i in ids if i not in known})
        if unknown:
            problems.append(f"{sid} names unknown {', '.join(unknown)}")
        covered.update(ids)
    out = set()
    om = re.search(r"(?ms)^##\s+Out of scope\s*$(.*?)(?=^##\s|\Z)", plan)
    if om:
        for line in om.group(1).splitlines():
            lm = re.match(r"^\s*[-*]\s*(CAP-\d+)\s*:\s*(\S.*)$", line)
            if lm:
                out.add(lm.group(1))
    for c in sorted(caps - covered - out, key=lambda x: int(x.split("-")[1])):
        problems.append(f'{c} is covered by no story (add it to a story\'s Covers: line or list it under "## Out of scope" as "- {c}: <reason>")')
    return problems


def clean_plan_path(x):
    x = x.strip().strip("`").strip()
    while x.startswith("./"):
        x = x[2:]
    return x


def paths_overlap(a, b):
    """The longer path when a and b are equal or one is a folder (trailing /) holding the other, else None."""
    if a == b:
        return a
    if a.endswith("/") and b.startswith(a):
        return b
    if b.endswith("/") and a.startswith(b):
        return a
    return None


def plan_why(text):
    """A PR's `- Why:` line as one safe line: control characters and `|` neutralised, at most 200 characters; None when empty."""
    t = re.sub(r"[\x00-\x1f\x7f]+", " ", text or "").replace("|", "/")
    t = " ".join(t.split())
    return t[:200].rstrip() or None


def plan_blocks(plan):
    """({S id: [files]}, {PR id: {repo, stories, deps, epic, check}}) read from plan.md's ### S<n> / ### PR<n> blocks.
    epic is the raw text of the PR's `- Epic:` line, or None when it has none."""
    stories, prs = {}, {}
    for m in re.finditer(r"(?ms)^###\s+(S\d+)\b(.*?)(?=^###\s|^##\s|\Z)", plan):
        fl = re.search(r"(?m)^[ \t]*-[ \t]*Files:[ \t]*(.*?)[ \t]*$", m.group(2))
        files = [clean_plan_path(x) for x in fl.group(1).split(",")] if fl else []
        stories[m.group(1)] = [x for x in files if x]
    for m in re.finditer(r"(?ms)^###\s+(PR\d+)\b(.*?)(?=^###\s|^##\s|\Z)", plan):
        b = m.group(2)
        g = lambda k: re.search(rf"(?m)^[ \t]*-[ \t]*{k}:[ \t]*(.*?)[ \t]*$", b)
        repo, sl, dl, el, cl, wl = g("Repo"), g("Stories"), g("Depends on"), g("Epic"), g("Check"), g("Why")
        chk = cl.group(1).strip() if cl else ""
        if len(chk) > 1 and chk.startswith("`") and chk.endswith("`") and chk.count("`") == 2:
            chk = chk[1:-1].strip()
        prs[m.group(1)] = {
            "repo": repo.group(1).strip("` ") if repo and repo.group(1).strip("` ") else None,
            "stories": re.findall(r"S\d+", sl.group(1)) if sl else [],
            "deps": re.findall(r"PR\d+", dl.group(1)) if dl else [],
            "epic": el.group(1).strip() if el else None,
            "check": chk or None,
            "why": plan_why(wl.group(1) if wl else None),
        }
    return stories, prs


def plan_story_tickets(plan):
    """{S id: [ticket ref, ...]} from each story's `- Tickets:` line (`none` and `-` are no ticket)."""
    out = {}
    for m in re.finditer(r"(?ms)^###\s+(S\d+)\b(.*?)(?=^###\s|^##\s|\Z)", plan):
        tl = re.search(r"(?m)^[ \t]*-[ \t]*Tickets:[ \t]*(.*?)[ \t]*$", m.group(2))
        refs = []
        for part in re.split(r"\s*,\s*", tl.group(1)) if tl else []:
            tok = part.strip().strip("`").split(" ")[0].strip("`") if part.strip() else ""
            if tok and tok.lower() not in ("none", "-") and tok not in refs:
                refs.append(tok)
        out[m.group(1)] = refs
    return out


def tickets_epics(run):
    """(ticket ref (lowercased id or custom id) -> epic id or None, epic id -> title or None) from tickets.json,
    or (None, {}) when the run has no readable tickets.json. An epic is a ticket's parent_id."""
    tj = load_json(os.path.join(run, "tickets.json"))
    if not isinstance(tj, dict) or not isinstance(tj.get("tasks"), list):
        return None, {}
    emap = {}
    for t in tj["tasks"]:
        if not isinstance(t, dict):
            continue
        pe = t.get("parent_id") if isinstance(t.get("parent_id"), str) and t.get("parent_id") else None
        for k in ("id", "custom_id"):
            if isinstance(t.get(k), str) and t[k]:
                emap[t[k].lower()] = pe
    titles = {}
    for e in tj.get("epics") if isinstance(tj.get("epics"), list) else []:
        if isinstance(e, dict) and isinstance(e.get("id"), str) and e["id"]:
            titles[e["id"]] = e["name"] if isinstance(e.get("name"), str) and e["name"] else None
    return emap, titles


def epic_id_of(line):
    """The epic id in a plan `- Epic:` line (`<id> <title>`), or None for `none`/empty."""
    tok = (line or "").strip().strip("`").split(" ")[0].strip("`") if (line or "").strip() else ""
    return None if not tok or tok.lower() in ("none", "-") else tok


def epic_title_of(line):
    """The title after the id in a plan `- Epic:` line (one line, at most 120 characters), or None."""
    rest = (line or "").strip().strip("`").split(" ", 1)
    t = re.sub(r"[\x00-\x1f\x7f]+", " ", rest[1]).strip().strip("`").strip() if len(rest) > 1 else ""
    return t[:120] or None


def plan_epics(run):
    """{PR id: {id, title} or None} from the plan's Epic lines; titles from tickets.json `epics`, else the Epic line's own
    title, else None."""
    plan = read(os.path.join(run, "plan.md"))
    if not plan:
        return {}
    _, titles = tickets_epics(run)
    out = {}
    for pid, p in plan_blocks(plan)[1].items():
        e = epic_id_of(p["epic"])
        out[pid] = {"id": e, "title": titles.get(e) or epic_title_of(p["epic"])} if e else None
    return out


VERSION_FILES = ("VERSION", "plugin.json", "package.json", "tauri.conf.json", "Cargo.toml")


def same_repo_deps(prs, pid):
    """The Depends on entries of pid that are PRs of the same repo (a stack parent; at most one is valid)."""
    p = prs[pid]
    return [d for d in p["deps"] if d in prs and p["repo"] and prs[d]["repo"] == p["repo"]]


def plan_group_problems(run):
    """Problems with how plan.md groups stories into PRs (empty list = fine): one PR per (epic, repo), the Epic line, stacks
    (a same-repo Depends on is a stack, one parent at most), same-repo Files overlap and version files. The single place for it."""
    plan = read(os.path.join(run, "plan.md"))
    if not plan:
        return []
    stories, prs = plan_blocks(plan)
    tix = plan_story_tickets(plan)
    emap, _ = tickets_epics(run)
    problems = []
    for sid, files in stories.items():
        if not files:
            problems.append(f"{sid} has no Files: line")
    pfiles, epic_of, mixed = {}, {}, set()
    for pid, p in prs.items():
        pfiles[pid] = sorted({f for s in p["stories"] for f in stories.get(s, [])})
        refs = []
        for s in p["stories"]:
            refs += [r for r in tix.get(s, []) if r not in refs]
        known = [(r, emap[r.lower()]) for r in refs if emap is not None and r.lower() in emap]
        epics = list(dict.fromkeys(e for _, e in known if e))
        noepic = [r for r, e in known if not e]
        tk = ", ".join(refs) or "no tickets"
        if len(epics) > 1:
            problems.append(f"{pid} mixes epics {' and '.join(epics)} (tickets {tk}): one PR per epic")
            mixed.add(pid)
        elif epics and noepic:
            problems.append(f"{pid} mixes epic {epics[0]} with the no-epic ticket {', '.join(noepic)} (tickets {tk}): a no-epic ticket gets its own PR")
            mixed.add(pid)
        elif len(noepic) > 1:
            problems.append(f"{pid} holds {len(noepic)} no-epic tickets ({', '.join(noepic)}): a no-epic ticket gets its own PR")
            mixed.add(pid)
        want = epics[0] if len(epics) == 1 and not noepic else None
        epic_of[pid] = want
        if pid in mixed:
            continue
        if p["epic"] is None:
            if emap is not None:
                problems.append(f"{pid} (tickets {tk}) has no Epic: line (expected `- Epic: {want or 'none'}`)")
        elif epic_id_of(p["epic"]) != want:
            problems.append(f"{pid} (tickets {tk}) has Epic: {p['epic']!r} but its tickets are in epic {want or 'none'}")
    groups = {}
    for pid, e in epic_of.items():
        if e and pid not in mixed:
            groups.setdefault((e, prs[pid]["repo"]), []).append(pid)
    for (e, repo), ids in groups.items():
        if len(ids) > 1:
            problems.append(f"epic {e} is split across {' and '.join(ids)} in repo {repo or '?'} (even if stacked): one PR per (epic, repo)")
    same = {pid: same_repo_deps(prs, pid) for pid in prs}
    for pid, ds in same.items():
        if len(ds) > 1:
            problems.append(f"{pid} depends on {' and '.join(ds)} of the same repo ({prs[pid]['repo']}): at most one same-repo Depends on "
                            f"(a stack has one parent: chain them, {' <- '.join(ds + [pid])})")
    parent = {pid: ds[0] for pid, ds in same.items() if ds}

    def ancestors(pid):
        out, x = [], parent.get(pid)
        while x and x not in out and x != pid:
            out.append(x)
            x = parent.get(x)
        return out

    seen = set()
    for pid in prs:
        x, trail = pid, []
        while x in parent and x not in trail:
            trail.append(x)
            x = parent[x]
        if x in trail and x in parent:
            cyc = frozenset(trail[trail.index(x):])
            if cyc not in seen:
                seen.add(cyc)
                problems.append(f"same-repo Depends on cycle: {' -> '.join(sorted(cyc, key=lambda i: int(i[2:])))}")
    ids = list(prs)
    for i, a in enumerate(ids):
        for b in ids[i + 1:]:
            ra, rb = prs[a]["repo"], prs[b]["repo"]
            if not ra or ra != rb:
                continue
            chained = a in ancestors(b) or b in ancestors(a)
            for fa in pfiles[a]:
                for fb in pfiles[b]:
                    hit = paths_overlap(fa, fb)
                    if not hit:
                        continue
                    if fa == fb and os.path.basename(fa) in VERSION_FILES:
                        problems.append(f"{a} and {b} ({ra}) both change the version file {fa}: it goes in one PR per repo")
                    elif not chained:
                        problems.append(f"{a} and {b} ({ra}) both change {hit} and neither is stacked on the other: chain them "
                                        f"(`Depends on`, e.g. {a} <- {b}), never two overlapping children of one parent")
    return problems


def gates(doc, run, old=None, plan_trace=True):
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
    if stage == "plan-approval" and bmad_mode(run) != "off":
        b = bmad_obj(doc)
        if bmad_mode(run) == "auto" and not b.get("size"):
            return "NOT WRITTEN: BMAD auto: run the sizing pass and state.sh bmad-size first", out
        if bmad_applied(run, doc):
            chk = b.get("check") if isinstance(b.get("check"), dict) else {}
            if chk.get("result") != "pass":
                return f"NOT WRITTEN: BMAD mode: state.sh bmad-check {run} must pass first", out
            if chk.get("digest") != bmad_digest(run, bmad_find(run)):
                return f"NOT WRITTEN: BMAD mode: documents changed since the last bmad-check; run state.sh bmad-check {run} again and make it pass", out
            if plan_trace:
                pr = bmad_plan_trace(run)
                if pr:
                    more = f"; and {len(pr) - 10} more" if len(pr) > 10 else ""
                    return "NOT WRITTEN: BMAD mode: plan.md does not trace to the BMAD documents: " + "; ".join(pr[:10]) + more, out
    if stage == "plan-approval" and plan_trace:
        sp = plan_group_problems(run)
        if sp:
            more = f"; and {len(sp) - 10} more" if len(sp) > 10 else ""
            return "NOT WRITTEN: plan.md grouping: " + "; ".join(sp[:10]) + more, out
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


def write_status(run, doc, trusted=False, plan_trace=True, guard=None, dry=False):
    """Validate and write status.json. guard: the .guard.json content to check against instead of the file's (restack moves
    bases[pid] after the status write); dry: validate and run gates() only, write nothing."""
    dest = os.path.join(run, "status.json")
    old = load_json(dest)
    notes, bad = prepare_status(doc, old, trusted, run)
    problems = bad + validate_status(doc, run, guard)
    if problems:
        die(f"NOT WRITTEN: {dest}: " + "; ".join(problems))
    refusal, out = gates(doc, run, old, plan_trace)
    if refusal:
        die(refusal)
    if dry:
        return notes + out
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
    low = name.casefold()
    if low in ("writeback.json", ".guard.json"):
        die(f"NOT WRITTEN: {name} is written only by its own state.sh command")
    if low in [a.casefold() for a in APP_FILES]:
        die(f"NOT WRITTEN: {name} is written only by the app")
    if os.path.basename(os.path.dirname(os.path.abspath(dest))).casefold() == "requests":
        die("NOT WRITTEN: requests/ is written only by `state.sh request` and the app")
    run_env = os.environ.get("DCV2_RUN_DIR", "")
    if not run_env:
        die("NOT WRITTEN: put writes the run's status.json, and DCV2_RUN_DIR (the run folder, set when claude is launched) is not set")
    if name != "status.json" or not same_path(dest, os.path.join(run_env, "status.json")):
        die(f"NOT WRITTEN: put writes only <DCV2_RUN_DIR>/status.json ({os.path.join(run_env, 'status.json')}), not {dest}")
    text = sys.stdin.read()
    try:
        doc = json.loads(text)
    except ValueError as e:
        die(f"NOT WRITTEN: invalid JSON for {dest}: {e}")
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
    print(f"bmad={bmad_mode(run)}")
    print(f"node={node_major()[1] or 'missing'}")
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
        # Freeze what the user approved (written to status.json only if the grouping check passes): each PR's Files.
        # Their presence also marks a plan approved under the 0.7.0+ rules (the gate then checks overlaps against them).
        fstories, fprs = plan_blocks(read(os.path.join(run, "plan.md")) or "")
        st["plan"]["prFiles"] = {pid: sorted({f for s in p["stories"] for f in fstories.get(s, [])}) for pid, p in fprs.items()}
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


def cmd_bmad_size(run, size, rest):
    run = os.path.abspath(run)
    docs, by, i = [], "none", 0
    while i < len(rest):
        if rest[i] == "--docs" and i + 1 < len(rest):
            docs = [d for d in rest[i + 1].split(",") if d]
            i += 2
        elif rest[i] == "--by" and i + 1 < len(rest):
            by = rest[i + 1]
            i += 2
        else:
            die("usage: state.sh bmad-size <run> Small|Medium|Large [--docs prd,ux] [--by planner|plan-reviewer]", 2)
    if size not in BMAD_SIZES:
        die(f"NOT WRITTEN: size must be one of {', '.join(BMAD_SIZES)} (got {size!r})")
    if any(d not in BMAD_DOCS for d in docs):
        die(f"NOT WRITTEN: --docs takes {', '.join(BMAD_DOCS)}")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,40}", by):
        die("NOT WRITTEN: --by must be a short word (planner, plan-reviewer)")
    mode = bmad_mode(run)
    if mode == "off":
        die("NOT WRITTEN: BMAD mode is off for this run (meta.json bmad.mode); nothing to size")
    st = load_status(run)
    if st.get("stage") != "planning":
        die(f"NOT WRITTEN: bmad-size runs at stage planning (stage {st.get('stage')})")
    reasons = [" ".join(l.split())[:200] for l in sys.stdin.read().splitlines() if l.strip()][:10]
    b = dict(bmad_obj(st))
    old = b.get("size")
    if old in BMAD_SIZES and BMAD_SIZES.index(size) < BMAD_SIZES.index(old):
        die(f"NOT WRITTEN: BMAD size can only go up (recorded {old})")
    b["mode"] = mode
    b["size"] = size
    b["reasons"] = reasons if reasons else (b.get("reasons") or [])
    b["docs"] = sorted(set(b.get("docs") or []) | set(docs))
    b["applied"] = bool(b.get("applied")) or mode == "on" or size == "Large"
    st["bmad"] = b
    for m in write_status(run, st, trusted=True):
        print(m)
    append_log(run, "planning", f"BMAD size {size} recorded by {by}; applied={'yes' if b['applied'] else 'no'}; docs={','.join(b['docs']) or 'none'}")
    print(f"bmad-size={size} applied={'yes' if b['applied'] else 'no'} docs={','.join(b['docs']) or 'none'}")


def cmd_bmad_init(run):
    run = os.path.abspath(run)
    if not bmad_applied(run):
        die("NOT INITIALISED: BMAD is not applied to this run (mode off, or auto without a Large size)")
    major, _ = node_major()
    if major is None or major < 18:
        die("NOT INITIALISED: BMAD needs Node 18+ (node is missing or too old)")
    out = os.path.join(run, "bmad", "output")
    cfg_dir = os.path.join(run, "bmad", "_bmad")
    os.makedirs(out, exist_ok=True)
    os.makedirs(cfg_dir, exist_ok=True)
    cfg = os.path.join(cfg_dir, "config.toml")
    if os.path.exists(cfg):
        print(f"bmad-init=exists {cfg}")
        return
    repos = load_json(os.path.join(run, "repos.json"))
    names = [r.get("name") for r in (repos.get("repos") if isinstance(repos, dict) else None) or [] if isinstance(r, dict) and r.get("name")]
    if not names:
        m = re.search(r"^folder:\s*(\S.*?)\s*$", read(os.path.join(run, "request.md")), re.M)
        names = [os.path.basename(m.group(1).rstrip("/"))] if m else []
    text = ("[core]\n"
            f"project_name = {toml_str(', '.join(names) or 'project')}\n"
            f"output_folder = {toml_str(os.path.realpath(out))}\n"
            f"active_initiative = {toml_str(os.path.basename(os.path.realpath(run)))}\n")
    atomic_write(cfg, text)
    append_log(run, (load_json(os.path.join(run, "status.json")) or {}).get("stage") or "planning", "BMAD workspace initialised")
    print(f"bmad-init=ok {cfg}")


def bmad_check_docs(run, docs):
    """(reasons, found): why the BMAD documents are not ready."""
    found = bmad_find(run)
    reasons = []
    texts = {}

    def body(f, why):
        if f not in texts:
            try:
                with open(f, encoding="utf-8") as fh:
                    texts[f] = fh.read()
            except (OSError, UnicodeDecodeError):
                texts[f] = ""
        rel = os.path.relpath(f, run)
        if not texts[f].strip():
            reasons.append(f"{rel} is empty or unreadable")
            return None
        left = bmad_placeholders(texts[f])
        if left:
            reasons.append(f"{rel} has unfilled template text: {', '.join(left)[:120]}")
        return texts[f]

    def reports(f):
        d = os.path.dirname(f)
        for name in ("validation-report.md", "validation-report.html"):
            rp = os.path.join(d, name)
            if not os.path.isfile(rp) or os.path.getsize(rp) == 0:
                reasons.append(f"{os.path.relpath(rp, run)} is missing or empty: run the skill's Validate intent after the last edit")
            elif os.path.getmtime(rp) < os.path.getmtime(f):
                reasons.append(f"{os.path.relpath(rp, run)} is older than {os.path.basename(f)}: run Validate again")

    if not found["spec"]:
        reasons.append("no spec: bmad/output/<initiative>/spec-<slug>/spec-<slug>.md")
    for f in found["spec"]:
        t = body(f, "spec")
        if t is not None and not bmad_frontmatter(t).get("id", "").startswith("SPEC-"):
            reasons.append(f"{os.path.relpath(f, run)} frontmatter has no id: SPEC-…")
    if not found["architecture"]:
        reasons.append("no architecture spine: bmad/output/<initiative>/architecture-<slug>/architecture-<slug>.md")
    for f in found["architecture"]:
        if body(f, "architecture") is None:
            continue
        env = dict(os.environ, DCV2_RUN_DIR=os.path.realpath(run))
        rt = os.path.join(PLUGIN_ROOT, "bmad", "runtime.mjs")
        skill = os.path.join(PLUGIN_ROOT, "bmad", "skills", "bmad-architecture")
        try:
            r = subprocess.run(["node", rt, "lint_spine", "--skill-root", skill, "--workspace", os.path.realpath(os.path.dirname(f))],
                               capture_output=True, text=True, timeout=60, env=env)
            res = json.loads(r.stdout) if r.returncode == 0 else None
        except (OSError, subprocess.TimeoutExpired, ValueError):
            res = None
        if not isinstance(res, dict) or res.get("ok") is not True:
            why = "lint_spine did not run" if not isinstance(res, dict) else (res.get("error") or "; ".join(
                str(x.get("detail")) for x in (res.get("findings") or [])[:3] if isinstance(x, dict)) or "findings")
            reasons.append(f"{os.path.relpath(f, run)} fails lint_spine: {why}"[:300])
    for kind, label in (("prd", "PRD"), ("ux", "UX")):
        if kind in docs and not found[kind]:
            reasons.append(f"docs names {kind} but there is no {label} document")
        for f in found[kind]:
            t = body(f, kind)
            if t is not None and bmad_frontmatter(t).get("status") != "final":
                reasons.append(f"{os.path.relpath(f, run)} frontmatter status is not final")
            reports(f)
    if "ux" in docs and found["ux"]:
        names = {os.path.basename(f) for f in found["ux"]}
        for need in ("DESIGN.md", "EXPERIENCE.md"):
            if need not in names:
                reasons.append(f"UX is missing {need}")
    return list(dict.fromkeys(reasons)), found  # each reason once, in order


def cmd_bmad_check(run):
    run = os.path.abspath(run)
    st = load_status(run)
    if bmad_mode(run) == "off" or not bmad_applied(run, st):
        die("NOT CHECKED: BMAD is not applied to this run")
    b = dict(bmad_obj(st))
    reasons, found = bmad_check_docs(run, b.get("docs") or [])
    chk = {"result": "fail" if reasons else "pass", "at": now(), "digest": bmad_digest(run, found),
           "documents": sorted(os.path.relpath(f, run) for fs in found.values() for f in fs), "reasons": reasons}
    b["mode"] = bmad_mode(run)
    b["check"] = chk
    st["bmad"] = b
    for m in write_status(run, st, trusted=True, plan_trace=False):
        print(m)
    append_log(run, st.get("stage") or "planning", f"bmad-check {chk['result']}: {len(chk['documents'])} document(s)" + (f"; {len(reasons)} problem(s)" if reasons else ""))
    if reasons:
        print("bmad-check=fail " + "; ".join(reasons))
        sys.exit(3)
    print("bmad-check=pass")


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
    if what == "prs":
        _, prs = plan_blocks(text)
        titles = {}
        for pid, p in prs.items():
            e = epic_id_of(p["epic"])
            titles[pid] = f"epic {e} {(p['epic'] or '').strip().strip('`')[len(e):].strip()[:40]}".rstrip() if e else "epic none"
        for pid, p in prs.items():
            sp = same_repo_deps(prs, pid)
            head = " | ".join([pid, (p["repo"] or "-")[:40], titles[pid], (",".join(p["stories"]) or "-")[:60],
                               "depends " + (",".join(p["deps"])[:40] or "-"), "stacked on " + (sp[0] if sp else "-"),
                               "why " + (p["why"] or "-")])
            print(f"{head} | check {p['check'] or '-'}")
        if prs:
            return
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


def record_guard(run, repo, pr_id, wt, copied=None, remote=None, base=None, stack=None):
    path = os.path.join(run, ".guard.json")
    with open(os.path.join(run, ".guard.lock"), "a") as lock:  # two `worktree` calls at once must not lose a record
        fcntl.flock(lock, fcntl.LOCK_EX)
        g = load_json(path)
        if not isinstance(g, dict) or g.get("runDir") != run:
            g = {"schema": 1, "runDir": run, "repos": [], "worktrees": {}}
        if repo not in g["repos"]:
            g["repos"].append(repo)
        g["worktrees"][pr_id] = wt
        if copied is not None:  # files copied in from the main checkout: never staged or committed (guard)
            old = g.setdefault("copied", {}).get(pr_id) or []
            g["copied"][pr_id] = sorted(set(old) | set(copied))
        if base and pr_id not in (g.get("bases") or {}):  # the base commit the worktree was made from (fixed)
            g.setdefault("bases", {})[pr_id] = base
        if stack and pr_id not in (g.get("stacks") or {}):  # the PR this one is stacked on (its base is that PR's branch), fixed
            g.setdefault("stacks", {})[pr_id] = stack
        if remote and pr_id not in (g.get("remotes") or {}):  # the push target, fixed when the worktree is first made
            g.setdefault("remotes", {})[pr_id] = remote
        atomic_write(path, json.dumps(g, indent=2) + "\n")


# https / ssh URLs, user@host:owner/name, and an ssh host alias (alias:owner/name, e.g. github-work:o/rr; the alias cannot start with -).
SLUG = re.compile(r"^(?:https?://[^/\s]+/|ssh://[^/\s]+/|[\w.-]+@[^:/\s]+:|[A-Za-z0-9_][\w.-]*:)([\w.-]+)/([\w.-]+?)(?:\.git)?/?$")


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


def run_setup(run, wt, pr_id, cmd, timeout=None, env=None, tracker=None):
    """Run the worktree's setup command (own process group, killed whole on timeout); never raises.
    `pr_id` is the log's label (<run>/logs/<label>-setup.txt). `timeout` (seconds, default DCV2_SETUP_TIMEOUT or 600), `env`
    and `tracker` (start(proc) / end(), for merge-check's kill-on-interrupt record) are optional."""
    log = os.path.join(run, "logs", f"{pr_id}-setup.txt")
    if not cmd:
        return "none", None
    os.makedirs(os.path.dirname(log), exist_ok=True)
    timeout = timeout if timeout is not None else int(os.environ.get("DCV2_SETUP_TIMEOUT", "") or 600)
    try:  # never through a link planted at the log's name
        fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o644)
    except OSError as e:
        print(f"note: the setup log {log} could not be opened ({e.strerror}); the setup did not run", file=sys.stderr)
        return "failed", log
    with os.fdopen(fd, "w") as out:
        out.write(f"$ {cmd}\n(in {wt})\n")
        out.flush()
        try:
            p = subprocess.Popen(["bash", "-c", cmd], cwd=wt, stdout=out, stderr=subprocess.STDOUT,
                                 stdin=subprocess.DEVNULL, start_new_session=True, env=env)
        except OSError as e:
            out.write(f"could not start: {e}\n")
            return "failed", log
        if tracker:
            tracker.start(p)
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
        finally:
            if tracker:
                tracker.end()
        out.write(f"\nexit code {rc}\n")
    return ("ok" if rc == 0 else "failed"), log


def run_owns_repo(run, repo):
    """The repo is one the run is about: listed in repos.json (by real path, so a symlinked listing matches) or inside the
    start folder named by request.md's folder: line."""
    rj = load_json(os.path.join(run, "repos.json"))
    for r in (rj.get("repos") if isinstance(rj, dict) else None) or []:
        if isinstance(r, dict) and isinstance(r.get("path"), str) and same_path(r["path"], repo):
            return True
    folders = set(re.findall(r"^folder:\s*(\S.*?)\s*$", read(os.path.join(run, "request.md")), re.M))
    return len(folders) == 1 and os.path.isabs(next(iter(folders))) and under(repo, next(iter(folders)))


def parent_head(run, st, parent):
    """(sha, None) of a stack parent's current head, else (None, why): its local branch, else its worktree HEAD (the janitor may
    have removed the branch), else its pushed branch on the recorded remote."""
    repo = os.path.realpath(str(parent.get("repoPath") or "/"))
    branch, wt = canon(repo, st.get("runId"), parent.get("id"))
    r = git("-C", repo, "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}^{{commit}}")
    if r.returncode == 0 and r.stdout.strip():
        return r.stdout.strip(), None
    if os.path.isdir(wt):
        r = git("-C", wt, "rev-parse", "--verify", "--quiet", "HEAD^{commit}")
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip(), None
    rem = (guard_rec(run).get("remotes") or {}).get(parent.get("id"))
    if isinstance(rem, dict) and rem.get("url") and not url_problem(rem["url"]):
        tip, err = remote_tip(rem["url"], branch)
        if tip:
            if git("-C", repo, "cat-file", "-e", f"{tip}^{{commit}}").returncode != 0:
                f = net_git(rem["url"], ["-C", repo, "fetch", "--no-tags", "--end-of-options", rem["url"], f"refs/heads/{branch}"], cwd=repo)
                if f.returncode != 0:
                    return None, f"{parent.get('id')}'s branch {branch} is only on the remote and the fetch failed: {first_line(f.stderr)[:160]}"
            if git("-C", repo, "cat-file", "-e", f"{tip}^{{commit}}").returncode == 0:
                return tip, None
        elif err:
            return None, f"{parent.get('id')}'s branch {branch} is gone locally and the remote could not be asked: {err[:160]}"
    return None, f"{parent.get('id')}'s head cannot be found: no branch {branch}, no worktree {wt}, not on the remote"


def cmd_worktree(run, repo, pr_id, base):
    run = os.path.realpath(run)
    run_id = os.path.basename(run)
    repo = os.path.realpath(repo)
    if not re.fullmatch(r"PR\d+", pr_id):
        die(f"pr id must be PR<n> (got {pr_id})")
    if not run_owns_repo(run, repo):
        die(f"NOT MADE: {repo} is not one of this run's repos (not in repos.json, and not inside the start folder in request.md); "
            "nothing was recorded in .guard.json")
    branch, wt = canon(repo, run_id, pr_id)
    st = load_json(os.path.join(run, "status.json")) or {}
    st_prs = [p for p in st.get("prs") or [] if isinstance(p, dict)]
    rec = next((p for p in st_prs if p.get("id") == pr_id), {})
    # The PR's one same-repo Depends on is its stack parent (status.json's dependsOn, and plan.md's when it is there).
    sd = same_repo_ids(st_prs, {**rec, "id": pr_id, "repoPath": repo}) if rec else []
    _, plan_prs = plan_blocks(read(os.path.join(run, "plan.md")) or "")
    for d in same_repo_deps(plan_prs, pr_id) if pr_id in plan_prs else []:
        if d not in sd:
            sd.append(d)
    if len(sd) > 1:
        die(f"NOT MADE: {pr_id} depends on {' and '.join(sd)} of the same repo: a stack has one parent (chain them)")
    parent = sd[0] if sd else None
    g = guard_rec(run)
    listed = f"worktree {wt}" in git("-C", repo, "worktree", "list", "--porcelain").stdout.splitlines()
    have = (listed and os.path.isdir(wt)) or bool((g.get("worktrees") or {}).get(pr_id)) \
        or git("-C", repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}").returncode == 0
    stack = None  # (parent id, parent branch)
    if re.fullmatch(r"PR\d+", base):  # a stacked PR: the base is its parent, a reviewed PR of the same repo
        if base != parent:
            die(f"NOT MADE: {base} is not {pr_id}'s one same-repo Depends on ({parent or 'none'}); a stack is built from that parent only")
        prec = next((p for p in st_prs if p.get("id") == base), None)
        if prec is None or os.path.realpath(str(prec.get("repoPath") or "/")) != repo:
            die(f"NOT MADE: {base} is not a PR of {repo} in status.json")
        recorded = (g.get("stacks") or {}).get(pr_id)
        if recorded and recorded != base:
            die(f"NOT MADE: {pr_id} is recorded as stacked on {recorded}, not {base}")
        if have:  # resume: keep what is recorded
            eff = (g.get("bases") or {}).get(pr_id) or rec.get("baseCommit") or ""
            if not eff or git("-C", repo, "rev-parse", "--verify", "--quiet", f"{eff}^{{commit}}").returncode != 0:
                eff, why = parent_head(run, st, prec)
                if not eff:
                    die(f"NOT MADE: {why}")
        else:
            if prec.get("state") != "reviewed":
                die(f"NOT MADE: {pr_id} is stacked on {base}, which is {prec.get('state')}: make its worktree once {base} is reviewed")
            eff, why = parent_head(run, st, prec)
            if not eff:
                die(f"NOT MADE: {why}")
        stack = (base, canon(repo, run_id, base)[0])
        base = eff
    else:
        if parent and not have:
            die(f"NOT WRITTEN: {pr_id} is stacked on {parent}: pass {parent} as the base once it is reviewed "
                f"(`state.sh worktree <run> <repo> {pr_id} {parent}`), not a commit")
        if git("-C", repo, "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}").returncode != 0:
            die(f"NOT MADE: base commit {base!r} is not a commit in {repo}; use baseCommit from status.json / state.sh base")
        if parent and (g.get("stacks") or {}).get(pr_id):
            stack = (g["stacks"][pr_id], canon(repo, run_id, g["stacks"][pr_id])[0])
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
    stack_line = f"base={stack[1]} baseCommit={(g.get('bases') or {}).get(pr_id) or base} stackedOn={stack[0]}" if stack else None
    if not made:
        record_guard(run, repo, pr_id, wt, remote=remote, base=base, stack=stack[0] if stack else None)
        print(f"branch={branch} worktree={wt}")
        if stack_line:
            print(stack_line)
        return
    # A newly made worktree (fresh or re-made folder): local files, then the install step.
    entry = repo_entry(run, repo)
    copied, refused = copy_files(repo, wt, entry.get("copyFiles"))
    record_guard(run, repo, pr_id, wt, copied, remote, base, stack=stack[0] if stack else None)
    print(f"branch={branch} worktree={wt}" + (" remade=yes" if remade else ""))
    if stack:
        print(f"base={stack[1]} baseCommit={(guard_rec(run).get('bases') or {}).get(pr_id) or base} stackedOn={stack[0]}")
    if copied or refused:
        print(f"copied={len(copied)}" + (f" refused: {'; '.join(refused)}" if refused else ""))
    setup = entry.get("setup")
    # Old runs (meta.json without reviewMode, plugin 0.2.x) get no detected install: only an explicit setup.
    meta = load_json(os.path.join(run, "meta.json"))
    new_run = isinstance(meta, dict) and "reviewMode" in meta
    cmd = setup.strip() if isinstance(setup, str) else (detect_setup(wt) if new_run else None)
    result, log = run_setup(run, wt, pr_id, cmd or None)
    print(f"setup={result} log={log or '-'}")


def move_base(run, pid, sha):
    """Set .guard.json bases[pid] (restack is the one writer that moves it), under the guard lock. Raises OSError."""
    path = os.path.join(run, ".guard.json")
    with open(os.path.join(run, ".guard.lock"), "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        g = load_json(path)
        if not isinstance(g, dict):
            raise OSError(".guard.json is missing or unreadable")
        g.setdefault("bases", {})[pid] = sha
        atomic_write(path, json.dumps(g, indent=2) + "\n")


def archive_restack(run, pid, k):
    """Best effort: copy the old gate and review files to -restack<k> names, so the new rounds 1-3 do not erase them."""
    notes = []
    try:
        gf = os.path.join(run, "gates", f"{pid}.yml")
        if os.path.isfile(gf):
            shutil.copy2(gf, os.path.join(run, "gates", f"{pid}-restack{k}.yml"))
        for n in (1, 2, 3):
            rf = os.path.join(run, "reviews", f"{pid}-round{n}.md")
            if os.path.isfile(rf):
                shutil.copy2(rf, os.path.join(run, "reviews", f"{pid}-restack{k}-round{n}.md"))
    except OSError as e:
        notes.append(f"note: the old gate / review files were not copied ({e.strerror or e})")
    return notes


def cmd_restack(run, pid):
    """Bring a stacked PR's parent changes in with a merge (never a rebase, never a force), then void its review."""
    run = this_run(run, "NOT RESTACKED")
    what = "NOT RESTACKED"
    st = load_status(run)
    pr, _ = canonical_pr(run, st, pid, what)
    parent_id = pr.get("stackedOn")
    if not parent_id:
        die(f"{what}: {pid} is not stacked on another PR (no stackedOn)")
    parent = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == parent_id), None)
    if parent is None:
        die(f"{what}: {parent_id}, the parent of {pid}, is not in status.json")
    if parent.get("state") != "reviewed":
        die(f"{what}: {parent_id} is {parent.get('state')}; restack {pid} once {parent_id} is reviewed again")
    if pr.get("state") in ("building", "fixing"):  # an Implementer is working in that worktree
        die(f"{what}: {pid} is {pr.get('state')}; restack it once its Implementer has finished")
    wt = pr["worktree"]
    if git("-C", wt, "rev-parse", "-q", "--verify", "MERGE_HEAD").returncode == 0:
        die(f"{what}: a merge is in progress in {wt}; finish or abort it first")
    dirty = git("-C", wt, "status", "--porcelain")
    if dirty.returncode != 0 or dirty.stdout.strip():
        die(f"{what}: {wt} has uncommitted changes (or git status failed); commit or stash them first")
    phead, why = parent_head(run, st, parent)
    if not phead:
        die(f"{what}: {why}")
    old_k = pr.get("restack") if type(pr.get("restack")) is int else 0
    old_doc = json.loads(json.dumps(st))
    guard = guard_rec(run)
    new_guard = json.loads(json.dumps(guard))
    new_guard.setdefault("bases", {})[pid] = phead

    def new_doc():
        doc = json.loads(json.dumps(st))
        npr = next(p for p in doc["prs"] if p.get("id") == pid)
        npr["state"], npr["note"], npr["baseCommit"], npr["restack"] = "pending", None, phead, old_k + 1
        npr["check"] = {**(npr.get("check") if isinstance(npr.get("check"), dict) else {}), "result": "not-run", "summary": None, "log": None}
        npr["review"] = {"round": 0, "verdict": None, "openFindings": 0, "path": None}
        mc = doc.get("mergeChecks")
        if isinstance(mc, dict):
            doc["mergeChecks"] = {k: v for k, v in mc.items() if k != npr.get("repo") and pid not in (v.get("prs") or [])}
        return doc

    # The dry run: the same validation and gates() as the real write, before any git change.
    write_status(run, new_doc(), trusted=True, guard=new_guard, dry=True)
    pre = git("-C", wt, "rev-parse", "HEAD").stdout.strip()
    if git("-C", wt, "merge-base", "--is-ancestor", phead, "HEAD").returncode != 0:
        who = [] if git("-C", wt, "config", "user.email").stdout.strip() else ["-c", "user.name=dcv2", "-c", "user.email=dcv2@localhost"]
        m = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", *who, "-C", wt, "merge", "--no-edit", phead], capture_output=True, text=True)
        if m.returncode != 0:
            files = git("-C", wt, "diff", "--name-only", "--diff-filter=U").stdout.split()
            if git("-C", wt, "rev-parse", "-q", "--verify", "MERGE_HEAD").returncode == 0:
                git("-C", wt, "merge", "--abort")
            if git("-C", wt, "rev-parse", "HEAD").stdout.strip() != pre:
                git("-C", wt, "reset", "--keep", pre)
            if not files:
                die(f"{what}: git merge of {parent_id} failed: {first_line(m.stderr) or first_line(m.stdout)}")
            attention(run, st, pr, f"restack onto {parent_id} conflicts in {', '.join(files[:6])}{' and more' if len(files) > 6 else ''}: "
                      f"merge {parent_id} (`git merge {phead[:12]}`) in the worktree, resolve, commit, then `state.sh restack` again", what)
    merged = git("-C", wt, "rev-parse", "HEAD").stdout.strip() != pre
    written = False

    def undo(reason):
        notes = []
        if merged and git("-C", wt, "reset", "--keep", pre).returncode != 0:
            notes.append(f"the merge commit could not be undone: run `git -C {wt} reset --keep {pre}`")
        if written:
            try:
                write_status(run, json.loads(json.dumps(old_doc)), trusted=True)
            except (SystemExit, OSError):
                notes.append("status.json could not be put back")
        die(f"{what}: {reason}; HEAD, baseCommit and bases are unchanged" + (f" ({'; '.join(notes)})" if notes else ""))

    try:
        write_status(run, new_doc(), trusted=True, guard=new_guard)
        written = True
    except SystemExit as e:
        undo(f"status.json was refused (exit {e.code}); see the message above")
    except OSError as e:
        undo(f"status.json could not be written ({e.strerror or e})")
    try:
        move_base(run, pid, phead)
    except OSError as e:
        undo(f".guard.json could not be written ({e.strerror or e})")
    notes = archive_restack(run, pid, old_k)
    try:
        append_log(run, st.get("stage") or "building", f"{pid} restacked on {parent_id} ({'merged ' + phead[:12] if merged else 'already up to date'}): "
                   f"pending, check to run again, review rounds start over (restack {old_k + 1})")
    except OSError:
        notes.append("note: log.md not written")
    print(f"restack=ok {pid} parent={parent_id} baseCommit={phead} merge={'new' if merged else 'none'} restack={old_k + 1} state=pending")
    for n in notes:
        print(n)


def recorded_remote(run, pr):
    """The push remote of a PR as ship-pr will use it: the one recorded in .guard.json when its worktree was made
    ({name, url, repo}), else the repo's current one; None when the repo has no remote."""
    rem = (guard_rec(run).get("remotes") or {}).get(pr.get("id"))
    if isinstance(rem, dict) and rem.get("url"):
        return rem
    repo = os.path.realpath(pr["repoPath"]) if pr.get("repoPath") else ""
    return remote_record(run, repo) if repo and os.path.isdir(repo) else None


def stack_root(st, pr):
    """The PR at the bottom of pr's stack (pr itself when it is not stacked)."""
    by = {p.get("id"): p for p in st.get("prs") or [] if isinstance(p, dict)}
    seen, cur = {pr.get("id")}, pr
    while cur.get("stackedOn") in by and cur["stackedOn"] not in seen:
        seen.add(cur["stackedOn"])
        cur = by[cur["stackedOn"]]
    return cur


def cmd_ship_check(run):
    st = load_status(run)
    for pr in st.get("prs") or []:
        so = pr.get("stackedOn")
        who = f"{pr.get('id')} {pr.get('repo')}" + (f": stacked on {so} (ships after it)" if so else "")
        sep = "; " if so else ": "
        rem = recorded_remote(run, pr)
        if not rem:
            print(f"{who}{sep}no git remote: will not push or open a PR")
            continue
        base = (stack_root(st, pr).get("base") if so else pr.get("base")) or ""  # a stack's parent branch is pushed when the parent ships
        why = url_problem(rem["url"])
        if why:
            print(f"{who}{sep}remote {rem['url']} cannot be pushed by the run ({why}): needs attention")
        elif remote_sha(rem["url"], base):
            print(f"{who}{sep}remote {rem['url']}, base {base} is on the remote")
        else:
            print(f"{who}{sep}remote {rem['url']}, base {base} is NOT on the remote: needs attention")


def pr_number(url):
    m = GH_URL.match(str(url or ""))
    return m.group(3) if m else None


def epic_siblings(st, pr):
    """The other PRs of the same epic (prs[].epic.id), when the epic has 2 or more PRs in this run."""
    ep = pr.get("epic")
    if not (isinstance(ep, dict) and ep.get("id")):
        return []
    return [q for q in st.get("prs") or [] if isinstance(q, dict) and q.get("id") != pr.get("id")
            and isinstance(q.get("epic"), dict) and q["epic"].get("id") == ep["id"]]


def stack_epic_lines(st, pr):
    """The PR body's stack line and Epic section (empty lists when the PR is neither stacked nor in a multi-PR epic)."""
    lines = []
    so = pr.get("stackedOn")
    if so:
        parent = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == so), {})
        n = pr_number(parent.get("prUrl"))
        lines += ["", f"Stacked on #{n}. Merge #{n} first." if n else f"Stacked on {so}. Merge {so} first."]
    sibs = epic_siblings(st, pr)
    if sibs:
        ep = pr["epic"]
        lines += ["", "## Epic", "", f"Epic {ep['id']}" + (f": {ep['title']}" if ep.get("title") else "") + f" ({len(sibs) + 1} PRs)"]
        lines += [f"- {q.get('repo')}: {q.get('prUrl') or str(q.get('id')) + ' (not opened yet)'}" for q in sibs]
    return lines


HEX_SHA = re.compile(r"[0-9a-f]{7,64}")
LOCK_FILES = {"package-lock.json", "yarn.lock", "pnpm-lock.yaml", "npm-shrinkwrap.json", "bun.lockb", "bun.lock", "Cargo.lock",
              "go.sum", "poetry.lock", "Pipfile.lock", "uv.lock", "Gemfile.lock", "composer.lock"}
OTHER_MANIFESTS = {"Gemfile", "Pipfile", "pom.xml", "build.gradle", "build.gradle.kts", "composer.json", "setup.py", "setup.cfg", "mix.exs"}


def canon_wt(run, st, pr):
    """The PR's canonical worktree (where `state.sh worktree` makes it, on its branch), else None."""
    try:
        if not os.path.isabs(str(pr.get("repoPath") or "")):
            return None
        branch, wt = canon(os.path.realpath(pr["repoPath"]), st.get("runId"), pr.get("id"))
        if not os.path.isdir(wt) or os.path.realpath(str(pr.get("worktree") or "")) != wt:
            return None
        if git("-C", wt, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() != branch:
            return None
        return wt
    except (OSError, ValueError, TypeError):
        return None


def scope_deps_package_json(text):
    try:
        d = json.loads(text or "{}")
    except ValueError:
        return {}
    out = {}
    if isinstance(d, dict):
        for sec in ("dependencies", "devDependencies"):
            if isinstance(d.get(sec), dict):
                for k, v in d[sec].items():
                    out[str(k)] = str(v)
    return out


def scope_deps_toml(text):
    """name -> version for the dependency tables of a Cargo.toml / pyproject.toml (line based, best effort)."""
    out, sect, cur = {}, "", None
    for line in (text or "").splitlines():
        s = line.strip()
        m = re.match(r"^\[\s*([^\]]+?)\s*\]$", s)
        if m:
            sect = m.group(1)
            sm = re.match(r"^(?:[\w.\-\"']+\.)?(?:dev-|build-)?dependencies\.([\w\-]+)$", sect)
            cur = sm.group(1) if sm else None
            if cur:
                out.setdefault(cur, "*")
            continue
        if cur:
            vm = re.match(r'^version\s*=\s*"([^"]*)"', s)
            if vm:
                out[cur] = vm.group(1)
            continue
        if not re.search(r"(?:^|\.)(?:dev-|build-)?dependencies$", sect):
            continue
        m = re.match(r"^([A-Za-z0-9_\-]+)\s*=\s*(.*)$", s)
        if m:
            vm = re.match(r'^"([^"]*)"', m.group(2)) or re.search(r'version\s*=\s*"([^"]*)"', m.group(2))
            out[m.group(1)] = vm.group(1) if vm else "*"
    block = re.search(r"(?ms)^\s*dependencies\s*=\s*\[(.*?)\]", text or "")  # pyproject [project] dependencies = [...]
    if block:
        for m in re.finditer(r'"([A-Za-z0-9_.\-]+)\s*([^"]*)"', block.group(1)):
            out[m.group(1)] = m.group(2).strip() or "*"
    return out


def scope_deps_requirements(text):
    out = {}
    for line in (text or "").splitlines():
        if line.lstrip().startswith(("#", "-")):
            continue
        m = re.match(r"^\s*([A-Za-z0-9][A-Za-z0-9_.\-]*)(?:\[[^\]]*\])?\s*([=<>!~][^;#\s]*)?", line)
        if m:
            out[m.group(1)] = m.group(2) or "*"
    return out


def scope_deps_gomod(text):
    out, in_block = {}, False
    for line in (text or "").splitlines():
        s = line.split("//")[0].strip()
        if s.startswith("require ("):
            in_block = True
            continue
        if in_block and s == ")":
            in_block = False
            continue
        m = re.match(r"^(?:require\s+)?(\S+)\s+(v\S+)$", s) if (in_block or s.startswith("require ")) else None
        if m:
            out[m.group(1)] = m.group(2)
    return out


def scope_manifest_deps(name, text):
    """{dependency: version} of a known manifest, or None when name is not one."""
    if name == "package.json":
        return scope_deps_package_json(text)
    if name in ("Cargo.toml", "pyproject.toml"):
        return scope_deps_toml(text)
    if name == "go.mod":
        return scope_deps_gomod(text)
    if re.fullmatch(r"requirements[\w.\-]*\.txt", name):
        return scope_deps_requirements(text)
    return None


def pr_scope(run, st, pr):
    """Lines for the body's Scope section from local git only, in the PR's canonical worktree: areas touched (top-level
    folder, file count) and dependency changes. [] when there is no worktree or git says no: the section is left out."""
    wt = canon_wt(run, st, pr)
    base = (guard_rec(run).get("bases") or {}).get(pr.get("id")) or pr.get("baseCommit")
    if not wt or not isinstance(base, str) or not HEX_SHA.fullmatch(base):
        return []
    r = subprocess.run(["git", "-c", "core.fsmonitor=false", "-C", wt, "diff", "--name-only", "-z", "--no-renames", f"{base}..HEAD"],
                       capture_output=True)
    if r.returncode != 0:
        return []
    paths = sorted({x for x in r.stdout.decode("utf-8", "replace").split("\0") if x})
    if not paths:
        return []
    areas = {}
    for p in paths:
        k = p.split("/")[0] if "/" in p else "(root)"
        areas[k] = areas.get(k, 0) + 1
    lines = ["- Areas: " + ", ".join(f"{k} ({n} file{'s' if n != 1 else ''})" for k, n in sorted(areas.items(), key=lambda kv: (-kv[1], kv[0])))]

    def show(rev, path):
        o = subprocess.run(["git", "-c", "core.fsmonitor=false", "-C", wt, "show", f"{rev}:{path}"], capture_output=True)
        return o.stdout.decode("utf-8", "replace") if o.returncode == 0 else ""

    for p in paths:
        name = os.path.basename(p)
        if name in LOCK_FILES or name in OTHER_MANIFESTS:
            lines.append(f"- {p}: changed")
            continue
        if scope_manifest_deps(name, "") is None:
            continue
        old, new = scope_manifest_deps(name, show(base, p)), scope_manifest_deps(name, show("HEAD", p))
        added = [f"{k} {v}" for k, v in sorted(new.items()) if k not in old]
        removed = [k for k in sorted(old) if k not in new]
        bumped = [f"{k} {old[k]} → {v}" for k, v in sorted(new.items()) if k in old and old[k] != v]
        if added:
            lines.append(f"- {p}: added {', '.join(added)}")
        if removed:
            lines.append(f"- {p}: removed {', '.join(removed)}")
        if bumped:
            lines.append(f"- {p}: bumped {', '.join(bumped)}")
        if not (added or removed or bumped):
            lines.append(f"- {p}: changed")
    return lines


FINDING_RE = re.compile(r"^\s*(?:\d+[.)]|[-*])\s*\**(CRITICAL|HIGH|MEDIUM)\**\s*[—–-]+\s*(.+)$")


def review_findings(run, pid, rnd):
    """The MEDIUM+ findings of reviews/<pid>-round<rnd>.md as short one-line summaries."""
    text = read(os.path.join(run, "reviews", f"{pid}-round{rnd}.md")) or ""
    m = re.search(r"(?ms)^##\s*Findings\s*$(.*?)(?=^##\s|\Z)", text)
    out = []
    for line in (m.group(1) if m else "").splitlines():
        f = FINDING_RE.match(line)
        if f:
            s = re.split(r"\s[—–-]+\s*Fix:", f.group(2))[0]
            s = " ".join(s.replace("`", "").split())
            out.append(s if len(s) <= 120 else s[:119].rstrip() + "…")
    return out


def review_lines(run, pr):
    """Review section: rounds, final verdict and gate, and each MEDIUM+ finding a later round did not raise again."""
    rev = pr.get("review") or {}
    rnd = rev.get("round")
    if not isinstance(rnd, int) or rnd < 1:
        return []
    gate = read_gate(run, pr.get("id"))
    head = f"- {rnd} review round{'s' if rnd != 1 else ''}"
    if rev.get("verdict"):
        head += f", final verdict {rev['verdict']}"
    if gate:
        head += f", gate {gate['gate']}"
    lines = [head]
    for n in range(1, rnd):
        later = " ".join((read(os.path.join(run, "reviews", f"{pr.get('id')}-round{n + 1}.md")) or "").split())
        for s in review_findings(run, pr.get("id"), n):
            if s[:60] in later:
                continue  # raised again in the next round: that round did not resolve it
            lines.append(f"- round {n} blocked: {s}; fixed in round {n + 1}")
    return lines


def ticket_lines(run, plan, pr):
    """Tickets section: id, neutralised title (at most 120 characters) and url of the PR's stories' tickets; never bodies."""
    tj = load_json(os.path.join(run, "tickets.json"))
    tasks = tj.get("tasks") if isinstance(tj, dict) else tj
    if not isinstance(tasks, list):
        return []
    by = {}
    for t in tasks:
        if isinstance(t, dict):
            for k in ("id", "custom_id"):
                if isinstance(t.get(k), str) and t[k]:
                    by.setdefault(t[k].lower(), t)
    refs, story_refs = [], plan_story_tickets(plan)
    for s in (pr.get("stories") or {}).get("ids") or []:
        for r in story_refs.get(s, []):
            if r.lower() in by and by[r.lower()] not in refs:
                refs.append(by[r.lower()])
    out = []
    for t in refs:
        title = " ".join(str(t.get("name") or t.get("title") or "").split())
        title = title if len(title) <= 120 else title[:119].rstrip() + "…"
        url = t.get("url") if isinstance(t.get("url"), str) and t["url"].startswith("http") else None
        out.append(f"- {t.get('custom_id') or t.get('id')}: {title}".rstrip(": ") + (f" ({url})" if url else ""))
    return out


def verify_lines(run, st, pr):
    """Verification section: the check and the worktree HEAD, the app's own re-run of it, and the dependency PRs."""
    chk = pr.get("check") or {}
    prs = {p.get("id"): p for p in st.get("prs") or []}
    wt = canon_wt(run, st, pr)
    head = git("-C", wt, "rev-parse", "--short=12", "HEAD").stdout.strip() if wt else ""
    out = []
    if chk.get("command") and chk.get("result"):
        out.append(f"- Check `{chk['command']}`: {chk['result']}" + (f" (worktree HEAD {head})" if head else ""))
    ac = load_json(os.path.join(run, "app-checks.json"))
    row = (ac.get("checks") or {}).get(pr.get("id")) if isinstance(ac, dict) and isinstance(ac.get("checks"), dict) else None
    if isinstance(row, dict) and row.get("result"):
        out.append(f"- App check: {row['result']}" + (f" at {str(row['head'])[:12]}" if row.get("head") else ""))
    for dep in pr.get("dependsOn") or []:
        d = prs.get(dep) or {}
        out.append(f"- Depends on: {d.get('prUrl') or dep + ' (branch ' + str(d.get('branch')) + ')'}")
    return out


def screen_lines(run, st, pr, sections):
    """sections: [(title, [lines])]. Every line is neutralised and secret-scanned and a line that looks like a secret is dropped
    (logged without its text). One scan covers everything and a bisect runs only on a hit, so a clean body costs one scan.
    A scanner that cannot run keeps the lines: the whole-body scan before anything is sent then refuses. Returns {title: lines}."""
    flat = [(i, neutral(l)) for i, (_, ls) in enumerate(sections) for l in ls if l.strip()]
    dropped = []

    def go(items):
        if not items:
            return []
        clean, _ = scan_secrets("\n".join(l for _, l in items))
        if clean is not False:
            return items
        if len(items) == 1:
            dropped.append(items[0][0])
            return []
        mid = len(items) // 2
        return go(items[:mid]) + go(items[mid:])

    kept = go(flat)
    for i in sorted(set(dropped)):
        try:
            append_log(run, st.get("stage") or "shipping", f"{pr.get('id')} PR body: dropped a line from {sections[i][0]} (looked like a secret)")
        except OSError:
            pass
    return {t: [l for i, l in kept if i == n] for n, (t, _) in enumerate(sections)}


def build_body(run, st, pr, mode):
    """mode 'draft' = what is known after build round 1, 'final' = every section. AD-9 order: Why, What it adds, Scope, Review,
    Verification, Tickets, then the stack / epic lines and the Plan line. A section with no data is left out."""
    plan = read(os.path.join(run, "plan.md")) or ""
    why = (plan_blocks(plan)[1].get(pr.get("id")) or {}).get("why")
    final = mode == "final"
    shown = screen_lines(run, st, pr, [
        ("Why", [why] if why else []), ("Scope", pr_scope(run, st, pr)),
        ("Review", review_lines(run, pr) if final else []), ("Verification", verify_lines(run, st, pr)),
        ("Tickets", ticket_lines(run, plan, pr) if final else [])])
    lines = []

    def section(title, body):
        if body:
            lines.extend([f"## {title}", ""] + body + [""])

    section("Why", shown["Why"])
    section("What it adds", story_lines(run, pr))
    section("Scope", shown["Scope"])
    section("Review", shown["Review"])
    rounds = sorted(pr.get("comments") or [], key=lambda c: c.get("round", 0)) if final else []
    section("Eagle-Eye review rounds", [f"- Round {c['round']}: {c['url']}" + (f" (fixes: {c['fixesUrl']})" if c.get("fixesUrl") else "") for c in rounds])
    section("Verification", shown["Verification"])
    section("Tickets", shown["Tickets"])
    stack = stack_epic_lines(st, pr)
    lines += stack[1:] if stack and stack[0] == "" else stack
    if lines and lines[-1] != "":
        lines.append("")
    if final:
        lines += [f"Plan: Eagle-Eye run {st.get('runId')}", ""]
    else:
        lines += [f"\U0001F916 Eagle-Eye run {st.get('runId')} — review in progress. Each review round is posted as a comment; "
                  "this PR is marked ready for review only after the run's ship approval.", ""]
    return "\n".join(lines)


def pr_body(run, st, pr):
    return build_body(run, st, pr, "final")


def cmd_pr_body(run, pid, draft):
    st = load_status(run)
    pr = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pid), None)
    if pr is None:
        die(f"no {pid} in status.json")
    print(build_body(run, st, pr, "draft" if draft else "final"), end="")


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
        if state == "opened":
            for n in epic_links(run, st, pr, rem_now[0]):
                print(n)

    rem_now = [None]
    if pr.get("state") not in ("reviewed", "pushed"):
        die(f"NOT SHIPPED: {pid} is {pr.get('state')}; only reviewed PRs ship")
    # The same protections as github mode: the canonical branch and worktree recorded in .guard.json, no copied file in the
    # commits, a push to the URL recorded when the worktree was made (no hooks, no remote name), `gh -R <owner/name>`.
    pr, rem = canonical_pr(run, st, pid, "NOT SHIPPED")
    rem_now[0] = rem
    parent_note = need_parent_pr(st, pr, "NOT SHIPPED")
    wt, branch, base = pr["worktree"], pr["branch"], pr.get("base")
    if not rem:
        return save("local-only", "no git remote: nothing pushed, no PR opened")
    if parent_note:
        return save("needs-attention", parent_note)
    url = rem["url"]
    why = url_problem(url)
    if why:
        return save("needs-attention", f"remote {url} is not an owner/name https or ssh URL: nothing pushed; "
                    f"push {branch} by hand and open the PR")
    if pr["state"] == "reviewed":
        no_copied(run, st, pr, "NOT SHIPPED")
    slug = rem.get("repo")
    if not slug:  # e.g. a local mirror (test suites): ask gh which repo this is
        q = run_cmd(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"], cwd=wt, timeout=60)
        slug = q.stdout.strip() if q.returncode == 0 and re.fullmatch(r"[\w.-]+/[\w.-]+", q.stdout.strip()) else None
    if not slug:
        return save("needs-attention", f"cannot tell which GitHub repo {url} is: nothing pushed; push {branch} by hand and open the PR")
    # The body is scanned before anything is pushed (epic titles and links are ticket data), so a hit really sent nothing.
    clean_text(run, st, pr, pr_body(run, st, pr), "the PR body", "NOT SHIPPED")
    if pr["state"] == "reviewed":
        push_parent(run, st, pr, rem, "NOT SHIPPED")
        if not remote_sha(url, base):
            return save("needs-attention", f"base {base} is not on {url}")
        r = push_branch(wt, url, branch)
        if r.returncode != 0:
            return save("needs-attention", "push refused: " + first_line(r.stderr)[:200])
        save("pushed", "pushed; opening the PR")
    v = gh(["pr", "view", branch, "--json", "url", "-q", ".url"], wt, slug, 60)
    if v.returncode == 0 and v.stdout.strip().startswith("http"):
        return save("opened", None, v.stdout.strip())
    body = os.path.join(run, "pr-bodies", f"{pid}.md")
    os.makedirs(os.path.dirname(body), exist_ok=True)
    text = pr_body(run, st, pr)
    clean_text(run, st, pr, text, "the PR body", "NOT SHIPPED")
    atomic_write(body, text)
    title = str(pr.get("title") or pid)
    c = gh(["pr", "create", "--base", base, "--head", branch, "--title", title, "--body-file", body], wt, slug)
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


# Network git: only https / ssh (and local folders in the test suites only) transports, no hooks, and the URL after
# --end-of-options, so a recorded URL can never be an option (`--upload-pack=…`) or an ext:: helper. The system git
# config is kept: it is where e.g. Homebrew git keeps the credential helper (osxkeychain) https pushes need.
NET = ["-c", "protocol.allow=never", "-c", "protocol.https.allow=always", "-c", "protocol.ssh.allow=always",
       "-c", "protocol.file.allow=always", "-c", "core.hooksPath=/dev/null"]


def url_problem(url):
    """None when the run may talk to this remote URL: an owner/name https or ssh URL (SLUG); an existing absolute
    folder only in the test suites (DCV2_TEST_LOCAL_REMOTES=1: a local bare repo runs its own hooks on a push)."""
    url = str(url or "")
    if not url or url.startswith("-") or any(c in url for c in "\n\r\0 ") or "::" in url:
        return "refused: not a safe remote URL"
    if url.startswith("http://"):
        return "refused: http:// remotes are not used (use https://)"
    if SLUG.match(url):
        return None
    if url.startswith("/") and os.path.isdir(url) and os.environ.get("DCV2_TEST_LOCAL_REMOTES") == "1":
        return None
    return "refused: not a safe remote URL (only https / ssh GitHub-style remotes)"


def net_env():
    return dict(os.environ, GIT_TERMINAL_PROMPT="0")


def net_git(url, args, cwd=None):
    why = url_problem(url)
    if why:
        return subprocess.CompletedProcess(args, 128, "", why)
    env = net_env()
    neutral = cwd or tempfile.mkdtemp(prefix="dcv2-net-")
    try:
        return subprocess.run(["git", *NET, *args], capture_output=True, text=True, cwd=neutral, env=env, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as e:
        return subprocess.CompletedProcess(args, 128, "", str(e))
    finally:
        if not cwd:
            shutil.rmtree(neutral, ignore_errors=True)


def remote_tip(url, branch):
    """(sha or None when the branch is not there, error or None). Run from a neutral folder: no repo config applies."""
    r = net_git(url, ["ls-remote", "--heads", "--end-of-options", url, f"refs/heads/{branch}"])
    if r.returncode != 0:
        return None, first_line(r.stderr) or "ls-remote failed"
    return (r.stdout.split() or [None])[0], None


def remote_sha(url, branch):
    return remote_tip(url, branch)[0]


def push_branch(wt, url, branch):
    """Push the branch to the recorded URL: never force, no hooks."""
    return net_git(url, ["-C", wt, "push", "--end-of-options", url, f"refs/heads/{branch}:refs/heads/{branch}"], cwd=wt)


def delete_remote_branch(url, branch, oid):
    """Delete the remote branch only while it is still at oid (a lease, never a plain force), from an empty repo."""
    tmp = tempfile.mkdtemp(prefix="dcv2-del-")
    try:
        subprocess.run(["git", "init", "-q", tmp], capture_output=True)
        return net_git(url, ["-C", tmp, "push", f"--force-with-lease=refs/heads/{branch}:{oid}", "--end-of-options", url,
                        f":refs/heads/{branch}"], cwd=tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


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
    pr, rem = canonical_pr(run, st, pid)
    return st, pr, rem


def canonical_pr(run, st, pid, what="NOT DONE"):
    """(the PR entry, its recorded remote or None) for a push / PR write, local or github mode: the PR must be on its
    canonical branch in its canonical worktree (made by state.sh worktree) and recorded in .guard.json. status.json's
    own worktree / branch fields are never trusted on their own."""
    pr = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pid), None)
    if pr is None:
        die(f"{what}: no {pid} in status.json")
    branch, wt = canon(pr.get("repoPath") or "/", st.get("runId"), pid)
    if pr.get("branch") != branch or os.path.realpath(str(pr.get("worktree") or "")) != wt or not os.path.isdir(wt):
        die(f"{what}: {pid} must be on {branch} in its worktree {wt} (made by state.sh worktree)")
    if git("-C", wt, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() != branch:
        die(f"{what}: {wt} is not on {branch}")
    g = guard_rec(run)
    if (g.get("worktrees") or {}).get(pid) != wt:
        die(f"{what}: {pid}'s worktree is not recorded in .guard.json (run `state.sh worktree` for it)")
    if pid not in (g.get("remotes") or {}):
        # A worktree made before 0.3.0 has no recorded remote. A github-mode run is always a 0.3.0 run (the app
        # writes reviewMode only then), whose worktrees were recorded when made; this fallback records the
        # repo's current remote once, and from then on the guard keeps the Implementer from changing remotes.
        r = remote_record(run, os.path.realpath(pr["repoPath"]))
        if r:
            record_guard(run, os.path.realpath(pr["repoPath"]), pid, wt, remote=r)
            g = guard_rec(run)
    rem = (g.get("remotes") or {}).get(pid)
    if rem and not (isinstance(rem, dict) and rem.get("url")):
        rem = None
    return pr, rem


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
    g = guard_rec(run)
    files = (g.get("copied") or {}).get(pr.get("id")) or []
    if not files:
        return []
    base = (g.get("bases") or {}).get(pr.get("id"))  # recorded by state.sh worktree, not status.json's copy
    if not base:
        return files  # cannot tell what the branch adds: treat every copied file as at risk
    # Every commit, not the net diff: a file added and later deleted is still in the pushed history.
    r = git("-C", pr["worktree"], "log", "--name-only", "--format=", f"{base}..HEAD")
    if r.returncode != 0:
        return files
    changed = set(l.strip() for l in r.stdout.splitlines())
    return [f for f in files if f in changed]


def no_copied(run, st, pr, what):
    bad = copied_committed(run, pr)
    if bad:
        attention(run, st, pr, f"the branch commits {', '.join(bad)}, copied in from the main checkout (copyFiles); remove it from the commits", what)


def clean_text(run, st, pr, text, label, what, soft=False):
    """Secret scan before anything is sent; a hit or a scanner that cannot run refuses (a hit: needs-attention).
    soft: return the reason instead (None when clean) and change no state: for the sibling-PR edits, which only give notes."""
    clean, why = scan_secrets(text)
    if clean is None:
        if soft:
            return f"{label} not sent: {why}"
        die(f"{what}: {why}")
    if not clean:
        if soft:
            return f"{label} not sent: {why}"
        attention(run, st, pr, f"{label} not sent to GitHub: {why}", what)
    return None


REMOTE_HOST = re.compile(r"^(?:https?://(?:[^@/]+@)?|ssh://(?:[^@/]+@)?|[\w.-]+@)([^/:\s]+)")


def verify_pr(pr, rem, url):
    """None when url is this branch's own PR in the recorded repo (on github.com or the remote's host), else why not."""
    m = GH_URL.match(str(url or ""))
    slug = rem.get("repo") or ""
    hm = REMOTE_HOST.match(rem.get("url") or "")
    hosts = {"github.com"} | ({hm.group(1).lower()} if hm else set())
    host = str(url).split("/")[2].lower() if m else ""
    if not m or f"{m.group(1)}/{m.group(2)}".lower() != slug.lower() or host not in hosts:
        return f"{url} is not a pull request of {slug or 'the recorded repo'} on {' / '.join(sorted(hosts))}"
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
    return build_body(run, st, pr, "draft")


def final_body(run, st, pr):
    return build_body(run, st, pr, "final")


def title_and_body(run, st, pr, body, what):
    """Neutralised, capped, secret-scanned title + body; the body is written to pr-bodies/<PR>.md."""
    title = capped(neutral(" ".join(str(pr.get("title") or pr["id"]).split())), "", TITLE_CAP).replace("\n", " ")
    body = capped(neutral(body), "The full text is in the run folder.")
    clean_text(run, st, pr, title + "\n" + body, "the PR title/body", what)
    f = os.path.join(run, "pr-bodies", f"{pr['id']}.md")
    os.makedirs(os.path.dirname(f), exist_ok=True)
    atomic_write(f, body)
    return title, f


def need_parent_pr(st, pr, what):
    """A stacked PR opens against its parent's branch, so the parent's PR must exist first: refuse otherwise."""
    so = pr.get("stackedOn")
    if not so:
        return
    parent = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == so), None)
    if parent and not parent.get("prUrl") and parent.get("state") in ("local-only", "pushed", "needs-attention"):
        return (f"{so} ended {parent.get('state')} with no PR, so this stacked PR was not pushed or opened and is left "
                f"needs-attention: open {so}'s PR by hand first, then push {pr.get('branch')} and open its PR against {so}'s branch")
    if not parent or not parent.get("prUrl"):
        die(f"{what}: {pr.get('id')} is stacked on {so}, which has no PR yet: open {so} first (it ships before {pr.get('id')})")
    return None


def push_parent(run, st, pr, rem, what):
    """Push the stack parent's branch first (never force) when the remote does not have its head: from the parent's worktree
    head or recorded head, since its local branch may be gone."""
    so = pr.get("stackedOn")
    if not so or not rem:
        return
    parent = next(p for p in st["prs"] if isinstance(p, dict) and p.get("id") == so)
    head, why = parent_head(run, st, parent)
    if not head:
        die(f"{what}: {why}")
    pbranch = canon(parent.get("repoPath") or "/", st.get("runId"), so)[0]
    if remote_sha(rem["url"], pbranch) == head:
        return
    r = net_git(rem["url"], ["-C", pr["worktree"], "push", "--end-of-options", rem["url"], f"{head}:refs/heads/{pbranch}"], cwd=pr["worktree"])
    if r.returncode != 0:
        die(f"{what}: {so}'s branch {pbranch} could not be pushed first: {first_line(r.stderr)[:200]}")


def soft(fn, *args):
    """Run a function that may die(): (True, None) or (False, its message); no state changes are made here."""
    buf = io.StringIO()
    try:
        with contextlib.redirect_stderr(buf):
            fn(*args)
        return True, None
    except SystemExit:
        return False, " ".join(buf.getvalue().split())[:200] or "refused"


def rem_with_slug(rem, cwd):
    """The recorded remote with its owner/name filled in: a local-mode run (or a local mirror) did not record it, so ask gh."""
    if not rem or rem.get("repo"):
        return rem
    q = run_cmd(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"], cwd=cwd, timeout=60)
    v = q.stdout.strip()
    return {**rem, "repo": v} if q.returncode == 0 and re.fullmatch(r"[\w.-]+/[\w.-]+", v) else rem


def epic_links(run, st, pr, rem):
    """After a PR with an epic is open: the epic-<id> label on it (created in the repo only if missing) and, when the epic has
    2+ PRs, the already-open siblings' bodies updated once. Every GitHub write goes through checked_pr and clean_text; a failure
    is a note (returned), never a state change."""
    notes = []
    ep = pr.get("epic")
    if not (isinstance(ep, dict) and ep.get("id")):
        return notes
    sibs = epic_siblings(st, pr)
    rem = rem_with_slug(rem, pr.get("worktree"))
    if not pr.get("prUrl") or not rem or not rem.get("repo"):
        return notes
    label = f"epic-{ep['id']}"
    if re.fullmatch(r"epic-[A-Za-z0-9_-]{1,50}", label):
        ok, why = soft(checked_pr, run, st, pr, rem, "epic label")
        if not ok:
            notes.append(f"note: label {label} not applied: {why}")
        else:
            ls = gh(["label", "list", "--search", label, "--json", "name", "--limit", "100"], pr["worktree"], rem["repo"], 60)
            try:
                have = {x.get("name") for x in json.loads(ls.stdout or "[]") if isinstance(x, dict)} if ls.returncode == 0 else None
            except ValueError:
                have = None
            if have is None:
                notes.append("note: " + gh_err(ls, f"label {label} not applied: gh label list"))
            else:
                if label not in have:
                    c = gh(["label", "create", label, "--color", "ededed", "--description", f"Eagle-Eye epic {ep['id']}"], pr["worktree"], rem["repo"], 60)
                    if c.returncode != 0:
                        notes.append("note: " + gh_err(c, f"label {label} not created: gh label create"))
                        have = None
                if have is not None:
                    e = gh(["pr", "edit", pr["prUrl"], "--add-label", label], pr["worktree"], rem["repo"], 60)
                    if e.returncode != 0:
                        notes.append("note: " + gh_err(e, f"label {label} not applied: gh pr edit"))
    else:
        notes.append(f"note: epic id {ep['id']!r} is not a safe label name: no label")
    for sib in sibs:
        if not sib.get("prUrl") or sib.get("state") not in ("opened", "drafted", "pushed"):
            continue

        def edit(sib=sib):
            sp, srem = canonical_pr(run, st, sib["id"], "sibling body")
            srem = rem_with_slug(srem, sp["worktree"])
            checked_pr(run, st, sp, srem, "sibling body")
            body = capped(neutral(final_body(run, st, sp) if sp.get("state") == "opened" else draft_body(run, st, sp)),
                          "The full text is in the run folder.")
            bad = clean_text(run, st, sp, body, f"{sib['id']}'s body", "sibling body", soft=True)
            if bad:
                die(bad)
            f = os.path.join(run, "pr-bodies", f"{sib['id']}.md")
            os.makedirs(os.path.dirname(f), exist_ok=True)
            atomic_write(f, body)
            r = gh(["pr", "edit", sp["prUrl"], "--body-file", f], sp["worktree"], srem["repo"])
            if r.returncode != 0:
                die(gh_err(r, "gh pr edit"))

        ok, why = soft(edit)
        if not ok:
            notes.append(f"note: {sib['id']}'s body not updated with the epic links: {why}")
    for n in notes:
        try:
            append_log(run, st.get("stage") or "shipping", f"{pr['id']} {n}")
        except OSError:
            pass
    return notes


def cmd_draft_pr(run, pid):
    st, pr, rem = github_pr(run, pid, ("building", "ship-approval"))
    if pr.get("prUrl"):
        print(f"draft=exists url={pr['prUrl']}")
        return
    pn = need_parent_pr(st, pr, "NOT DRAFTED")
    if pn:
        die(f"NOT DRAFTED: {pn}")
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
    push_parent(run, st, pr, rem, "NOT DRAFTED")
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
    for n in epic_links(run, st, pr, rem):
        print(n)


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
    hit = next((c for c in cs if isinstance(c, dict) and str(c.get("body") or "").rstrip().endswith(footer)), None)
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
    rk = pr.get("restack") if type(pr.get("restack")) is int else 0
    old = next((c for c in comments if c.get("round") == rnd and (c.get("restack") or 0) == rk), None)
    if old:
        print(f"posted=already url={old.get('url')}")
        return
    rel = f"reviews/{pid}-round{rnd}.md"
    text = read(os.path.join(run, rel)).strip()
    if not text:
        die(f"NOT POSTED: {rel} is missing or empty")
    checked_pr(run, st, pr, rem, "NOT POSTED")
    footer = f"_Eagle-Eye run {st.get('runId')}, review round {rnd}" + (f" (after restack {rk})" if rk else "") + "._"
    head = git("-C", pr["worktree"], "rev-parse", "HEAD").stdout.strip()
    url = posted_already(pr, rem, footer)
    how = "found"
    if not url:
        body = capped(neutral(text), f"The full review is in the run folder: {rel}.") + f"\n\n{footer}\n"
        clean_text(run, st, pr, body, f"review round {rnd}", "NOT POSTED")
        url, err = post_comment(run, st, pr, rem, body, f"{pid}-restack{rk}-round{rnd}" if rk else f"{pid}-round{rnd}")
        if err:
            print(f"posted=failed reason={err}")
            return
        how = "ok"
    pr["comments"] = comments + [{"round": rnd, "url": url, "head": head, **({"restack": rk} if rk else {})}]
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
    rk = pr.get("restack") if type(pr.get("restack")) is int else 0
    entry = next((c for c in pr.get("comments") or [] if c.get("round") == rnd and (c.get("restack") or 0) == rk), None)
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
    footer = f"_Eagle-Eye run {st.get('runId')}, round {rnd} fixes" + (f" (after restack {rk})" if rk else "") + "._"
    url, how = posted_already(pr, rem, footer), "found"
    if not url:
        clean, _ = scan_secrets("\n".join(commits))
        items = [f"- `{c.split(' ', 1)[0]}` {c.split(' ', 1)[1] if ' ' in c and clean else ''}".rstrip() for c in commits]
        body = capped(neutral(f"Round {rnd} fixes: {len(commits)} commit(s)\n\n" + "\n".join(items)), "See the branch for the rest.") + \
            f"\n\nReview round {rnd + 1} follows.\n\n{footer}\n"
        clean_text(run, st, pr, body, f"round {rnd} fixes comment", "NOT POSTED")
        url, err = post_comment(run, st, pr, rem, body, f"{pid}-restack{rk}-round{rnd}-fixes" if rk else f"{pid}-round{rnd}-fixes")
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
    parent_note = need_parent_pr(st, pr, "NOT READY")

    def save(state, note=None, url=None):
        pr["state"], pr["note"] = state, note
        if url:
            pr["prUrl"] = url
        save_pr(run, st, pr, f"{pid} {state}" + (f": {url}" if url else "") + (f" ({note})" if note else ""))
        if state == "opened":
            for n in epic_links(run, st, pr, rem):
                print(n)

    wt, branch, base = pr["worktree"], pr["branch"], pr.get("base")
    if not rem:
        return save("local-only", "no git remote: nothing pushed, no PR opened")
    if parent_note:
        return save("needs-attention", parent_note)
    if not rem.get("repo"):
        return save("needs-attention", f"cannot tell which GitHub repo {rem.get('url')} is")
    push_parent(run, st, pr, rem, "NOT READY")
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
        # A ticket no story covers gets no comment (nothing to say on it, and nothing is auto-posted).
        comment = ("Eagle-Eye run " + str(st.get("runId", "")) + "\n\nDone:\n" + "\n".join(done) + "\n\nPull requests:\n"
                   + "\n".join(lines or ["- none"])) if stories else None
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


def pr_owned_files(run, st):
    """{PR id: [paths]} the approved Files of each PR (plan.prFiles), else the live plan.md's."""
    frozen = (st.get("plan") or {}).get("prFiles") if isinstance(st.get("plan"), dict) else None
    if isinstance(frozen, dict):
        return frozen
    stories, prs = plan_blocks(read(os.path.join(run, "plan.md")) or "")
    return {pid: sorted({f for s in p["stories"] for f in stories.get(s, [])}) for pid, p in prs.items()}


class DiffError(Exception):
    pass


def diff_paths(run, st, pid):
    """The paths the PR's branch changes against its base (.guard.json bases[pid], else baseCommit). Raises DiffError (with
    the reason) when the worktree, the branch or the base is not there."""
    pr = next((p for p in st.get("prs") or [] if isinstance(p, dict) and p.get("id") == pid), None)
    if pr is None:
        raise DiffError(f"no {pid} in status.json")
    if not pr.get("repoPath") or not os.path.isabs(str(pr.get("repoPath"))):
        raise DiffError(f"{pid} has no repoPath")
    repo = os.path.realpath(pr["repoPath"])
    branch, wt = canon(repo, st.get("runId"), pid)
    if pr.get("branch") != branch or os.path.realpath(str(pr.get("worktree") or "")) != wt or not os.path.isdir(wt):
        raise DiffError(f"{pid} is not on {branch} in its worktree {wt} (made by state.sh worktree); the worktree is missing")
    if git("-C", wt, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() != branch:
        raise DiffError(f"{wt} is not on {branch}")
    g = guard_rec(run)
    if (g.get("worktrees") or {}).get(pid) != wt:
        raise DiffError(f"{pid}'s worktree is not recorded in .guard.json (run `state.sh worktree` for it)")
    base = (g.get("bases") or {}).get(pid) or pr.get("baseCommit")
    if not base:
        raise DiffError(f"{pid} has no base commit")
    r = subprocess.run(["git", "-C", wt, "diff", "--name-only", "-z", "-M", f"{base}..HEAD"], capture_output=True)
    if r.returncode != 0:
        raise DiffError("git diff failed: " + first_line(r.stderr.decode("utf-8", "replace")))
    return sorted({x for x in r.stdout.decode("utf-8", "surrogateescape").split("\0") if x})


def stack_family(run, st, pid):
    """The ids of pid's stack ancestors and descendants: prs[].stackedOn, and the plan's same-repo Depends on (a child has
    no stackedOn until its worktree is made, which is after its parent's gate)."""
    parent = {p.get("id"): p.get("stackedOn") for p in st.get("prs") or [] if isinstance(p, dict)}
    _, plan_prs = plan_blocks(read(os.path.join(run, "plan.md")) or "")
    for q in plan_prs:
        deps = same_repo_deps(plan_prs, q)
        if len(deps) == 1 and not parent.get(q):
            parent[q] = deps[0]

    def chain(x):
        out, y = [], parent.get(x)
        while isinstance(y, str) and y and y not in out and y != x:
            out.append(y)
            y = parent.get(y)
        return out

    return set(chain(pid)) | {q for q in parent if q != pid and pid in chain(q)}


def gate_overlaps(run, st, pid):
    """[(path, other PR id)]: paths the PR changes that another PR of the same repo owns (plan.prFiles), leaving out the PR's
    stack ancestors and descendants (a stacked child may change what its parent changed). Raises DiffError."""
    changed = diff_paths(run, st, pid)
    me = next(p for p in st["prs"] if p.get("id") == pid)
    repo = os.path.realpath(str(me.get("repoPath") or "/"))
    owned, skip = pr_owned_files(run, st), stack_family(run, st, pid)
    out = []
    for oid, other in sorted((p.get("id"), p) for p in st.get("prs") or [] if isinstance(p, dict)):
        if oid == pid or oid in skip or os.path.realpath(str(other.get("repoPath") or "/")) != repo:
            continue
        for path in changed:
            if any(paths_overlap(path, f) for f in owned.get(oid) or []):
                out.append((path, oid))
    return out


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
        if isinstance(st.get("plan"), dict) and isinstance(st["plan"].get("prFiles"), dict):  # a plan approved under the 0.7.0+ rules
            try:
                overlaps = gate_overlaps(run, st, pid)
            except DiffError as e:
                die(f"NOT WRITTEN: {gate} for {pid} needs its diff checked against the other PRs' files, and it cannot be: {e}. "
                    f"Fix that (state.sh worktree re-makes a missing worktree from its branch), then write the gate again")
            if overlaps:
                die(f"NOT WRITTEN: {pid} changes paths another PR of the same repo owns: "
                    + "; ".join(f"{path} ({oid})" for path, oid in overlaps) + ". Write FAIL, or WAIVED with the user's reason")
        else:
            print("overlap: not checked (plan approved before 0.7.0)")
    if gate == "WAIVED":
        if not old or old.get("gate") not in ("CONCERNS", "FAIL"):
            die(f"NOT WRITTEN: WAIVED only replaces a CONCERNS or FAIL gate (now {(old or {}).get('gate') or 'missing'}), "
                "and only when the user said so, with their reason")
    os.makedirs(os.path.join(run, "gates"), exist_ok=True)
    rs = pr.get("restack") if type(pr.get("restack")) is int else 0
    body = (f"gate: {gate}\nreason: {json.dumps(reason)}\nupdated: {now()}\nreview: {json.dumps(review)}\nround: {rnd}\nrestack: {rs}\n"
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
    rk = pl.get("risk") if isinstance(pl.get("risk"), dict) else None
    L += [f"- Plan: {pl.get('reviewRound', 0)} review round(s), last verdict {pl.get('verdict') or '-'}, "
          f"{pl.get('openFindings', 0)} MEDIUM+ open; approved by {appr.get('by') or '-'}" + (f" at {appr['at']}" if appr.get("at") else ""),
          *([f"- Risk: {rk.get('round', 0)} review round(s), last verdict {rk.get('verdict') or '-'}, {rk.get('openFindings', 0)} MEDIUM+ open"
             + (f" (`{rk['path']}`)" if rk.get("path") else "")] if rk else []),
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
    # What the run could not do itself (the skill logs "not done: <what> (<why>)" instead of handing the user a command).
    nd = [l.split(" ", 2)[2] for l in log if len(l.split(" ", 2)) == 3 and l.split(" ", 2)[2].lower().startswith("not done:")]
    if nd:
        L += ["## Not done by the run", ""] + [f"- {x[len('not done:'):].strip()}" for x in nd] + [""]
    L += ["## Events", "", f"{len(log)} log lines; retries {sum(1 for l in log if 'retried' in l)}; "
          f"needs-attention {sum(1 for p in prs if p.get('state') == 'needs-attention')}. Full timeline: `log.md`.", ""]
    atomic_write(os.path.join(run, "report.md"), "\n".join(L))


def cmd_report(run):
    if not isinstance(load_json(os.path.join(run, "status.json")), dict):
        die(f"no valid status.json in {run}")
    write_report(run)
    print(f"wrote {os.path.join(run, 'report.md')}")


# ---- cleanup and app requests (AFK runs, plan 2026-10-27) -------------------------------------
# `cleanup` does what the app's janitor and its merged-run cleanup do, for this run only, with the same rules:
# only the run's own worktrees (where the contract puts them, a linked worktree, on the run's branch), never
# --force (a dirty one, or one with ignored files other than build output and the copied-in files, is kept),
# `git branch -d` only, and a remote branch delete only for a merged PR whose head is the run's branch and
# whose remote tip is the PR's headRefOid, pushed to the remote URL recorded when the worktree was made.

DISPOSABLE = ("node_modules", "target", "dist", "build", ".next", "coverage", ".turbo", "__pycache__", ".venv",
              ".pytest_cache", ".gradle")
REQUEST_WAIT_DEFAULT, REQUEST_WAIT_MAX = 90, 600


def disposable(path):
    comps = path.rstrip("/").split("/")
    dirs = comps if path.endswith("/") else comps[:-1]
    return any(c in DISPOSABLE for c in dirs)


def verify_wt(run_id, pid, repo, wt):
    """None when wt is this PR's own linked worktree of repo on its branch, else why not."""
    branch, want = canon(repo, run_id, pid)
    if os.path.realpath(wt) != want:
        return f"not where this run's worktrees go ({want})"
    gd = git("-C", wt, "rev-parse", "--absolute-git-dir").stdout.strip()
    common = git("-C", wt, "rev-parse", "--path-format=absolute", "--git-common-dir").stdout.strip()
    if not gd or not common or os.path.realpath(gd) == os.path.realpath(common):
        return "not a linked worktree"
    # Same git common dir as the recorded repo: right for a repo that is itself a linked worktree, a submodule
    # (common dir under the superproject's .git/modules/) and a --separate-git-dir repo, where the common dir's parent
    # is not the checkout.
    repo_common = git("-C", repo, "rev-parse", "--path-format=absolute", "--git-common-dir").stdout.strip()
    if not repo_common or same_path(common, "") or not same_path(common, repo_common):
        return "not a worktree of its recorded repo"
    head = git("-C", wt, "symbolic-ref", "--quiet", "--short", "HEAD").stdout.strip()
    if head != branch:
        return f"on {head or 'a detached HEAD'}, not {branch}"
    return None


def kept_files(wt, copied):
    """(changed count, kept ignored files) or (None, why)."""
    r = git("-C", wt, "status", "--porcelain", "--ignored", "--untracked-files=normal")
    if r.returncode != 0:
        return None, first_line(r.stderr) or "git status failed"
    changed, kept = 0, []
    for line in r.stdout.splitlines():
        if len(line) <= 3:
            continue
        path = line[3:].strip('"')
        if line.startswith("!!"):
            if not disposable(path) and path not in copied:
                kept.append(path)
        else:
            changed += 1
    return changed, kept


def merged_head(pr, rem, cwd, branch):
    """(headRefOid, None, baseRefName) for a merged PR of this run's branch in the recorded repo, else (None, why, None).
    The base is GitHub's own: in a stack it is the branch the PR was retargeted to, not the base recorded at ship time."""
    url = pr.get("prUrl")
    if not url:
        return None, "no PR", None
    if not rem or not rem.get("url") or not rem.get("repo"):
        return None, "no recorded GitHub remote", None
    m = GH_URL.match(str(url))
    hm = REMOTE_HOST.match(rem.get("url") or "")
    hosts = {"github.com"} | ({hm.group(1).lower()} if hm else set())
    if not m or f"{m.group(1)}/{m.group(2)}".lower() != rem["repo"].lower() or url.split("/")[2].lower() not in hosts:
        return None, f"{url} is not a pull request of {rem['repo']}", None
    r = gh(["pr", "view", url, "--json", "state,mergeCommit,headRefName,headRefOid,baseRefName"], cwd, rem["repo"], 60)
    if r.returncode != 0:
        return None, gh_err(r, "gh pr view"), None
    try:
        v = json.loads(r.stdout or "{}")
    except ValueError:
        return None, "gh pr view printed no JSON", None
    if not isinstance(v, dict):
        return None, "gh pr view printed no JSON object", None
    if v.get("headRefName") != branch:
        return None, f"the PR's head is {v.get('headRefName')}, not {branch}", None
    if v.get("state") != "MERGED":
        return None, f"not merged ({str(v.get('state') or 'unknown').lower()})", None
    oid = str(v.get("headRefOid") or "")
    base = v.get("baseRefName") if isinstance(v.get("baseRefName"), str) and v.get("baseRefName") else None
    return (oid, None, base) if re.fullmatch(r"[0-9a-f]{40}", oid) else (None, "gh pr view gave no head commit", None)


def retarget_children(rem, cwd, branch, new_base):
    """None when no open PR uses `branch` as its base: those that did were retargeted to new_base and checked. Else why not
    (the caller then keeps the branch)."""
    slug = rem["repo"]
    r = gh(["pr", "list", "--base", branch, "--state", "open", "--json", "url,baseRefName", "--limit", "100"], cwd, slug, 60)
    if r.returncode != 0:
        return gh_err(r, "listing the PRs stacked on it: gh pr list --base")
    try:
        rows = json.loads(r.stdout or "[]")
    except ValueError:
        return "gh pr list printed no JSON"
    hm = REMOTE_HOST.match(rem.get("url") or "")
    hosts = {"github.com"} | ({hm.group(1).lower()} if hm else set())
    rows = rows if isinstance(rows, list) else []
    if rows and (not new_base or not re.fullmatch(r"[\w][\w./-]{0,200}", str(new_base)) or ".." in str(new_base) or str(new_base).endswith(("/", ".lock"))):
        return f"no usable base recorded to retarget to ({new_base!r})"
    for row in rows:
        url = str(row.get("url") or "") if isinstance(row, dict) else ""
        m = GH_URL.match(url)
        if not m or f"{m.group(1)}/{m.group(2)}".lower() != slug.lower() or url.split("/")[2].lower() not in hosts:
            return f"{url or 'a PR'} stacked on it is not a pull request of {slug}"
        e = gh(["pr", "edit", url, "--base", new_base], cwd, slug, 60)
        if e.returncode != 0:
            return gh_err(e, f"retargeting {url} to {new_base}: gh pr edit --base")
        v = gh(["pr", "view", url, "--json", "baseRefName"], cwd, slug, 60)
        try:
            now_base = json.loads(v.stdout or "{}").get("baseRefName") if v.returncode == 0 else None
        except (ValueError, AttributeError):
            now_base = None
        if now_base != new_base:
            return f"{url} is not on {new_base} after the retarget (base {now_base!r})"
    return None


def this_run(run, what):
    """The run folder, refused when it is not the run claude was launched for (DCV2_RUN_DIR)."""
    run = os.path.realpath(run)
    env = os.environ.get("DCV2_RUN_DIR", "")
    if env and os.path.realpath(env) != run:
        die(f"{what}: only this session's run ({os.path.realpath(env)}) can be asked about, not {run}")
    return run


def cmd_cleanup(run):
    run = this_run(run, "NOT CLEANED")
    st = load_status(run)
    if st.get("stage") not in ("done", "failed"):
        die(f"NOT CLEANED: cleanup runs only at stage done or failed (stage {st.get('stage')})")
    run_id = os.path.basename(run)
    g = guard_rec(run)
    repos = {os.path.realpath(r) for r in g.get("repos") or [] if isinstance(r, str)}
    by_id = {p.get("id"): p for p in st.get("prs") or [] if isinstance(p, dict)}
    results = []
    if mc_sweep(run, extra_repos=[p["repoPath"] for p in by_id.values() if isinstance(p.get("repoPath"), str) and p["repoPath"]]):
        print("merge-check scratch removed")
    def depth(pid):  # a stack's parents go before their children
        n, seen, cur = 0, {pid}, by_id.get(pid) or {}
        while cur.get("stackedOn") in by_id and cur["stackedOn"] not in seen:
            seen.add(cur["stackedOn"])
            cur = by_id[cur["stackedOn"]]
            n += 1
        return n

    closed_parents = set()
    for pid, wt in sorted((g.get("worktrees") or {}).items(), key=lambda kv: (depth(kv[0]), kv[0])):
        if not isinstance(wt, str) or not re.fullmatch(r"PR\d+", pid):
            continue
        pr = by_id.get(pid) or {}
        repo = os.path.realpath(str(pr.get("repoPath") or ""))
        if repo not in repos:
            results.append(f"{pid}: kept: its repo is not recorded for this run")
            continue
        branch, want = canon(repo, run_id, pid)
        parts = []
        if os.path.isdir(wt):
            why = verify_wt(run_id, pid, repo, wt)
            changed, kept = kept_files(wt, set(((g.get("copied") or {}).get(pid)) or [])) if not why else (None, why)
            if why or changed is None:
                parts.append(f"worktree kept: {why or kept}")
            elif changed or kept:
                parts.append(f"worktree kept: {changed} changed files" if changed else
                             f"worktree kept: has ignored files ({', '.join(kept[:3])}{' and more' if len(kept) > 3 else ''})")
            else:
                r = git("-C", repo, "worktree", "remove", "--", want)  # no --force: git refuses a dirty one too
                parts.append("worktree removed" if r.returncode == 0 else f"worktree kept: {first_line(r.stderr)}")
        else:
            git("-C", repo, "worktree", "prune")
            parts.append("worktree already gone")
        if git("-C", repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}").returncode == 0:
            if os.path.isdir(want) and f"worktree {want}" in git("-C", repo, "worktree", "list", "--porcelain").stdout.splitlines():
                parts.append(f"branch {branch} kept (its worktree is kept)")
            else:
                r = git("-C", repo, "branch", "-d", "--", branch)  # never -D
                parts.append(f"branch {branch} deleted" if r.returncode == 0 else f"branch {branch} kept (not merged here)")
        rem = (g.get("remotes") or {}).get(pid)
        oid, why, merged_base = merged_head(pr, rem if isinstance(rem, dict) else None, repo, branch)
        if not oid:
            parts.append(f"remote branch kept ({why})")
            if str(why).startswith("not merged (closed"):
                closed_parents.add(pid)
        else:
            tip, err = remote_tip(rem["url"], branch)
            if err:
                parts.append(f"remote branch kept ({err[:200]})")
            elif not tip:
                parts.append("remote branch already deleted")
            elif tip != oid:
                parts.append("remote branch kept (its tip is not the merged PR's head)")
            else:
                # A PR stacked on this branch is retargeted to this PR's own base on GitHub (and checked) before the branch goes:
                # deleting the base of an open PR closes it on GitHub. In a chain that base may already have been retargeted, so
                # GitHub's baseRefName wins over the one recorded at ship time. Any failure keeps the branch.
                bad = retarget_children(rem, repo, branch, merged_base or pr.get("base"))
                if bad:
                    parts.append(f"remote branch kept (not deleted: {bad})")
                else:
                    r = delete_remote_branch(rem["url"], branch, oid)
                    gone = r.returncode == 0 or "remote ref does not exist" in (r.stderr or "")
                    parts.append("remote branch deleted" if gone else f"remote branch kept: {first_line(r.stderr)[:200]}")
        results.append(f"{pid}: " + "; ".join(parts))
    for pid, pr in sorted(by_id.items()):
        if pr.get("stackedOn") in closed_parents:
            results.append(f"{pid}: note: stacked on {pr['stackedOn']}, which was closed without merge: {pid}'s base is unchanged and "
                           f"{pr['stackedOn']}'s branch is kept; retarget {pid} by hand if you still want it")
    if not results:
        print("cleanup: this run has no worktrees")
        return
    for line in results:
        print(line)
    append_log(run, st.get("stage") or "done", "cleanup: " + " | ".join(results))


# ---- merge-check: the repo's check on a scratch merge of its reviewed same-repo PRs ----------------

MERGE_RESULTS = ("pass", "fail", "conflict", "error", "none")
MC = {"run": None, "tracker": None}


class McInterrupted(Exception):
    pass


def ps_start(pid):
    r = run_cmd(["ps", "-o", "lstart=", "-p", str(pid)], timeout=10)
    return r.stdout.strip() if r.returncode == 0 else ""


class McTracker:
    """Records the running setup / check process group in <run>/merge-check/.running.json (pid, pgid, start time), so an
    interrupted or killed state.sh does not leave it running: the signal handler and the next sweep kill that group only."""

    def __init__(self, run):
        self.path = os.path.join(run, "merge-check", ".running.json")
        self.pgid = None

    def start(self, proc):
        self.pgid = proc.pid
        try:
            os.makedirs(os.path.dirname(self.path), exist_ok=True)
            atomic_write(self.path, json.dumps({"pid": proc.pid, "pgid": proc.pid, "start": ps_start(proc.pid)}) + "\n")
        except OSError:
            pass

    def end(self):
        self.pgid = None
        try:
            os.unlink(self.path)
        except OSError:
            pass


def mc_handler(signum, frame):
    t = MC.get("tracker")
    if t and t.pgid:
        try:
            os.killpg(t.pgid, signal.SIGKILL)
        except OSError:
            pass
    raise McInterrupted(signum)


def mc_remove_scratch(repo, scratch):
    """Layer (a): remove the scratch worktree whatever happened (git first, then the folder if still there)."""
    if repo and os.path.isdir(repo):
        git("-C", repo, "worktree", "remove", "--force", "--", scratch)
        git("-C", repo, "worktree", "prune")
    if os.path.islink(scratch):
        os.unlink(scratch)
    elif os.path.lexists(scratch):
        shutil.rmtree(scratch, ignore_errors=True)


def mc_sweep(run, extra_repos=()):
    """Layer (c): remove what a killed merge-check left under <run>/merge-check (a running group only when the pid is alive and
    its start time matches; scratch worktrees git still lists; leftover folders). Inside the run folder only, never through a link.
    Returns True when something was removed."""
    root = os.path.join(run, "merge-check")
    removed = False
    if os.path.islink(root):  # never follow a link out of the run folder
        os.unlink(root)
        return True
    if not os.path.isdir(root):
        return False
    rp = os.path.realpath(root)
    rec = load_json(os.path.join(root, ".running.json"))
    if isinstance(rec, dict) and type(rec.get("pid")) is int and type(rec.get("pgid")) is int:
        try:
            os.kill(rec["pid"], 0)
            alive = True
        except OSError:
            alive = False
        if alive and rec.get("start") and ps_start(rec["pid"]) == rec["start"]:
            try:
                os.killpg(rec["pgid"], signal.SIGKILL)
            except OSError:
                pass
    try:
        os.unlink(os.path.join(root, ".running.json"))
    except OSError:
        pass
    repos = {os.path.realpath(r) for r in guard_rec(run).get("repos") or [] if isinstance(r, str)}
    repos |= {os.path.realpath(r) for r in extra_repos}
    for repo in sorted(repos):
        if not os.path.isdir(repo):
            continue
        for line in git("-C", repo, "worktree", "list", "--porcelain").stdout.splitlines():
            if line.startswith("worktree "):
                wt = line[len("worktree "):]
                if os.path.realpath(wt).startswith(rp + os.sep):
                    git("-C", repo, "worktree", "remove", "--force", "--", wt)
                    removed = True
        git("-C", repo, "worktree", "prune")
    for name in sorted(os.listdir(root)):
        path = os.path.join(root, name)
        if os.path.islink(path):
            os.unlink(path)
            removed = True
        elif os.path.isdir(path):
            shutil.rmtree(path, ignore_errors=True)
            removed = True
    return removed


def mc_env():
    return {k: v for k, v in os.environ.items() if not k.startswith("DCV2_")}


def mc_timeout():
    v = (os.environ.get("DCV2_MERGE_CHECK_TIMEOUT") or "").strip()
    try:
        t = float(v)
        return t if t > 0 else 540.0
    except ValueError:
        return 540.0


def stack_sorted(st, prs):
    """prs with stack parents before their children (depth, then id): a child's branch contains its parent's commits."""
    by = {p.get("id"): p for p in st.get("prs") or [] if isinstance(p, dict)}

    def depth(p):
        n, seen, cur = 0, {p.get("id")}, p
        while cur.get("stackedOn") in by and cur["stackedOn"] not in seen:
            seen.add(cur["stackedOn"])
            cur = by[cur["stackedOn"]]
            n += 1
        return n

    return sorted(prs, key=lambda p: (depth(p), int(str(p.get("id"))[2:]) if str(p.get("id"))[2:].isdigit() else 0))


def mc_one(run, repo, name, prs, entry, deadline, total, base=None):
    """Check one repo's PRs on a scratch merge. Returns the mergeChecks record. base: the scratch base commit (a stack's root base)."""
    ids = [p["id"] for p in prs]
    rec = {"prs": ids, "result": "none", "error": None, "log": None, "at": now()}
    safe = re.sub(r"[^\w.-]", "_", name) or "repo"
    log = os.path.join(run, "logs", f"merge-check-{safe}.log")
    check = entry.get("check") if isinstance(entry.get("check"), str) and entry.get("check").strip() else None
    if not check:  # the command comes from repos.json only (never status.json's check.command)
        return rec
    if deadline - time.time() <= 0:
        rec.update(result="error", error="not run, time budget used")
        return rec
    scratch = os.path.join(run, "merge-check", safe)
    tracker = MC["tracker"]
    try:
        os.makedirs(os.path.join(run, "logs"), exist_ok=True)
        os.makedirs(os.path.join(run, "merge-check"), exist_ok=True)
        for p in prs:
            if git("-C", repo, "show-ref", "--verify", "--quiet", f"refs/heads/{p.get('branch')}").returncode != 0:
                rec.update(result="error", error=f"{p['id']}: branch {p.get('branch')} is missing")
                return rec
        base = base or prs[0].get("baseCommit") or ""
        r = git("-c", "core.hooksPath=/dev/null", "-C", repo, "worktree", "add", "--detach", scratch, base)
        if r.returncode != 0:
            rec.update(result="error", error="git worktree add failed: " + first_line(r.stderr)[:200])
            return rec
        copy_files(repo, scratch, entry.get("copyFiles"))
        meta = load_json(os.path.join(run, "meta.json"))
        setup = entry.get("setup")
        cmd = setup.strip() if isinstance(setup, str) else (detect_setup(scratch) if isinstance(meta, dict) and "reviewMode" in meta else None)
        result, slog = run_setup(run, scratch, f"merge-check-{safe}", cmd or None, timeout=max(1, min(600, int(deadline - time.time()))),
                                 env=mc_env(), tracker=tracker)
        if result == "failed":
            expired = time.time() >= deadline
            rec.update(result="error", error=f"timed out after {int(total)} s" if expired else f"setup failed (see {slog})", log=slog)
            return rec
        merged = []
        for p in prs:
            m = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "-c", "user.name=dcv2", "-c", "user.email=dcv2@localhost", "-C", scratch,
                                "merge", "--no-ff", "--no-edit", p["branch"]], capture_output=True, text=True)
            if m.returncode != 0:
                git("-C", scratch, "merge", "--abort")
                conflict = "CONFLICT" in (m.stdout or "") or "conflict" in (m.stderr or "").lower()
                with open(log, "w") as f:
                    f.write(f"merging {p['id']} ({p['branch']}) into the merge of {', '.join(merged) or 'the base'}:\n{m.stdout}{m.stderr}")
                what = (f"{p['id']} does not merge cleanly with {', '.join(merged)}" if merged else f"{p['id']} does not merge") if conflict \
                    else f"git merge of {p['id']} failed: {first_line(m.stderr)[:200]}"
                rec.update(result="conflict" if conflict else "error", error=what, log=log)
                return rec
            merged.append(p["id"])
        remaining = deadline - time.time()
        if remaining <= 0:
            rec.update(result="error", error=f"timed out after {int(total)} s")
            return rec
        fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o644)
        rec["log"] = log
        with os.fdopen(fd, "w") as out:
            out.write(f"$ {check}\n(in {scratch}, the merge of {', '.join(ids)})\n")
            out.flush()
            try:
                proc = subprocess.Popen(["bash", "-c", check], cwd=scratch, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                        start_new_session=True, env=mc_env())
            except OSError as e:
                out.write(f"could not start: {e}\n")
                rec.update(result="error", error=f"could not start the check: {e.strerror}")
                return rec
            tracker.start(proc)
            try:
                rc = proc.wait(timeout=remaining)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except OSError:
                    pass
                proc.wait()
                out.write(f"\ntimed out after {int(total)} s\n")
                rec.update(result="error", error=f"timed out after {int(total)} s")
                return rec
            finally:
                tracker.end()
            out.write(f"\nexit code {rc}\n")
        rec.update(result="pass" if rc == 0 else "fail")
        return rec
    finally:
        mc_remove_scratch(repo, scratch)


def mc_save(run, records):
    st = load_status(run)
    cur = st.get("mergeChecks") if isinstance(st.get("mergeChecks"), dict) else {}
    st["mergeChecks"] = {**cur, **records}
    for m in write_status(run, st, trusted=True):
        print(m)


def cmd_merge_check(run):
    run = this_run(run, "NOT CHECKED")
    st = load_status(run)
    groups = {}
    for p in st.get("prs") or []:
        if not isinstance(p, dict) or not p.get("repoPath"):
            continue
        chk = p.get("check") if isinstance(p.get("check"), dict) else {}
        if p.get("state") == "reviewed" or (p.get("state") == "needs-attention" and chk.get("result") in ("pass", "none")):
            groups.setdefault(os.path.realpath(p["repoPath"]), []).append(p)
    groups = {r: stack_sorted(st, ps) for r, ps in groups.items() if len(ps) >= 2}
    mc_sweep(run, extra_repos=[os.path.realpath(p["repoPath"]) for p in st.get("prs") or [] if isinstance(p, dict) and p.get("repoPath")])
    if not groups:
        print("merge-check: none")
        return
    total = mc_timeout()
    deadline = time.time() + total
    MC["run"], MC["tracker"] = run, McTracker(run)
    old = {sig: signal.signal(sig, mc_handler) for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP)}
    records, current, lines = {}, None, []
    try:
        for repo, prs in groups.items():
            entry = repo_entry(run, repo)
            name = entry.get("name") or prs[0].get("repo") or os.path.basename(repo)
            current = (name, [p["id"] for p in prs])
            records[name] = mc_one(run, repo, name, prs, entry, deadline, total, stack_root(st, prs[0]).get("baseCommit"))
            current = None
            r = records[name]
            lines.append(f"merge-check {name}: {r['result']} ({'+'.join(r['prs'])})" + (f" {r['error']}" if r.get("error") else "")
                         + (f" log={r['log']}" if r.get("log") else ""))
    except McInterrupted:
        if current:
            records[current[0]] = {"prs": current[1], "result": "error", "error": "interrupted", "log": None, "at": now()}
        for sig, h in old.items():
            signal.signal(sig, h)
        mc_sweep(run)
        if records:
            mc_save(run, records)
        print("merge-check: interrupted", file=sys.stderr)
        sys.exit(130)
    finally:
        for sig, h in old.items():
            signal.signal(sig, h)
    mc_save(run, records)
    for line in lines:
        print(line)


def cmd_request(run, kind):
    run = this_run(run, "NOT REQUESTED")
    if kind not in REQUEST_KINDS:
        die(f"NOT REQUESTED: the app does only {', '.join(REQUEST_KINDS)} (got {kind!r})")
    if not os.path.isfile(os.path.join(run, "request.md")):
        die(f"NOT REQUESTED: {run} is not a run folder")
    d = os.path.join(run, "requests")
    os.makedirs(d, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    rid, n = f"{stamp}-{kind}", 1
    while os.path.exists(os.path.join(d, rid + ".json")):
        n += 1
        rid = f"{stamp}-{n}-{kind}"
    atomic_write(os.path.join(d, rid + ".json"), json.dumps({"schema": 1, "id": rid, "kind": kind, "at": now()}) + "\n")
    print(f"request={rid}")


def cmd_request_wait(run, rid, rest):
    run = this_run(run, "NOT WAITED")
    if not REQUEST_ID.match(rid):
        die(f"not a request id: {rid!r} (use the id `state.sh request` printed)")
    timeout = REQUEST_WAIT_DEFAULT
    if "--timeout" in rest[:-1]:
        v = rest[rest.index("--timeout") + 1]
        if not v.isdigit():
            die("--timeout takes seconds")
        timeout = min(int(v), REQUEST_WAIT_MAX)
    if not os.path.isfile(os.path.join(run, "requests", rid + ".json")):
        die(f"no request {rid} in {run}/requests")
    res_path = os.path.join(run, "requests", rid + ".result.json")
    deadline = time.monotonic() + timeout
    res = None
    while True:
        res = load_json(res_path)
        if isinstance(res, dict) and res.get("state") in ("done", "refused", "ignored"):
            break
        if time.monotonic() >= deadline:
            break
        time.sleep(1)
    state = res.get("state") if isinstance(res, dict) else None
    msg = " ".join(str((res or {}).get("message") or "").split())[:500] if isinstance(res, dict) else ""
    if state in ("done", "refused", "ignored"):
        print(f"result={state} ok={'yes' if res.get('ok') else 'no'} message={msg or '-'}")
        return
    print(f"result=pending message={msg or ('the app has not picked it up yet: is DCTerminalv2 running?' if state is None else 'still running')}")
    sys.exit(3)


def main(argv):
    cmd, args = (argv[0] if argv else ""), argv[1:]
    need = {"preflight": 1, "put": 1, "approve": 3, "plan": 2, "base": 1, "worktree": 4, "ship-check": 1, "ship-pr": 2, "pr-body": 2, "writeback": 1, "testmode": 0,
            "gate": 4, "report": 1, "mode": 1, "draft-pr": 2, "push": 2, "post-review": 3, "comment": 4, "ready": 2, "bmad-size": 2, "bmad-init": 1, "bmad-check": 1,
            "cleanup": 1, "restack": 2, "merge-check": 1, "request": 2, "request-wait": 2}
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
    elif cmd == "pr-body": cmd_pr_body(args[0], args[1], "draft" in args[2:])
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
    elif cmd == "bmad-size": cmd_bmad_size(args[0], args[1], args[2:])
    elif cmd == "bmad-init": cmd_bmad_init(args[0])
    elif cmd == "bmad-check": cmd_bmad_check(args[0])
    elif cmd == "cleanup": cmd_cleanup(args[0])
    elif cmd == "restack": cmd_restack(args[0], args[1])
    elif cmd == "merge-check": cmd_merge_check(args[0])
    elif cmd == "request": cmd_request(args[0], args[1])
    elif cmd == "request-wait": cmd_request_wait(args[0], args[1], args[2:])


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
    # The same event twice in a row, same stage, within 30 s (state.sh already logged it and the orchestrator repeated
    # it) is written once. Retry lines always count.
    squash() { tr -s '[:space:]' ' ' <<<"$1" | sed -e 's/^ //' -e 's/ $//'; }
    lastline="$(tail -n 1 "$run/log.md" 2>/dev/null || true)"
    read -r lt ls _ <<<"$lastline" || true
    last="$(cut -d' ' -f3- <<<"$lastline")"
    lsec="$(date -u -j -f %Y-%m-%dT%H:%M:%SZ "${lt:-x}" +%s 2>/dev/null || date -u -d "${lt:-x}" +%s 2>/dev/null || echo 0)"
    if [[ -n "$last" && "$ls" == "$stage" && "$text" != *retried* ]] && (( $(date -u +%s) - lsec <= 30 )) \
       && [[ "$(squash "$last")" == "$(squash "$text")" ]]; then
      exit 0
    fi
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
