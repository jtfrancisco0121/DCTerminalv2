#!/usr/bin/env python3
"""Assertions for the golden-run suite (golden.sh), reusing check_run.py's helpers.

golden_check.py <case> <run> <ws> <transcript.jsonl> <memory root> [<onboard transcript> | <bare origin (github)>]
"""
import json
import os
import re
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_run as cr  # noqa: E402
from check_run import check, git  # noqa: E402

MEM_SH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "plugin", "scripts", "memory.sh")

CASES = {
    "multi": {"repos": {"strutil": ["npm", "run", "-s", "check"], "units": ["npm", "test", "-s"]}, "prs": 2},
    "dep": {"repos": {"pricing": ["npm", "run", "-s", "check"], "shop": ["npm", "test", "-s"]}, "prs": 2},
    "fix": {"repos": {"textkit": ["npm", "run", "-s", "check"]}, "prs": 1},
    "github": {"repos": {"greet": ["npm", "run", "-s", "check"]}, "prs": 1, "state": "opened"},
    "bmad": {"repos": {"pct": ["npm", "run", "-s", "check"]}, "prs": 1},
}


def all_tool_uses(events):
    """Every tool call in the transcript, the orchestrator's and the sub-agents'."""
    for ev in events:
        if ev.get("type") != "assistant":
            continue
        for b in (ev.get("message") or {}).get("content") or []:
            if isinstance(b, dict) and b.get("type") == "tool_use":
                yield b.get("name"), b.get("input") or {}, b.get("id")


def read_gate(run, pid):
    try:
        text = open(os.path.join(run, "gates", f"{pid}.yml")).read()
    except OSError:
        return None
    m = re.search(r"^gate:\s*(\S+)", text, re.M)
    return m.group(1) if m else None


def log_lines(run):
    try:
        return open(os.path.join(run, "log.md")).read().splitlines()
    except OSError:
        return []


def first(lines, rx):
    return next((i for i, l in enumerate(lines) if re.search(rx, l)), None)


def check_pr(pr, ws, cmds, want="local-only"):
    pid = pr["id"]
    check(pr.get("state") == want, f"{pid} {want} (got {pr.get('state')}: {pr.get('note')})")
    check((pr.get("check") or {}).get("result") == "pass", f"{pid} check pass")
    g = read_gate(cr_run, pid)
    check(g in ("PASS", "CONCERNS"), f"{pid} gate PASS/CONCERNS (got {g})")
    wt = pr.get("worktree")
    if check(wt and os.path.isdir(wt), f"{pid} worktree exists"):
        n = git("-C", wt, "rev-list", "--count", f"{pr['baseCommit']}..HEAD").stdout.strip()
        check(n.isdigit() and int(n) >= 1, f"{pid} has commits ({n})")
        ran = subprocess.run(cmds[pr["repo"]], cwd=wt, capture_output=True, text=True)
        check(ran.returncode == 0, f"{pid} check passes in its worktree: {ran.stdout[-200:]} {ran.stderr[-300:]}")
        check(git("-C", wt, "status", "--porcelain").stdout.strip() == "", f"{pid} worktree clean")


def check_mains(ws, cmds):
    for repo, cmd in cmds.items():
        d = os.path.join(ws, repo)
        check(git("-C", d, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() == "main", f"{repo} main checkout on main")
        check(git("-C", d, "rev-list", "--count", "main").stdout.strip() == "1", f"{repo} main has only the initial commit")
        check(git("-C", d, "status", "--porcelain").stdout.strip() == "", f"{repo} main checkout clean")
        check(subprocess.run(cmd, cwd=d, capture_output=True).returncode != 0, f"{repo} main checkout check still fails (untouched)")


def check_memory_writes(events, mem, label):
    """Nobody wrote into the memory root with Edit/Write (the guard blocks it; memory.sh is the path)."""
    blocked = cr.guard_blocked(events)
    bad = [f"{n} {i.get('file_path')}" for n, i, tid in all_tool_uses(events)
           if n in ("Write", "Edit", "MultiEdit") and str(i.get("file_path", "")).startswith(mem) and tid not in blocked]
    check(not bad, f"{label}: memory written only via memory.sh: {bad[:3]}")


def main():
    global cr_run
    case, run, ws, tpath, mem = sys.argv[1:6]
    onboard = sys.argv[6] if len(sys.argv) > 6 and case == "multi" else None
    origin = sys.argv[6] if len(sys.argv) > 6 and case == "github" else None
    spec = CASES[case]
    ws, mem, cr_run = os.path.realpath(ws), os.path.realpath(mem), run
    st = cr.load_status(run)
    events = cr.transcript(tpath)
    res = cr.final_result(events)
    check(res and not res.get("is_error"), f"claude run ended without error ({res.get('subtype')}, {str(res.get('result'))[:200]})")
    cr.check_orchestrator_context(events, ws)
    cr.check_state_helper(events, os.path.realpath(run))
    cr.check_orchestrator_inputs(events, run)
    check_memory_writes(events, mem, "run")
    kinds = [k for k, _ in cr.subagent_launches(events)]
    print(f"    [{case}] sub-agents: {kinds}")
    if st is None:
        cr.report()
    cr.check_schema(st)
    check(st.get("stage") == "done", f"stage done (got {st.get('stage')}: {st.get('error')})")
    prs = st.get("prs") or []
    check(len(prs) == spec["prs"], f"{spec['prs']} PR(s) ({len(prs)})")
    for pr in prs:
        check_pr(pr, ws, spec["repos"], spec.get("state", "local-only"))
    check_mains(ws, spec["repos"])
    rep = os.path.join(run, "report.md")
    if check(os.path.isfile(rep), "report.md written (R10)"):
        text = open(rep).read()
        for pr in prs:
            check(f"| {pr['id']} |" in text, f"report.md has {pr['id']}")
        check("Gate:" in text, "report.md has gates")
    check(os.path.isfile(os.path.join(run, "memory-update.md")), "memory-update.md written (Step 5)")
    check("dcv2:memory-updater" in kinds or os.path.isfile(os.path.join(run, "memory-update.md")), "memory update ran")
    lines = log_lines(run)
    by_repo = {p.get("repo"): p for p in prs}

    if case == "multi":
        check(all(not p.get("dependsOn") for p in prs), f"no dependencies ({[p.get('dependsOn') for p in prs]})")
        st_mem = json.loads(subprocess.run(["bash", MEM_SH, "status", ws, "--root", mem], capture_output=True, text=True).stdout or "{}")
        check(all(r.get("present") for r in st_mem.get("repos", [])), f"onboard wrote index.md for every repo ({[(r.get('name'), r.get('present')) for r in st_mem.get('repos', [])]})")
        check((st_mem.get("workspace") or {}).get("present") is True, "onboard wrote workspace.md")
        if onboard:
            ev = cr.transcript(onboard)
            r = cr.final_result(ev)
            check(r and not r.get("is_error"), "onboard ended without error")
            check_memory_writes(ev, mem, "onboard")
            check(any(n == "Bash" and "memory.sh" in str(i.get("command")) and " write " in str(i.get("command"))
                      for n, i, _ in all_tool_uses(ev)), "onboard wrote through memory.sh write")
        check(kinds.count("dcv2:memory-updater") >= 1, "the memory updater ran (memory exists)")
        # R6 with DCV2_MAX_IMPLEMENTERS=1: the second PR starts building only after the first one left building/fixing.
        starts = [i for i, l in enumerate(lines) if re.search(r"\bPR\d+ building\b", l)]
        built = first(lines, r"\bPR\d+ (built|reviewed|review round)")
        p1 = next((p for p in prs if p.get("id") == "PR1"), {})
        p2 = next((p for p in prs if p.get("id") == "PR2"), {})
        stamped = bool(p1.get("startedAt") and p2.get("startedAt") and p2["startedAt"] > p1["startedAt"])
        check((len(starts) >= 2 and built is not None and starts[1] > built) or
              (stamped and not any(re.search(r"PR1.*PR2 building|PR2.*PR1 building", l) for l in lines)),
              f"cap 1: the second PR started after the first was built (starts {starts[:2]}, first built {built})")
        launches = [k for k, _ in cr.subagent_launches(events)]
        check(launches.count("dcv2:implementer") >= 2, "an implementer per PR")

    elif case == "dep":
        p1, p2 = by_repo.get("pricing"), by_repo.get("shop")
        if check(p1 and p2, "a PR per repo"):
            check(p1["id"] in (p2.get("dependsOn") or []), f"shop PR depends on pricing PR ({p2.get('dependsOn')})")
            # From state.sh's own stamps, not the orchestrator's log wording: PR2 left pending
            # (startedAt) only after PR1's passing gate was written (the step before reviewed).
            gate1 = open(os.path.join(run, "gates", f"{p1['id']}.yml")).read() if os.path.isfile(os.path.join(run, "gates", f"{p1['id']}.yml")) else ""
            m = re.search(r"^updated:\s*(\S+)", gate1, re.M)
            g1, s2 = (m.group(1) if m else None), p2.get("startedAt")
            check(g1 and s2 and s2 >= g1, f"{p2['id']} started ({s2}) only after {p1['id']}'s gate ({g1})")

    elif case == "fix":
        p = by_repo.get("textkit")
        if p and p.get("worktree"):
            ran = subprocess.run(["node", "-e", "const t=require('./text'); process.exit(t.capitalize('') === '' ? 0 : 1)"],
                                 cwd=p["worktree"], capture_output=True)
            check(ran.returncode == 0, "the pre-existing capitalize('') failure was fixed too")
        check("bmad" not in st, "mode off: no bmad object in status.json (the unchanged flow)")
        check(not os.path.exists(os.path.join(run, "bmad")), "mode off: no <run>/bmad folder")
    elif case == "github":
        check_github(run, prs, events, origin, lines)
    elif case == "bmad":
        check_bmad(run, ws, prs, st)
    cr.report()


def bmad_digest(run, docs):
    """The same digest state.sh bmad-check records: sha-256 over path (relative to the run) + bytes of every document."""
    import hashlib
    h = hashlib.sha256()
    for rel in sorted(docs):
        h.update(rel.encode() + b"\0")
        h.update(open(os.path.join(run, rel), "rb").read())
        h.update(b"\0")
    return h.hexdigest()


def check_bmad(run, ws, prs, st):
    bm = st.get("bmad") or {}
    check(" " in run, f"the run folder path has a space ({run})")
    check(bm.get("mode") == "on" and bm.get("applied") is True, f"status.json bmad applied in mode on ({bm.get('mode')}, {bm.get('applied')})")
    chk = bm.get("check") or {}
    check(chk.get("result") == "pass", f"bmad.check.result pass (got {chk.get('result')}: {chk.get('reasons')})")
    out = os.path.join(run, "bmad", "output")
    found = {k: [] for k in ("spec", "architecture")}
    for root, dirs, files in os.walk(out):
        for f in files:
            for k in found:
                if f.startswith(k + "-") and f.endswith(".md") and os.path.basename(root) == f[:-3]:
                    found[k].append(os.path.join(root, f))
    for k, v in found.items():
        check(len(v) >= 1, f"a {k} document under <run>/bmad/output ({v})")
    spec = open(found["spec"][0]).read() if found["spec"] else ""
    check(re.search(r"^id:\s*SPEC-", spec, re.M) is not None, "the spec has an id: SPEC- frontmatter")
    docs = chk.get("documents") or []
    check(bool(docs) and all(os.path.isfile(os.path.join(run, d)) for d in docs), f"check.documents exist ({docs})")
    if docs:
        check(chk.get("digest") == bmad_digest(run, docs), "the last bmad-check digest matches the documents (unchanged since stop #1)")
    plan = open(os.path.join(run, "plan.md")).read()
    check(re.search(r"\bCAP-\d+", plan) is not None, "plan.md stories cite spec items (CAP-n)")
    check(os.path.isfile(os.path.join(run, "bmad", "_bmad", "config.toml")), "bmad-init made config.toml")
    leftovers = []
    for pr in prs:
        for base in (os.path.join(ws, pr["repo"]), pr.get("worktree") or ""):
            if base and os.path.isdir(base):
                for name in ("_bmad", "_bmad-output", "bmad"):
                    if os.path.exists(os.path.join(base, name)):
                        leftovers.append(os.path.join(base, name))
    check(not leftovers, f"no BMAD files in the repo or its worktree ({leftovers})")


def check_github(run, prs, events, origin, lines):
    shim = os.environ.get("GH_SHIM_DIR", "")
    calls = open(os.path.join(shim, "calls.log")).read().splitlines() if os.path.isfile(os.path.join(shim, "calls.log")) else []
    try:
        gh_prs = json.load(open(os.path.join(shim, "prs.json")))
    except (OSError, ValueError):
        gh_prs = []
    pr = prs[0] if prs else {}
    check(len(gh_prs) == 1, f"one PR on the (fake) GitHub ({len(gh_prs)})")
    check(any(c.startswith("pr create --draft") for c in calls), "opened as a draft (gh pr create --draft)")
    check(not any(c.startswith("pr create") and "--draft" not in c for c in calls), "never a non-draft gh pr create")
    check(any(c.startswith("pr ready") for c in calls), "marked ready at stop #2 (gh pr ready)")
    if gh_prs:
        g = gh_prs[0]
        check(g.get("url") == pr.get("prUrl"), f"prUrl is the PR's ({pr.get('prUrl')} vs {g.get('url')})")
        check(g.get("isDraft") is False, "the PR is ready for review now")
        check("Eagle-Eye review rounds" in (g.get("body") or ""), "final body lists the review rounds")
    check(pr.get("draft") is False, f"status.json draft false ({pr.get('draft')})")
    comments = pr.get("comments") or []
    check(comments and comments[0].get("round") == 1 and str(comments[0].get("url", "")).startswith("https://"),
          f"review round 1 posted and recorded ({comments})")
    rounds = (pr.get("review") or {}).get("round") or 0
    check(len(comments) == rounds, f"one comment per review round ({len(comments)} comments, {rounds} rounds)")
    posted = [c for c in calls if c.startswith("pr comment")]
    want = len(comments) + sum(1 for c in comments if c.get("fixesUrl"))
    check(len(posted) == want, f"no comment posted twice ({len(posted)} gh pr comment calls, {want} recorded)")
    first = open(os.path.join(run, "reviews", f"{pr.get('id')}-round1.md")).readline() if pr.get("id") else ""
    check(re.match(r"^\W*Eagle-Eye review\s*[\u2014\u2013-]+\s*round 1:\s*(APPROVED|REQUEST CHANGES)", first), f"review file first line is the fixed verdict line ({first.strip()[:80]})")
    wt = pr.get("worktree") or ""
    if origin and wt:
        on = git("--git-dir", origin, "rev-parse", f"refs/heads/{pr.get('branch')}").stdout.strip()
        check(on and on == git("-C", wt, "rev-parse", "HEAD").stdout.strip(), "the branch on the remote is the worktree's HEAD")
    if wt:
        check(os.path.isfile(os.path.join(wt, ".env")), "copyFiles: .env copied into the worktree")
        tree = git("-C", wt, "log", "--all", "--name-only", "--format=").stdout.split()
        check(".env" not in tree, "copyFiles: .env never committed")
    blocked = cr.guard_blocked(events)
    direct = [str(i.get("command"))[:120] for n, i, tid in all_tool_uses(events)
              if n == "Bash" and tid not in blocked and "state.sh" not in str(i.get("command"))
              and re.search(r"\bgit\b[^|;&]*\bpush\b|\bgh\s+pr\s+(create|comment|edit|ready|merge|close)\b|\bgh\s+api\b.*\s(-X|--method|-f|-F|--field|--raw-field|--input)\b|\bgh\s+issue\s+comment\b", str(i.get("command")))]
    check(not direct, f"no direct git push / gh by any agent: {direct[:3]}")
    check(first_idx(lines, r"draft PR (opened|found)") is not None, "log: draft PR opened")
    opened = [l for l in lines if re.search(r"\bdraft PR (opened|found)\b", l)]
    check(len(opened) == len(set(re.sub(r"^\S+ \S+ ", "", l) for l in opened)), f"log: each draft PR logged once ({len(opened)} lines)")
    check(first_idx(lines, r"review round 1 posted") is not None, "log: review round 1 posted")


def first_idx(lines, rx):
    return first(lines, rx)


if __name__ == "__main__":
    main()
