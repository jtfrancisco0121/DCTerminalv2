#!/usr/bin/env python3
"""Assertions for the dcv2 eagle-eye e2e test.

check_run.py done    <run> <ws> <transcript.jsonl>          full run finished
check_run.py halted  <run> <ws> <transcript.jsonl> <PR id>  halted after a PR (test-mode)
check_run.py resumed <run> <ws> <transcript.jsonl> <PR id> <sha of that PR's branch before resume>
"""
import json
import os
import re
import subprocess
import sys

errors = []


def check(cond, msg):
    if not cond:
        errors.append(msg)
    return cond


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True)


def load_status(run):
    path = os.path.join(run, "status.json")
    if not check(os.path.isfile(path), "status.json exists"):
        return None
    with open(path) as f:
        try:
            return json.load(f)
        except json.JSONDecodeError as e:
            check(False, f"status.json is valid JSON ({e})")
            return None


STAGES = {"planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"}
PR_STATES = {"pending", "building", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"}
CHECK_RESULTS = {"not-run", "pass", "fail", "none", "error"}


def check_schema(st):
    check(st.get("schema") == 1, "status.schema == 1")
    check(st.get("stage") in STAGES, f"status.stage valid ({st.get('stage')})")
    check(st.get("waitingFor") in (None, "plan-approval", "ship-approval"), "status.waitingFor valid")
    check(bool(re.match(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$", str(st.get("updatedAt")))), f"updatedAt is UTC ISO ({st.get('updatedAt')})")
    for key in ("runId", "folder", "plan", "prs", "ship", "writeback", "error", "paused", "shipApproval"):
        check(key in st, f"status has '{key}'")
    for pr in st.get("prs", []):
        pid = pr.get("id")
        for key in ("id", "title", "repo", "repoPath", "branch", "base", "baseCommit", "worktree", "dependsOn",
                    "stories", "commits", "check", "review", "prUrl", "state", "note", "startedAt", "updatedAt", "agent"):
            check(key in pr, f"{pid} has '{key}'")
        check(bool((pr.get("stories") or {}).get("ids")), f"{pid}.stories.ids set")
        for key in ("cwd", "log"):
            check(key in (pr.get("check") or {}), f"{pid}.check has '{key}'")
        check(pr.get("state") in PR_STATES, f"{pid}.state valid ({pr.get('state')})")
        check(isinstance(pr.get("dependsOn"), list), f"{pid}.dependsOn is a list")
        check((pr.get("check") or {}).get("result") in CHECK_RESULTS, f"{pid}.check.result valid")
        check(str(pr.get("branch", "")).startswith("dcv2/"), f"{pid}.branch starts with dcv2/ ({pr.get('branch')})")
        wt = pr.get("worktree") or ""
        repo_path = pr.get("repoPath") or ""
        expected_parent = os.path.join(os.path.dirname(repo_path), os.path.basename(repo_path) + "-worktrees")
        check(os.path.dirname(wt) == expected_parent, f"{pid}.worktree is beside its repo in {expected_parent} ({wt})")


def pr_by_repo(st, repo):
    for pr in st.get("prs", []):
        if pr.get("repo") == repo:
            return pr
    return None


CHECKS = {"mathlib": ["npm", "run", "-s", "check"], "calc-cli": ["npm", "test", "-s"]}


def check_pr_done(pr, ws):
    pid = pr["id"]
    check(pr.get("state") == "local-only", f"{pid} is local-only (no remote) (got {pr.get('state')})")
    check((pr.get("review") or {}).get("verdict") == "APPROVED", f"{pid} review APPROVED (got {(pr.get('review') or {}).get('verdict')})")
    check((pr.get("check") or {}).get("result") == "pass", f"{pid} check pass")
    check(pr.get("prUrl") is None, f"{pid} has no PR url (no remote)")
    wt = pr.get("worktree")
    if check(wt and os.path.isdir(wt), f"{pid} worktree exists ({wt})"):
        n = git("-C", wt, "rev-list", "--count", f"{pr['baseCommit']}..HEAD").stdout.strip()
        check(n.isdigit() and int(n) >= 1, f"{pid} worktree has commits ({n})")
        head_branch = git("-C", wt, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
        check(head_branch == pr["branch"], f"{pid} worktree is on its branch ({head_branch})")
        ran = subprocess.run(CHECKS[pr["repo"]], cwd=wt, capture_output=True, text=True)
        check(ran.returncode == 0, f"{pid} check passes in its worktree: {ran.stdout[-200:]} {ran.stderr[-200:]}")
        status = git("-C", wt, "status", "--porcelain").stdout.strip()
        check(status == "", f"{pid} worktree is clean: {status[:200]}")


def check_main_checkouts_untouched(ws):
    for repo, cmd in CHECKS.items():
        d = os.path.join(ws, repo)
        branch = git("-C", d, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
        check(branch == "main", f"{repo} main checkout still on main ({branch})")
        count = git("-C", d, "rev-list", "--count", "main").stdout.strip()
        check(count == "1", f"{repo} main has only the initial commit ({count})")
        status = git("-C", d, "status", "--porcelain").stdout.strip()
        check(status == "", f"{repo} main checkout clean: {status[:200]}")
        ran = subprocess.run(cmd, cwd=d, capture_output=True, text=True)
        check(ran.returncode != 0, f"{repo} main checkout check still fails (untouched)")
        check(git("-C", d, "remote").stdout.strip() == "", f"{repo} has no remote")


def check_files(run, min_log_lines):
    check(os.path.isfile(os.path.join(run, "plan.md")), "plan.md exists")
    plan = open(os.path.join(run, "plan.md")).read() if os.path.isfile(os.path.join(run, "plan.md")) else ""
    check("## Pull requests" in plan and "### PR1" in plan, "plan.md has a Pull requests section with PR1")
    log_path = os.path.join(run, "log.md")
    lines = open(log_path).read().splitlines() if os.path.isfile(log_path) else []
    good = [l for l in lines if re.match(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ \S+ \S", l)]
    check(len(good) >= min_log_lines, f"log.md has >= {min_log_lines} well-formed lines (got {len(good)} of {len(lines)})")
    return lines


def transcript(path):
    events = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line.startswith("{"):
                try:
                    events.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    return events


def guard_blocked(events):
    """ids of tool calls the dcv2 guard hook blocked (they did not run)."""
    ids = set()
    for ev in events:
        for block in (ev.get("message") or {}).get("content") or []:
            if isinstance(block, dict) and block.get("type") == "tool_result" and "Blocked by the dcv2 guard" in str(block.get("content")):
                ids.add(block.get("tool_use_id"))
    return ids


def orchestrator_tool_uses(events, ran_only=False):
    """Tool calls made by the orchestrator itself (not inside a sub-agent); ran_only skips guard-blocked ones."""
    blocked = guard_blocked(events) if ran_only else set()
    for ev in events:
        if ev.get("type") != "assistant" or ev.get("parent_tool_use_id"):
            continue
        for block in (ev.get("message") or {}).get("content") or []:
            if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("id") not in blocked:
                yield block.get("name"), block.get("input") or {}


def subagent_launches(events):
    return [(inp.get("subagent_type"), inp.get("prompt", "")) for name, inp in orchestrator_tool_uses(events)
            if name in ("Task", "Agent")]


# A command that reads files, diffs or test output inside a worktree.
HEAVY = [
    re.compile(r"\b(cat|head|tail|less|more|grep|sed|awk)\b[^|;&>]*-worktrees/"),
    re.compile(r"git\s+-C\s+\S*-worktrees/\S*\s+(diff|show)\b"),
    re.compile(r"cd\s+\S*-worktrees/\S*\s*&&\s*(npm|node|go|cargo|make|pytest)\b"),
]


def strip_heredocs(cmd):
    """Drop heredoc bodies: what the orchestrator writes is not what it reads."""
    out, skip = [], None
    for line in cmd.split("\n"):
        if skip is not None:
            if line.strip() == skip:
                skip = None
            continue
        out.append(line)
        m = re.search(r"<<-?\s*['\"]?(\w+)['\"]?", line)
        if m:
            skip = m.group(1)
    return "\n".join(out)


def check_orchestrator_context(events, ws):
    """Context rule 1: the orchestrator never reads code, diffs or test output in the worktrees."""
    bad = []
    for name, inp in orchestrator_tool_uses(events):
        if name == "Read":
            p = inp.get("file_path", "")
            if p.startswith(ws):
                bad.append(f"Read {p}")
        elif name == "Bash":
            cmd = strip_heredocs(inp.get("command", ""))
            if any(h.search(cmd) for h in HEAVY):
                bad.append(f"Bash {cmd[:200]}")
    check(not bad, "orchestrator never reads code/diffs/test output in worktrees: " + "; ".join(bad[:5]))


def check_state_helper(events, run):
    """status.json is only written through state.sh put (validated + atomic); attempts the guard blocked did not write."""
    bad = []
    for name, inp in orchestrator_tool_uses(events, ran_only=True):
        if name in ("Write", "Edit") and str(inp.get("file_path", "")).endswith(("status.json", "writeback.json", ".guard.json")):
            bad.append(f"{name} {inp.get('file_path')}")
        if name == "Bash":
            cmd = strip_heredocs(inp.get("command", ""))
            if re.search(r">\s*['\"]?" + re.escape(run) + r"/(status|writeback|\.guard)\.json\b", cmd):
                bad.append(f"Bash {cmd[:160]}")
    check(not bad, "state files written only via state.sh: " + "; ".join(bad[:3]))


def check_orchestrator_inputs(events, run):
    """The orchestrator reads neither request.md nor tickets.json, and ships only via state.sh."""
    bad = []
    for name, inp in orchestrator_tool_uses(events):
        if name == "Read" and os.path.basename(str(inp.get("file_path", ""))) in ("request.md", "tickets.json"):
            bad.append(f"Read {inp.get('file_path')}")
        if name == "Bash":
            cmd = strip_heredocs(inp.get("command", ""))
            if re.search(r"\b(cat|head|tail|less|grep|sed|awk)\b[^|;&]*(request\.md|tickets\.json)", cmd):
                bad.append(f"Bash {cmd[:160]}")
            if re.search(r"\bgit\b[^|;&]*\bpush\b|\bgh\s+pr\s+create\b", cmd):
                bad.append(f"Bash {cmd[:160]}")
    check(not bad, "orchestrator never reads request.md / tickets.json and ships only via state.sh: " + "; ".join(bad[:3]))


def final_result(events):
    results = [e for e in events if e.get("type") == "result"]
    return results[-1] if results else {}


def main():
    mode, run, ws, tpath = sys.argv[1:5]
    ws = os.path.realpath(ws)
    st = load_status(run)
    events = transcript(tpath)
    res = final_result(events)
    check(res and not res.get("is_error"), f"claude run ended without error ({res.get('subtype')}, {str(res.get('result'))[:200]})")
    check_orchestrator_context(events, ws)
    check_state_helper(events, os.path.realpath(run))
    check_orchestrator_inputs(events, run)
    launches = subagent_launches(events)
    kinds = [k for k, _ in launches]
    print(f"    sub-agents launched by the orchestrator: {kinds}")
    if st is None:
        report()
    check_schema(st)
    check_main_checkouts_untouched(ws)
    mathlib, cli = pr_by_repo(st, "mathlib"), pr_by_repo(st, "calc-cli")
    check(mathlib is not None and cli is not None, "status has a PR for each repo")
    check(len(st.get("prs", [])) == 2, f"exactly two PRs ({len(st.get('prs', []))})")
    if mathlib and cli:
        check(mathlib["id"] in cli.get("dependsOn", []), f"calc-cli PR depends on the mathlib PR ({cli.get('dependsOn')})")

    if mode == "done":
        check(st.get("stage") == "done", f"stage done (got {st.get('stage')})")
        check(st.get("waitingFor") is None, "waitingFor null")
        check((st.get("plan") or {}).get("approved") is True, "plan approved")
        check(((st.get("plan") or {}).get("approval") or {}).get("by") == "test", f"plan.approval by test ({(st.get('plan') or {}).get('approval')})")
        check((st.get("shipApproval") or {}).get("by") == "test", f"shipApproval by test ({st.get('shipApproval')})")
        check((st.get("ship") or {}).get("remote") == "none", f"ship.remote none ({st.get('ship')})")
        check((st.get("ship") or {}).get("pushed") is False, "nothing pushed")
        for pr in (mathlib, cli):
            if pr:
                check_pr_done(pr, ws)
        lines = check_files(run, 8)
        check(any("auto-approved (test-mode)" in l for l in lines), "log shows test-mode auto-approval")
        wb_path = os.path.join(run, "writeback.json")
        if check(os.path.isfile(wb_path), "writeback.json exists"):
            wb = json.load(open(wb_path))
            check(wb.get("schema") == 1, "writeback.schema == 1")
            drafts = wb.get("drafts", [])
            check(len(drafts) == 2, f"two writeback drafts ({len(drafts)})")
            for d in drafts:
                check(isinstance(d.get("ticket"), dict) and d["ticket"].get("id") in ("t1", "t2"), f"draft ticket id ({d.get('ticket')})")
                check(bool(d.get("comment")), "draft has a comment")
                check("status" in d, "draft has a status field")
        for kind in ("dcv2:planner", "dcv2:plan-reviewer"):
            check(kind in kinds, f"{kind} was launched")
        check(kinds.count("dcv2:implementer") >= 2, "an implementer per PR")
        check(kinds.count("dcv2:pr-reviewer") >= 2, "a PR reviewer per PR")

    elif mode == "halted":
        halt_pr = sys.argv[5]
        check(st.get("stage") == "building", f"halted run stays in building (got {st.get('stage')})")
        halted = next((p for p in st.get("prs", []) if p["id"] == halt_pr), None)
        if check(halted is not None, f"{halt_pr} in status"):
            check(halted.get("state") == "reviewed", f"{halt_pr} reviewed (got {halted.get('state')})")
        for p in st.get("prs", []):
            if p["id"] != halt_pr:
                check(p.get("state") == "pending", f"{p['id']} not started (got {p.get('state')})")
        lines = check_files(run, 5)
        check(any("halted after" in l for l in lines), "log shows the test-mode halt")

    elif mode == "resumed":
        halt_pr, sha_before = sys.argv[5], sys.argv[6]
        check(st.get("stage") == "done", f"stage done after resume (got {st.get('stage')})")
        for pr in (mathlib, cli):
            if pr:
                check_pr_done(pr, ws)
        halted = next((p for p in st.get("prs", []) if p["id"] == halt_pr), None)
        if halted:
            sha_after = git("-C", halted["repoPath"], "rev-parse", halted["branch"]).stdout.strip()
            check(sha_after == sha_before, f"{halt_pr} branch unchanged by resume ({sha_before[:8]} -> {sha_after[:8]})")
        lines = check_files(run, 8)
        idx = next((i for i, l in enumerate(lines) if "resumed" in l), None)
        if check(idx is not None, "log has a 'resumed' line"):
            redo = [l for l in lines[idx:] if re.search(rf"\b{halt_pr}\b.*\b(building|built|review round)\b", l)]
            check(not redo, f"resume did not redo {halt_pr}: {redo[:3]}")
        impl = [p for k, p in launches if k == "dcv2:implementer"]
        check(1 <= len(impl) <= 3, f"resume launched 1-3 implementers, only for the unfinished PR ({len(impl)})")
        check("dcv2:planner" not in kinds, "resume did not re-plan")
    report()


def report():
    if errors:
        print("    assertions FAILED:")
        for e in errors:
            print(f"      - {e}")
        sys.exit(1)
    print("    all assertions passed")
    sys.exit(0)


if __name__ == "__main__":
    main()
