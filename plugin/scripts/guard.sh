#!/usr/bin/env bash
# PreToolUse guard for the eagle-eye pipeline sub-agents (plugin hook).
# dcv2:implementer may write only inside an Eagle-Eye worktree
# (<repo>-worktrees/dcv2-*) or a run folder; dcv2:planner, dcv2:plan-reviewer and
# dcv2:pr-reviewer only inside a run folder. A run folder is a folder holding
# request.md and status.json. Everything else (the orchestrator, standalone role
# skills, the user's own session) is not affected. Exit 2 blocks the tool call.
command -v python3 >/dev/null 2>&1 || exit 0
exec python3 -c '
import json, os, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
agent = data.get("agent_type") or ""
if agent not in ("dcv2:implementer", "dcv2:planner", "dcv2:plan-reviewer", "dcv2:pr-reviewer"):
    sys.exit(0)
inp = data.get("tool_input") or {}
path = inp.get("file_path") or inp.get("notebook_path") or ""
if not path:
    sys.exit(0)
path = os.path.realpath(os.path.join(data.get("cwd") or "/", path))

def in_run_folder(p):
    d = os.path.dirname(p)
    while d and d != os.path.dirname(d):
        if os.path.isfile(os.path.join(d, "request.md")) and os.path.isfile(os.path.join(d, "status.json")):
            return True
        d = os.path.dirname(d)
    return False

def in_worktree(p):
    parts = p.split(os.sep)
    return any(a.endswith("-worktrees") and b.startswith("dcv2-") for a, b in zip(parts, parts[1:]))

ok = in_run_folder(path) or (agent == "dcv2:implementer" and in_worktree(path))
if ok:
    sys.exit(0)
where = "your worktree (<repo>-worktrees/dcv2-...) or the run folder" if agent == "dcv2:implementer" else "the run folder"
print(f"Blocked by the dcv2 guard: {agent} may only write inside {where}, not {path}. Never edit a repo main checkout; use the worktree path you were given.", file=sys.stderr)
sys.exit(2)
'
