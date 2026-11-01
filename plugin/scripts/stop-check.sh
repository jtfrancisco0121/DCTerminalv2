#!/usr/bin/env bash
# Stop / SubagentStop guardrail (A3). When an Implementer sub-agent of the bound Eagle-Eye run
# (DCV2_RUN_DIR) is about to finish, run its PR's check in its worktree (30 s timeout). If the
# check fails, exit 2 with trimmed findings so the Implementer fixes them before it replies.
#
# Never blocks: the main session's Stop, other agents, standalone sessions (no DCV2_RUN_DIR),
# a second stop in a row (stop_hook_active), DCV2_STOP_HOOKS=off, a PR it cannot identify, a PR
# not building/fixing, a missing worktree, a check slower than the timeout
# (remembered per PR in <run>/logs/.stop-check.json, then skipped), or any error of its own.
# Full output of a failing check: <run>/logs/<PR>-stop-check.txt.
# Slop check: the check yields one outcome (pass, absent = no check command, slow = cached slow or timed out now,
# fail). Slop findings (slop-patterns.tsv, lines the PR added since its base commit, committed or not) are reported
# for pass, absent and slow; a failing check is reported alone. DCV2_SLOP_CHECK=off turns slop off.
# What runs is never taken from status.json (the orchestrator writes it): the command is the
# check of the PR's repo in <run>/repos.json (the app's input, matched by real path; no repos.json,
# no entry or no check → skip), and it runs in the PR's canonical worktree
# (<repo parent>/<repo>-worktrees/dcv2-<runId>-<pr>), which must also be the one .guard.json
# records (state.sh worktree). Anything that does not match → skip.
# DCV2_STOP_CHECK_TIMEOUT (seconds, default 30) is for tests.
input="$(cat)"
[[ "${DCV2_STOP_HOOKS:-on}" == "off" ]] && exit 0
[[ -n "${DCV2_RUN_DIR:-}" ]] || exit 0
# The one Python picker (lib/python.sh, next to this script); no Python 3.8+ means no check, as before.
_cdir="${BASH_SOURCE[0]%/*}"; [[ "$_cdir" != "${BASH_SOURCE[0]}" ]] || _cdir="."
. "$_cdir/lib/python.sh" 2>/dev/null || exit 0
(( ${#DCV2_PY[@]} > 0 )) || exit 0
_clib="$(cd "$_cdir/lib" 2>/dev/null && pwd -P)" || exit 0
# The absolute path of this bash for the check (never a bare `bash` on Windows): cygpath -w there, as is elsewhere.
_cbash="$BASH"; if command -v cygpath >/dev/null 2>&1; then _cbash="$(cygpath -w "$BASH" 2>/dev/null || echo "$BASH")"; fi

IFS= read -r -d '' PY <<'PY' || true
import json, os, re, subprocess, sys, tempfile, time

sys.dont_write_bytecode = True  # no __pycache__ inside the plugin folder (the installed app bundle is sealed)
sys.path.insert(0, os.environ["DCV2_LIB_DIR"])  # set by stop-check.sh itself, from its own path
import dcv2_portable as portable

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
        wt = str(p.get("worktree") or "").rstrip("/")
        if wt and re.search(re.escape(wt) + r"(?=$|[/\s\"'`,;)\]])", text or ""):
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
try:
    repo = os.path.realpath(str(pr.get("repoPath") or ""))
    if not pr.get("repoPath") or not os.path.isabs(pr["repoPath"]):
        sys.exit(0)
    wt = os.path.join(os.path.dirname(repo), os.path.basename(repo) + "-worktrees",
                      f"dcv2-{os.path.basename(run)}-{str(pid).lower()}")
    guard = json.load(open(os.path.join(run, ".guard.json")))
    if os.path.realpath(str(guard.get("runDir") or "")) != run or \
            os.path.realpath(str((guard.get("worktrees") or {}).get(pid) or "")) != wt or \
            os.path.realpath(str(pr.get("worktree") or "")) != wt or not os.path.isdir(wt):
        sys.exit(0)
    cwd = os.path.realpath(wt)
    if not (cwd == wt or cwd.startswith(wt + os.sep)):
        sys.exit(0)
except SystemExit:
    raise
except Exception:
    sys.exit(0)
# The check command: repos.json only (the app's input), matched by real path. None = absent (no command).
cmd = None
try:
    entries = json.load(open(os.path.join(run, "repos.json"))).get("repos") or []
    entry = next((e for e in entries if isinstance(e, dict) and isinstance(e.get("path"), str)
                  and os.path.isabs(e["path"]) and os.path.realpath(e["path"]) == repo), None)
    c = entry.get("check") if entry else None
    cmd = c if isinstance(c, str) and c.strip() else None
except Exception:
    cmd = None

logs = os.path.join(run, "logs")
cache_path = os.path.join(logs, ".stop-check.json")
try:
    cache = json.load(open(cache_path))
    cache = cache if isinstance(cache, dict) else {}
except Exception:
    cache = {}


def save_cache():
    os.makedirs(logs, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".stop-check.json.", suffix=".tmp", dir=logs)
    with os.fdopen(fd, "w") as f:
        json.dump(cache, f)
    portable.replace(tmp, cache_path)


# ---- slop: the lines this PR added that match slop-patterns.tsv ---------------------------------
SLOP_MAX = 30
DOC_EXT = (".md", ".mdx", ".txt", ".rst")
LOCKS = {"package-lock.json", "yarn.lock", "pnpm-lock.yaml", "npm-shrinkwrap.json", "cargo.lock", "go.sum", "poetry.lock",
         "pipfile.lock", "composer.lock", "gemfile.lock", "uv.lock", "bun.lockb", "bun.lock"}
EXCLUDED = ("plugin/scripts/slop-patterns.tsv", "fixtures/slop/", "plugin/bmad/")


def slop_rules():
    rules = []
    path = os.path.join(os.path.dirname(os.path.abspath(sys.argv[1])), "slop-patterns.tsv")  # next to this script
    for line in open(path, encoding="utf-8"):
        if not line.strip() or line.startswith("#"):
            continue
        parts = line.rstrip("\n").split("\t")
        if len(parts) < 4:
            continue
        try:
            rules.append((parts[0], parts[1], re.compile(parts[2]), parts[3]))
        except re.error:
            continue
    return rules


def in_scope(scope, path):
    low = path.lower()
    if scope == "*":
        return True
    if scope == "code":
        return not low.endswith(DOC_EXT)
    if scope.startswith("ext:"):
        spec, _, prefix = scope[4:].partition("@")
        return low.endswith(tuple("." + e.lower() for e in spec.split(",") if e)) and path.startswith(prefix)
    return False


def excluded(path):
    return path.startswith(EXCLUDED) or path.rsplit("/", 1)[-1].lower() in LOCKS or path.lower().endswith(".lock")


def added_lines(wt, base):
    """{(path, line number): text} of the added lines: `git diff --unified=0 <base>...HEAD` plus `git diff --unified=0 HEAD`."""
    found = {}
    for rng in ([f"{base}...HEAD"] if base else []) + ["HEAD"]:
        r = subprocess.run(["git", "-C", wt, "-c", "core.quotepath=off", "-c", "core.fsmonitor=false", "diff", "--no-color",
                            "--no-ext-diff", "--no-renames", "--unified=0", rng, "--"], capture_output=True, text=True,
                           errors="replace", timeout=30)
        if r.returncode != 0:
            continue
        path, ln = None, 0
        for line in r.stdout.splitlines():
            if line.startswith("+++ "):
                path = line[6:] if line.startswith("+++ b/") else None
            elif line.startswith("@@"):
                m = re.match(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@", line)
                ln = int(m.group(1)) if m else 0
            elif line.startswith("+") and path and not line.startswith("+++"):
                found[(path, ln)] = line[1:]
                ln += 1
    return found


def slop_base(wt, pr):
    try:
        b = (json.load(open(os.path.join(run, ".guard.json"))).get("bases") or {}).get(pid)
    except Exception:
        b = None
    if isinstance(b, str) and re.fullmatch(r"[0-9a-f]{7,40}", b):
        return b
    ref = str(pr.get("base") or "")
    for cand in ([f"origin/{ref}", ref] if re.fullmatch(r"[\w./-]+", ref) else []) + ["origin/HEAD", "main", "master"]:
        r = subprocess.run(["git", "-C", wt, "merge-base", "HEAD", cand], capture_output=True, text=True)
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip()
    return None


def slop_findings():
    rules = slop_rules()
    out = []
    for (path, ln), text in added_lines(cwd, slop_base(cwd, pr)).items():
        if excluded(path) or len(text) > 2000:
            continue
        for rid, scope, rx, msg in rules:
            if in_scope(scope, path) and rx.search(text):
                out.append((path, ln, rid, msg))
    return sorted(out)


def report_slop(note):
    found = slop_findings()
    if not found:
        return
    shown = [f"  {p}:{n}: [{rid}] {msg}" for p, n, rid, msg in found[:SLOP_MAX]]
    more = f"\n  [... {len(found) - SLOP_MAX} more]" if len(found) > SLOP_MAX else ""
    print(f"dcv2 stop check: slop in lines the {pid} change added ({len(found)} finding(s)){note}. Remove a line only if it is "
          f"leftover debugging or filler; keep real output (CLI output, logging a program is meant to print). Then commit and "
          f"re-run the check.\n" + "\n".join(shown) + more, file=sys.stderr)
    sys.exit(2)


slop_on = os.environ.get("DCV2_SLOP_CHECK", "on") != "off"
limit = float(os.environ.get("DCV2_STOP_CHECK_TIMEOUT") or 30)
outcome = "absent"
if cmd:
    if (cache.get(pid) or {}).get("slow") and cache[pid].get("command") == cmd:
        outcome = "slow"
    else:
        start = time.time()
        proc = portable.spawn_group(portable.check_argv(cmd), cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    stdin=subprocess.DEVNULL)
        try:
            out, _ = proc.communicate(timeout=limit)
            outcome = "pass" if proc.returncode == 0 else "fail"
        except subprocess.TimeoutExpired:
            portable.kill_group(proc)
            proc.communicate()
            cache[pid] = {"slow": True, "command": cmd, "seconds": limit}
            save_cache()
            outcome = "slow"
        if outcome != "slow":
            cache[pid] = {"slow": False, "command": cmd, "seconds": round(time.time() - start, 1)}
            save_cache()

if outcome != "fail":
    if slop_on:
        try:
            report_slop(" (check not run in the hook (slow))" if outcome == "slow" else
                        " (no check command for this repo)" if outcome == "absent" else "")
        except SystemExit:
            raise
        except Exception:
            pass  # never blocks on its own errors
    sys.exit(0)

text = out.decode("utf-8", "replace")
os.makedirs(logs, exist_ok=True)
full = os.path.join(logs, f"{pid}-stop-check.txt")
try:  # never through a link planted at the log's name
    with os.fdopen(os.open(full, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0), 0o644), "w") as f:
        f.write(text)
except OSError as e:
    full = f"(not written: {e.strerror})"
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

DCV2_LIB_DIR="$_clib" DCV2_BASH_EXE="$_cbash" "${DCV2_PY[@]}" -c "$PY" "$0" <<<"$input"
rc=$?
[[ $rc == 2 ]] && exit 2
exit 0
