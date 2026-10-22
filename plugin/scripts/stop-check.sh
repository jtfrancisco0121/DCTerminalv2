#!/usr/bin/env bash
# Stop / SubagentStop guardrail (A3). When an Implementer sub-agent of the bound Eagle-Eye run
# (DCV2_RUN_DIR) is about to finish, run its PR's check in its worktree (30 s timeout). If the
# check fails, exit 2 with trimmed findings so the Implementer fixes them before it replies.
#
# Never blocks: the main session's Stop, other agents, standalone sessions (no DCV2_RUN_DIR),
# a second stop in a row (stop_hook_active), DCV2_STOP_HOOKS=off, a PR it cannot identify, a PR
# not building/fixing, no check command, a missing worktree, a check slower than the timeout
# (remembered per PR in <run>/logs/.stop-check.json, then skipped), or any error of its own.
# Full output of a failing check: <run>/logs/<PR>-stop-check.txt.
# DCV2_STOP_CHECK_TIMEOUT (seconds, default 30) is for tests.
input="$(cat)"
[[ "${DCV2_STOP_HOOKS:-on}" == "off" ]] && exit 0
[[ -n "${DCV2_RUN_DIR:-}" ]] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

IFS= read -r -d '' PY <<'PY' || true
import json, os, re, signal, subprocess, sys, tempfile, time

try:
    data = json.loads(sys.stdin.read())
except Exception:
    sys.exit(0)
if not isinstance(data, dict) or data.get("stop_hook_active"):
    sys.exit(0)
agent = str(data.get("agent_type") or "")
if (agent[5:] if agent.startswith("dcv2:") else agent) != "implementer":
    sys.exit(0)
run = os.path.realpath(os.environ["DCV2_RUN_DIR"])
try:
    st = json.load(open(os.path.join(run, "status.json")))
except Exception:
    sys.exit(0)
prs = [p for p in st.get("prs") or [] if isinstance(p, dict)]
by_id = {p.get("id"): p for p in prs}


def from_text(text):
    for m in re.finditer(r'"pr"\s*:\s*"(PR\d+)"|\bpr\s*[=:]\s*`?(PR\d+)\b', text or ""):
        pid = m.group(1) or m.group(2)
        if pid in by_id:
            return pid
    for p in prs:
        wt = p.get("worktree")
        if wt and wt in (text or ""):
            return p.get("id")
    return None


pid = from_text(str(data.get("last_assistant_message") or ""))
if not pid:
    tp = str(data.get("agent_transcript_path") or "")
    try:
        with open(tp) as f:
            head = f.read(200_000)
        first = next((json.loads(l) for l in head.splitlines() if '"user"' in l), None)
        pid = from_text(json.dumps(first)) if first else None
    except Exception:
        pid = None
if not pid:
    busy = [p.get("id") for p in prs if p.get("state") in ("building", "fixing")]
    pid = busy[0] if len(busy) == 1 else None
pr = by_id.get(pid)
if not pr or pr.get("state") not in ("building", "fixing"):
    sys.exit(0)
chk = pr.get("check") or {}
cmd, wt = chk.get("command"), pr.get("worktree")
cwd = chk.get("cwd") or wt
if not cmd or not wt or not os.path.isdir(wt) or not cwd or not os.path.isdir(cwd):
    sys.exit(0)

logs = os.path.join(run, "logs")
cache_path = os.path.join(logs, ".stop-check.json")
try:
    cache = json.load(open(cache_path))
    cache = cache if isinstance(cache, dict) else {}
except Exception:
    cache = {}
if (cache.get(pid) or {}).get("slow") and cache[pid].get("command") == cmd:
    sys.exit(0)


def save_cache():
    os.makedirs(logs, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".stop-check.json.", suffix=".tmp", dir=logs)
    with os.fdopen(fd, "w") as f:
        json.dump(cache, f)
    os.replace(tmp, cache_path)


limit = float(os.environ.get("DCV2_STOP_CHECK_TIMEOUT") or 30)
start = time.time()
proc = subprocess.Popen(["bash", "-c", cmd], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                        stdin=subprocess.DEVNULL, start_new_session=True)
try:
    out, _ = proc.communicate(timeout=limit)
except subprocess.TimeoutExpired:
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except OSError:
        pass
    proc.communicate()
    cache[pid] = {"slow": True, "command": cmd, "seconds": limit}
    save_cache()
    sys.exit(0)
cache[pid] = {"slow": False, "command": cmd, "seconds": round(time.time() - start, 1)}
save_cache()
if proc.returncode == 0:
    sys.exit(0)

text = out.decode("utf-8", "replace")
os.makedirs(logs, exist_ok=True)
full = os.path.join(logs, f"{pid}-stop-check.txt")
with open(full, "w") as f:
    f.write(text)
# Trim: strip ANSI and progress redraws, keep the head, error-looking lines and the tail.
text = re.sub(r"\x1b\[[0-9;?]*[ -/]*[@-~]", "", text)
lines = [l.rsplit("\r", 1)[-1].rstrip() for l in text.splitlines()]
lines = [l for l in lines if l.strip()]
ERR = re.compile(r"error|fail|assert|expected|panic|exception|traceback|not ok|\bFAIL\b|✗|×", re.I)
keep = set(range(min(10, len(lines)))) | set(range(max(0, len(lines) - 30), len(lines)))
keep |= {i for i, l in enumerate(lines) if ERR.search(l)}
shown, last = [], -1
for i in sorted(keep):
    if i != last + 1:
        shown.append("  [...]")
    shown.append("  " + lines[i][:300])
    last = i
body = "\n".join(shown)
if len(body) > 3500:
    body = body[:1200] + "\n  [...]\n" + body[-2200:]
print(f"dcv2 stop check: the {pid} check `{cmd}` fails in {cwd} (exit {proc.returncode}). Fix it in your worktree before "
      f"you finish, commit, re-run the check (write {run}/logs/{pid}-check.txt), and update your report and JSON reply. "
      f"Full output: {full}\n{body}", file=sys.stderr)
sys.exit(2)
PY

python3 -c "$PY" <<<"$input"
rc=$?
[[ $rc == 2 ]] && exit 2
exit 0
