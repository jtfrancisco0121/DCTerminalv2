#!/usr/bin/env bash
# PreToolUse guard for the eagle-eye run (plugin hook: Edit, Write, NotebookEdit, MultiEdit, Bash, Monitor, PowerShell, Read, Grep, Glob, mcp__*).
#
# The run it guards is the one named by the env DCV2_RUN_DIR, set when claude is launched
# (the app and the e2e test set it; `state.sh preflight` refuses to start a run without it).
# Hooks inherit it from the claude process, so the orchestrator cannot change it and ticket
# text cannot reach it. The run's allowed worktrees are listed in <run>/.guard.json, which
# only `state.sh worktree` writes.
#
# Pipeline sub-agents (agent_type implementer, planner, plan-reviewer, risk-reviewer, pr-reviewer, qa, web-qa, memory-updater,
# with or without the "dcv2:" prefix):
#   - Edit/Write: the real path (symlinks and .. resolved) must be inside the run folder, or,
#     for the implementer only, inside one of the run's recorded worktrees. The qa agent may write only its own
#     result files: reviews/PR<n>-qa<r>.md, reviews/qa/<name>.png and logs/PR<n>-qa<r>*.txt in the run folder.
#   - Bash: never `git push`, `gh pr`, `gh api` (or gh issue/repo/release writes). Git commands
#     that change anything are allowed only for the implementer inside a recorded worktree;
#     elsewhere only read-only git. Redirects, tee, sed -i, cp/mv/rm/touch/mkdir etc. into a
#     git checkout (a repo's main checkout, another worktree) are blocked unless the target is
#     an allowed place. Best effort: commands that write through other programs (node, python,
#     npm scripts, xargs) are not seen.
#   - No DCV2_RUN_DIR, no Python 3.8+, or a payload that does not parse: blocked (fail closed).
#     Exceptions: the memory-updater also runs at the end of a standalone /dcv2:developer session,
#     and the risk-reviewer in the standalone planner / codebase-audit / recommendation sessions
#     (no run): then they may not Edit/Write anything (the memory-updater writes memory only with
#     memory.sh write; the risk-reviewer replies with its review).
# The orchestrator (no agent_type) in a bound run: local review mode (meta.json reviewMode missing
# or "local"): `git push` and `gh pr create` only when status.json has stage shipping and a
# shipApproval (state.sh ship-pr does both itself). github mode: no direct `git push` / `gh pr
# create` at all; `state.sh draft-pr|push|post-review|comment` only after the plan approval and
# `state.sh ready` only after the ship approval (state.sh checks the same). Either mode: a push
# names only dcv2/ branches; never a force push, --all/--mirror or a branch delete; no GitHub-writing
# gh (`gh api` with a non-GET method or -f/-F/--field/--raw-field/--input, `gh pr merge|close|edit|
# ready|comment|...`, `gh <anything> create|comment|edit|delete|...`). It never writes into (or
# runs changing git in) a repo's main checkout recorded in .guard.json.
# Implementer: `git add|commit` naming a file state.sh worktree copied in from the main checkout
# (.guard.json "copied", from repos.json copyFiles) is blocked (best effort).
# Web QA (docs/run-folder.md, "Web QA"): the web-qa agent writes only webqa/results.md, webqa/shots/<name>.png and
# webqa/steps/<name>.jsonl in the run folder; its Bash is limited to the driver (qa-browser.mjs with --out/--steps/--timeout), `state.sh webqa <run>`
# and ls / mkdir -p under <run>/webqa (rule webqa-command); its Read, Grep and Glob stay in the run and plugin folders (webqa-read-scope).
# webqa-secret.json (the test account's password, written by the app) is refused to EVERY agent in EVERY session (webqa-secret): a Read of it
# by name, a Grep or Glob naming it or rooted at the runs folder / a parent / a run holding it, and a shell command naming it (also through
# a ? * [] glob) or recursively reading a tree that holds it (grep -r, rg, ag, find, tar, zip, rsync, cp -r). Best effort: an interpreter that
# builds the name at run time evades text checks, which is why the account is a test account only (docs/run-folder.md).
# Everyone in a run: no `security dump-keychain|find-generic-password|find-internet-password|export`.
# Sub-agents never run state.sh's GitHub commands; the Implementer never changes git remotes/config.
# BMAD (bmad/ in the plugin, <run>/bmad in the run): in a bound run only the planner may run
# `node .../bmad/runtime.mjs` (the wrapper itself refuses writes outside <run>/bmad); any command
# naming ade-runtime.mjs (the bundle behind the wrapper) is blocked for everyone; <run>/bmad/_bmad/
# config*.toml and _bmad/custom/ are written only by `state.sh bmad-init`, never with Edit/Write or
# a shell write. Repo and worktree rules are unchanged.
# Cleanup (0.5.0): the orchestrator never runs `git worktree <anything but list>` or a changing `git branch`
# anywhere in a bound run (and never `git push --delete`): `state.sh cleanup` does it, at done / failed only.
# <run>/requests/ is written only by `state.sh request` and the app; app-cleanup.json and app-github.json only by the app.
# Live tests: a pipeline sub-agent never runs `claude`; `scripts/skill-e2e/golden.sh` / `bmad_spike.sh` (which
# launch claude) only the implementer or pr-reviewer runs, inside its recorded worktree, when repos.json gives
# that repo `liveTests: true` (the app setting "Allow live Claude tests in checks"). Best effort, like the rest.
# Interpreters (python/node/perl/ruby `-c`/`-e`/`-p` code, a heredoc or here-string fed to one): in a bound run the code may not
# name a protected run file (path-aware, like Edit/Write and shell writes) or the run's signed records; the orchestrator's inline
# code may not contain a write primitive (open(..,'w'), write_text, writeFile, os.system, subprocess, ...). A shell or interpreter
# never reads its program from a pipe (`| bash`, `| sh -s`, `| python3`, `| python3 -`), /dev/stdin or `<(...)`, and never runs a
# file the same command wrote. `... | python3 -c '<code that reads stdin as data>'` stays allowed. Best effort, as above.
# Everyone: status.json, writeback.json and .guard.json in a run folder are written only by
# state.sh. When DCV2_MEMORY_DIR is set (every dcv2 launch), nobody writes into it with
# Edit/Write or a shell redirect/copy: agents use memory.sh write (the app's editor writes as the
# user, outside Claude). Sessions with no DCV2_RUN_DIR and no pipeline agent are not affected otherwise.
# ClickUp tickets (docs/skills.md, "ClickUp helper"): a command naming a file ending in clickup.sh or clickup.py (any path) and the
# word `create` is blocked in a bound run; standalone it gets a PreToolUse `ask` when the payload's permission_mode shows prompts
# (default, acceptEdits, plan) and is blocked in any other or missing mode (the user types it with `!`). A block of any other rule wins.
# Every block in a bound run appends one line to <run>/logs/guard.log (docs/run-folder.md): UTC, rule id (the RULES table),
# tool, agent id (main), tool_use_id (-), tab separated. Never command text, paths or the message; a write failure changes
# nothing. logs/guard.log is protected like the other run records. The three blocks made in bash before Python starts
# (no Python, a payload that does not parse) cannot be logged.
# Exit 2 blocks the tool call (stderr is shown to the model).
input="$(cat)"
PIPE_RE='"agent_type"[[:space:]]*:[[:space:]]*"(dcv2:)?(implementer|planner|plan-reviewer|risk-reviewer|pr-reviewer|qa|web-qa|memory-updater)"'

# The plugin folder this script lives in (pure bash: no dirname, the PATH may be broken).
gdir="${BASH_SOURCE[0]%/*}"; [[ "$gdir" != "${BASH_SOURCE[0]}" ]] || gdir="."
GUARD_PLUGIN="$(cd "$gdir/.." 2>/dev/null && pwd -P)" || GUARD_PLUGIN=""

fallback() { # no working Python 3.8+, or the payload did not parse: fail closed where it matters
  if grep -qiF -- "webqa-secret" <<<"$input"; then  # the test account's password file: refused in every session
    echo "Blocked by the dcv2 guard: this call names webqa-secret and could not be checked (install Python 3.8+ (tried python3, python, py -3)); the test account's password file is never read by an agent." >&2
    exit 2
  fi
  if grep -Eq "$PIPE_RE" <<<"$input"; then
    echo "Blocked by the dcv2 guard: this call could not be checked (install Python 3.8+ (tried python3, python, py -3), or an unreadable hook payload)." >&2
    exit 2
  fi
  if [[ -n "${DCV2_RUN_DIR:-}" ]]; then  # a bound run: nothing is let through unchecked
    echo "Blocked by the dcv2 guard: this call could not be checked (install Python 3.8+ (tried python3, python, py -3), or an unreadable hook payload) and a run is bound." >&2
    exit 2
  fi
  if grep -Eq '"tool_name"[[:space:]]*:[[:space:]]*"(Read|Grep|Glob)"' <<<"$input"; then  # plain reads in an unbound tab: as when they were not hooked
    exit 0
  fi
  local p
  for p in "${DCV2_MEMORY_DIR:-}" "${CLAUDE_PLUGIN_ROOT:-}" "$GUARD_PLUGIN"; do  # a dcv2 tab: memory and the plugin stay closed
    if [[ -n "$p" ]] && grep -qiF -- "$p" <<<"$input"; then
      echo "Blocked by the dcv2 guard: this call names the codebase memory or the plugin folder and could not be checked (install Python 3.8+ (tried python3, python, py -3))." >&2
      exit 2
    fi
  done
  exit 0
}

# The one Python picker (lib/python.sh, from this script's own folder): DCV2_PY is empty when no Python 3.8+ works.
. "$gdir/lib/python.sh" 2>/dev/null || DCV2_PY=()
_glib="$(cd "$gdir/lib" 2>/dev/null && pwd -P)" || DCV2_PY=()
(( ${#DCV2_PY[@]} > 0 )) || fallback

IFS= read -r -d '' PY <<'PY' || true
import datetime, json, os, re, shlex, sys

sys.dont_write_bytecode = True  # no __pycache__ inside the plugin folder (the installed app bundle is sealed)
sys.path.insert(0, os.environ["DCV2_LIB_DIR"])  # set by guard.sh itself, from its own path
import dcv2_portable as portable

try:
    data = json.loads(sys.stdin.read())
    if not isinstance(data, dict):
        raise ValueError("not an object")
except Exception:
    sys.exit(3)

ROLES = ("implementer", "planner", "plan-reviewer", "risk-reviewer", "pr-reviewer", "qa", "web-qa", "memory-updater")
# Sub-agents that also run in a standalone session (no run); there they write nothing with Edit/Write.
UNBOUND_OK = ("memory-updater", "risk-reviewer")
agent = str(data.get("agent_type") or "")
role = agent[len("dcv2:"):] if agent.startswith("dcv2:") else agent
pipeline = role in ROLES
tool = str(data.get("tool_name") or "")
inp = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
cwd0 = str(data.get("cwd") or "/")

def norm(p):
    """The identity of a path for every allow / deny decision (comparison only, never opened or printed): POSIX is the real
    path (symlinks and .. resolved), casefolded (the default macOS volume ignores case, so STATUS.json is status.json and
    /Users/X is /Users/x); Windows adds Git Bash path spellings (portable.ident). The null device has no identity (""),
    and a path that cannot be resolved has the unique UNRESOLVED identity, which `protected` refuses."""
    ident = portable.ident(p) if p else ""
    return "" if ident is None else ident


def same(a, b):
    return bool(a) and bool(b) and norm(a) == norm(b)


run = os.environ.get("DCV2_RUN_DIR") or ""
run = portable.real_path(run) if run else ""
# The app's signed ClickUp records of each run: <app data>/run-trust (the run is <app data>/runs/<id>).
trust_root = os.path.join(os.path.dirname(os.path.dirname(run)), "run-trust") if run else ""
TRUST_MSG = "the app's run records (run-trust) are written only by the app, never by Claude."
mem = os.environ.get("DCV2_MEMORY_DIR") or ""
mem = portable.real_path(os.path.expanduser(mem)) if mem else ""
worktrees, repos, status, copied, mode = [], [], {}, set(), "local"
if run:
    try:
        g = json.load(open(os.path.join(run, ".guard.json")))
        if norm(str(g.get("runDir") or "")) == norm(run):
            wmap = {k: portable.real_path(w) for k, w in (g.get("worktrees") or {}).items() if isinstance(w, str) and os.path.isabs(w)}
            worktrees = list(wmap.values())
            repos = [portable.real_path(r) for r in g.get("repos") or [] if isinstance(r, str) and os.path.isabs(r)]
            for k, files in (g.get("copied") or {}).items():  # files copied in from the main checkout (never committed)
                if k in wmap and isinstance(files, list):
                    copied.update(norm(os.path.join(wmap[k], str(f))) for f in files)
    except Exception:
        pass
    try:
        mode = "github" if json.load(open(os.path.join(run, "meta.json"))).get("reviewMode") == "github" else "local"
    except Exception:
        mode = "local"
    try:
        status = json.load(open(os.path.join(run, "status.json")))
        status = status if isinstance(status, dict) else {}
    except Exception:
        status = {}
# Written only by state.sh (status, writeback, guard, waves.json) or only by the app
# (the ClickUp snapshot and records, meta.json, the app's own check results).
PROTECTED = ("status.json", "writeback.json", ".guard.json",
             "tickets.json", "meta.json", "repos.json", "writeback-posted.json", "tickets-pulls.json", "tickets-update.md", "app-checks.json", "app-usage.json",
             "app-cleanup.json", "app-github.json", "app-receipt.json", "waves.json", "triage.json", "guard.log",
             "webqa-target.json", "webqa-secret.json", "webqa.json", "webqa-approval.json", "webqa-posted.json")
who = agent or "the orchestrator"
where = "your worktree or the run folder" if role == "implementer" else "the run folder"
QA_WRITE = re.compile(r"^(reviews/PR\d+-qa\d\.md|reviews/qa/[A-Za-z0-9][A-Za-z0-9._-]*\.png|logs/PR\d+-qa\d[A-Za-z0-9._-]*\.txt)$")
if role == "qa":
    where = "the run folder, and there only reviews/PR<n>-qa<r>.md, reviews/qa/<name>.png and logs/PR<n>-qa<r>*.txt"
WEBQA_WRITE = re.compile(r"^(webqa/results\.md|webqa/shots/[A-Za-z0-9][A-Za-z0-9._-]*\.png|webqa/steps/[A-Za-z0-9][A-Za-z0-9._-]*\.jsonl)$")
if role == "web-qa":
    where = "the run folder, and there only webqa/results.md, webqa/shots/<name>.png and webqa/steps/<name>.jsonl"
SECRET_NAME = "webqa-secret.json"


# Every block has one rule id from this table (the kebab-case key). The id, never the message, is what logs/guard.log records.
RULES = {
    "no-run-bound": "a pipeline agent runs with no bound run",
    "no-file-path": "Edit/Write without a file path",
    "no-command": "a shell tool without a command text",
    "run-trust": "the app's signed run records",
    "run-file-write": "a protected run file written by hand",
    "bmad-config": "BMAD config written by hand",
    "git-user-config": "the user's git config",
    "memory-write": "codebase memory written without memory.sh",
    "no-write-zone": "the plugin, Claude's config or app data",
    "main-checkout-write": "a repo main checkout written by the orchestrator",
    "memory-updater-write": "the memory updater wrote outside a run",
    "risk-reviewer-write": "the risk reviewer wrote outside a run",
    "outside-allowed-write": "a write outside the worktree or run folder",
    "unknown-write-target": "a write whose target cannot be told",
    "mcp-tool": "a sub-agent called an MCP tool",
    "mcp-write-verb": "the orchestrator called a non-read MCP tool",
    "powershell": "PowerShell in a run",
    "ship-gate": "a ship step before the ship approval",
    "git-worktree": "git worktree outside state.sh",
    "git-ref-change": "a git command that changes shared refs",
    "git-notes": "git notes",
    "git-fetch-branch": "git fetch into a local branch",
    "git-branch": "git branch changes",
    "git-config-flag": "git -c / --config-env / --exec-path",
    "git-remote-config": "git remote or config changes",
    "git-env": "an environment variable that redirects git",
    "git-outside-worktree": "a changing git command outside a worktree",
    "main-checkout-git": "changing git in a repo main checkout",
    "copied-file-commit": "committing a file copied from the main checkout",
    "push-forbidden": "a push by a sub-agent or outside shipping",
    "push-force": "a force push or remote branch delete",
    "push-target": "a push from an unknown folder",
    "push-multi": "a push of more than one branch",
    "push-ref": "a push of a non dcv2/ branch",
    "push-github-mode": "a direct push in a GitHub review run",
    "gh-subagent": "gh from a sub-agent",
    "gh-write": "a GitHub-writing gh command",
    "gh-api-write": "gh api write",
    "gh-pr-write": "gh pr merge / close / edit",
    "gh-pr-create": "gh pr create outside state.sh",
    "state-github-subagent": "state.sh GitHub command from a sub-agent",
    "state-github-local": "state.sh GitHub command in a local review run",
    "state-plan-approval": "state.sh GitHub command before the plan approval",
    "state-ship-approval": "state.sh ready before the ship approval",
    "state-other-run": "state.sh on another run folder",
    "bmad-runtime": "the BMAD runtime outside the planner",
    "live-test": "live Claude tests",
    "claude-launch": "launching claude",
    "env-unknown": "a variable assignment whose name is unknown",
    "env-setting": "a setting the guard and plugin trust",
    "env-parse": "an env -S text that does not parse",
    "keychain": "reading the keychain",
    "interpreter-write": "inline interpreter code that writes or names a protected file",
    "write-then-run": "a command that writes a file and runs it",
    "worktree-discard": "discarding a run worktree",
    "pipe-program": "a program read from a pipe",
    "stdin-program": "a program read from stdin",
    "process-substitution": "a program from <(...)",
    "nesting-depth": "command nesting too deep",
    "parse-error": "a command that does not parse",
    "test-remotes": "the test-suite remote override",
    "webqa-command": "a web-qa shell command outside the driver, state.sh webqa and ls / mkdir under webqa/",
    "webqa-secret": "the web QA test account's password file",
    "webqa-read-scope": "a web-qa read outside the run and plugin folders",
    "clickup-create": "creating ClickUp tickets without a permission prompt",
}


def log_block(rule):
    """Append one line to <run>/logs/guard.log: UTC, rule id, tool, agent id (main), tool_use_id (-), tab separated. Never the
    command, a path or the message. One O_APPEND|O_NOFOLLOW write under 512 bytes; any failure is swallowed (the block stays)."""
    try:
        if not run or not os.path.isdir(run):
            return

        def field(v, dflt):
            v = re.sub(r"[^A-Za-z0-9_.:-]", "", str(v or ""))[:100]
            return v or dflt

        logs = os.path.join(run, "logs")
        if os.path.islink(logs):
            return
        os.makedirs(logs, exist_ok=True)
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        line = "\t".join((stamp, rule, field(tool, "-"), field(data.get("agent_id"), "main"), field(data.get("tool_use_id"), "-"))) + "\n"
        fd = os.open(os.path.join(logs, "guard.log"), os.O_WRONLY | os.O_APPEND | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o644)
        try:
            os.write(fd, line.encode("ascii", "replace")[:511])
        finally:
            os.close(fd)
    except Exception:
        pass


def block(rule, msg):
    if rule not in RULES:
        raise ValueError("unknown guard rule id: " + repr(rule))
    log_block(rule)
    print("Blocked by the dcv2 guard: " + msg, file=sys.stderr)
    sys.exit(2)


def real(p, base):
    return portable.real_path(os.path.join(base, os.path.expanduser(p)))


def inside(p, root):
    if not root:
        return False
    p, root = norm(p), norm(root)
    return p == root or p.startswith(root.rstrip(os.sep) + os.sep)


def git_user_config(p):
    """The user's own git config (~/.gitconfig, $XDG_CONFIG_HOME/git/): the app's network git reads it (credential
    helpers run from there), so nothing in a run writes it."""
    home = portable.real_path(os.path.expanduser("~"))
    xdg = portable.real_path(os.environ.get("XDG_CONFIG_HOME") or os.path.join(home, ".config"))
    return same(p, os.path.join(home, ".gitconfig")) or inside(p, os.path.join(xdg, "git"))


GITCONFIG_MSG = "the user's git config (~/.gitconfig, ~/.config/git/) is never written from a dcv2 run."


PROTECTED_CF = frozenset(n.casefold() for n in PROTECTED)


def has_request_md(d):
    """d is a run folder: it holds a request.md (any case, as the volume may ignore case)."""
    try:
        return any(n.casefold() == "request.md" for n in os.listdir(d))
    except OSError:
        return False


def protected(p):
    """A protected run file (the names above in a run folder), or anything in a requests/ folder of one; a path that cannot
    be resolved is refused (fail closed)."""
    return portable.is_protected(p, norm(run), PROTECTED_CF, os.path, portable.ident, has_request_md)


def no_write_zone(p):
    """Bound runs (everyone, the orchestrator too) never write the plugin (its guard and hooks), Claude's own
    configuration, or the app's data outside the run folder (and, for the implementer, its worktrees)."""
    if not run:
        return None
    home = portable.real_path(os.path.expanduser("~"))
    plugin_roots = [r for r in (os.environ.get("CLAUDE_PLUGIN_ROOT") or "", GUARD_PLUGIN) if r]
    if any(inside(p, r) for r in plugin_roots):
        return "the dcv2 plugin (its guard, hooks and scripts)"
    cfg = os.environ.get("CLAUDE_CONFIG_DIR") or ""
    if inside(p, cfg) or (inside(p, home) and os.path.relpath(norm(p), norm(home)).split(os.sep)[0].startswith(".claude")):
        return "Claude's own configuration (~/.claude*)"
    roots = [os.path.dirname(os.path.dirname(run))] + ([os.path.dirname(mem)] if mem else [])
    if any(inside(p, r) for r in roots if r and norm(r) not in (norm(home), os.sep)) and not write_allowed(p):
        return "the app's data folder"
    return None


def bmad_config(p):
    """<run>/bmad/_bmad/config*.toml and _bmad/custom/: BMAD's config is made only by `state.sh bmad-init`."""
    if not run:
        return False
    b = os.path.join(run, "bmad", "_bmad")
    if same(os.path.dirname(p), b) and re.fullmatch(r"config[\w.-]*\.toml", os.path.basename(p), re.I):
        return True
    return inside(p, os.path.join(b, "custom"))


BMAD_CFG_MSG = "BMAD's config under <run>/bmad/_bmad is written only by `state.sh bmad-init`, never with Edit, Write or a shell write."


GUARD_PLUGIN = portable.real_path(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1] else ""


def write_allowed(p):
    if role in UNBOUND_OK and not run:
        return False
    if role in ("qa", "web-qa"):  # only its own result files, never the worktree or any other run file
        rp = real(p, "/")
        allow = QA_WRITE if role == "qa" else WEBQA_WRITE
        return bool(run) and inside(rp, run) and bool(allow.match(os.path.relpath(rp, real(run, "/")).replace(os.sep, "/")))
    if inside(p, run):
        return True
    return role == "implementer" and any(inside(p, w) for w in worktrees)


def main_checkout(p):
    """Inside a repo's main checkout recorded for this run (worktrees live beside the repo, not in it)."""
    return any(inside(p, r) for r in repos)


def in_checkout(p):
    if any(inside(p, r) for r in repos) or any(c.casefold().endswith("-worktrees") for c in p.split(os.sep)):
        return True
    d = p
    while True:
        if os.path.exists(os.path.join(d, ".git")):
            return True
        nd = os.path.dirname(d)
        if nd == d:
            return False
        d = nd


# ---- the web QA test account's password file: refused to everyone, in every session --------------------------------------
import glob as _glob

DEFAULT_RUNS = "~/Library/Application Support/com.jtfrancisco.dcterminalv2/runs"


def runs_folder():
    """The app's runs folder: the bound run's parent, else the codebase memory folder's sibling `runs`, else the default."""
    if run:
        return portable.real_path(os.path.dirname(run))
    if mem:
        return portable.real_path(os.path.join(os.path.dirname(mem), "runs"))
    return portable.real_path(os.path.expanduser(DEFAULT_RUNS))


_SECRET_RUNS = None


def secret_runs():
    """The run folders that hold webqa-secret.json (one scandir of the runs folder, done once and only when a rule needs it)."""
    global _SECRET_RUNS
    if _SECRET_RUNS is None:
        _SECRET_RUNS = []
        try:
            with os.scandir(runs_folder()) as it:
                for e in it:
                    try:
                        if e.is_dir(follow_symlinks=False) and os.path.lexists(os.path.join(e.path, SECRET_NAME)):
                            _SECRET_RUNS.append(portable.real_path(e.path))
                    except OSError:
                        pass
        except OSError:
            pass
    return _SECRET_RUNS


def hits_secret_tree(path):
    """While a secret exists: `path` is the runs folder, a parent of it, or a run folder holding the secret. A folder below a
    run (webqa/, reviews/ ...) is not."""
    if not path or not secret_runs():
        return False
    rf = runs_folder()
    p, r = norm(path), norm(rf)
    if p == r or r.startswith(p.rstrip(os.sep) + os.sep):
        return True
    return any(p == norm(s) for s in secret_runs())


SECRET_MSG = ("webqa-secret.json is the web QA test account's password file: no agent reads, searches or lists it, in any session. "
              "The driver reads it itself.")
SECRET_STEM = SECRET_NAME.rsplit(".", 1)[0]


def expand_vars(text):
    """$DCV2_RUN_DIR / ${DCV2_RUN_DIR}, CLAUDE_PLUGIN_ROOT and DCV2_MEMORY_DIR written out (as the shell will)."""
    for name, value in (("DCV2_RUN_DIR", run), ("CLAUDE_PLUGIN_ROOT", os.environ.get("CLAUDE_PLUGIN_ROOT") or GUARD_PLUGIN),
                        ("DCV2_MEMORY_DIR", mem)):
        if value:
            text = re.sub(r"\$\{" + name + r"\}|\$" + name + r"\b", lambda _m, v=value: v, text)
    return text


def glob_hits_secret(token, cwd):
    """A shell operand with * ? or [ : expand it on the file system and say whether it matches a webqa-secret.json."""
    pattern = os.path.expanduser(token)
    if not os.path.isabs(pattern):
        pattern = os.path.join(cwd or cwd0, pattern)
    seen = 0
    try:
        for hit in _glob.iglob(pattern):
            if os.path.basename(hit).casefold() == SECRET_NAME:
                return True
            seen += 1
            if seen > 20000:
                break
    except (OSError, ValueError):
        return False
    return False


def words_of(text):
    try:
        return lex(text)
    except ValueError:
        return text.split()


READERS_ALWAYS = ("rg", "ag", "find", "tar", "zip", "rsync")


def secret_command_check(cmd, cwd, depth=0):
    """Bash, Monitor and PowerShell, every agent: refuse a command that names the secret file (also after the run-folder
    variables are written out, and through a * ? [] glob), or that recursively reads a tree that holds one."""
    text = expand_vars(cmd)
    if SECRET_STEM in text.casefold():
        block("webqa-secret", SECRET_MSG)
    toks = words_of(text)
    for t in toks:
        if not is_punct(t) and re.search(r"[*?\[]", t) and not t.startswith("-") and glob_hits_secret(t, cwd):
            block("webqa-secret", SECRET_MSG)
    segs, seg = [], []
    for t in toks:
        if is_punct(t):
            if seg:
                segs.append(seg)
            seg = []
        else:
            seg.append(t)
    if seg:
        segs.append(seg)
    for words in segs:
        words = unwrap(list(words))
        if not words:
            continue
        name, args = re.sub(r"\.exe$", "", os.path.basename(words[0]), flags=re.I), words[1:]
        if name in SHELLS and shell_c(args) is not None and depth < 3:
            secret_command_check(shell_c(args), cwd, depth + 1)
            continue
        nonopt = [a for a in args if a and not a.startswith("-")]
        recursive = False
        if name in ("grep", "egrep", "fgrep"):
            recursive = any(a in ("--recursive", "--dereference-recursive") or re.match(r"^-[A-Za-z]*[rR][A-Za-z]*$", a) for a in args)
        elif name == "cp":
            recursive = any(a == "--recursive" or re.match(r"^-[A-Za-z]*[rRa][A-Za-z]*$", a) for a in args)
        elif name in READERS_ALWAYS:
            recursive = True
        if not recursive or not secret_runs():
            continue
        operands = list(nonopt)
        if name in ("grep", "egrep", "fgrep", "rg", "ag") and len(nonopt) <= 1:
            operands.append(cwd or cwd0)  # no path: the working folder
        for o in operands:
            o = os.path.expanduser(expand_vars(o))
            if "$" in o:
                block("webqa-secret", SECRET_MSG + " (A recursive search of a path that cannot be told apart from the runs folder is refused while a test password exists.)")
            if re.search(r"[*?\[]", o):
                base = re.split(r"[*?\[]", o, 1)[0]
                if hits_secret_tree(real(os.path.dirname(base) or ".", cwd or cwd0)) or glob_hits_secret(o, cwd):
                    block("webqa-secret", SECRET_MSG)
                continue
            if hits_secret_tree(real(o, cwd or cwd0)):
                block("webqa-secret", SECRET_MSG)


def search_roots(cwd_):
    """The folders a Grep or Glob call searches: its `path` (default: the cwd), and for Glob the fixed prefix of an absolute pattern."""
    roots = [real(str(inp.get("path")), cwd_) if inp.get("path") else real(cwd_, "/")]
    pat = str(inp.get("pattern") or "") if tool == "Glob" else ""
    if os.path.isabs(os.path.expanduser(pat)):
        fixed = re.split(r"[*?\[{]", os.path.expanduser(pat), 1)[0]
        roots.append(real(fixed.rsplit("/", 1)[0] or "/", "/"))
    return roots


def read_search_check():
    """Read, Grep and Glob (hooked since the web QA release): the secret rules for everyone, and the folder scope of web-qa."""
    if tool == "Read":
        target = real(str(inp.get("file_path") or ""), cwd0) if inp.get("file_path") else ""
        if target and os.path.basename(target).casefold() == SECRET_NAME:
            block("webqa-secret", SECRET_MSG)
        scope_roots = [target] if target else []
    else:
        names = [str(inp.get(k) or "") for k in ("pattern", "glob")]
        if any(SECRET_STEM in n.casefold() for n in names):
            block("webqa-secret", SECRET_MSG)
        if inp.get("path") and os.path.basename(real(str(inp["path"]), cwd0)).casefold() == SECRET_NAME:
            block("webqa-secret", SECRET_MSG)
        scope_roots = search_roots(cwd0)
        for r in scope_roots:
            if hits_secret_tree(r):
                block("webqa-secret", SECRET_MSG + " (A search rooted at the runs folder, a parent of it, or a run holding it is refused while a test password exists; search below a run's webqa/ folder instead.)")
    if role == "web-qa":
        if not run:
            block("no-run-bound", f"{who} runs only inside an Eagle-Eye run, and none is bound (DCV2_RUN_DIR is not set at launch).")
        plugin_roots = [r for r in (os.environ.get("CLAUDE_PLUGIN_ROOT") or "", GUARD_PLUGIN) if r]
        for r in scope_roots:
            if not (inside(r, run) or any(inside(r, q) for q in plugin_roots)):
                block("webqa-read-scope", f"{who} reads only inside the run folder and the plugin folder, not {r}.")


# ---- Read / Grep / Glob ------------------------------------------------------------------
# Never hooked before the web QA release. Every other call behaves as it did when they were not hooked (exit 0).
if tool in ("Read", "Grep", "Glob"):
    read_search_check()
    sys.exit(0)

if pipeline and not run and role not in UNBOUND_OK and tool not in ("Read", "Grep", "Glob"):
    block("no-run-bound", f"{who} runs only inside an Eagle-Eye run, and none is bound (DCV2_RUN_DIR is not set at launch).")

# ---- Edit / Write ------------------------------------------------------------------------
if tool in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
    path = inp.get("file_path") or inp.get("notebook_path") or ""
    if not path:
        if pipeline:
            block("no-file-path", f"{tool} without a file path")
        sys.exit(0)
    p = real(str(path), cwd0)
    if inside(p, trust_root):
        block("run-trust", TRUST_MSG)
    if protected(p):
        block("run-file-write", "write run-folder state only with scripts/state.sh (put / approve / writeback / worktree), never with Edit or Write.")
    if bmad_config(p):
        block("bmad-config", BMAD_CFG_MSG)
    if (run or pipeline) and git_user_config(p):
        block("git-user-config", GITCONFIG_MSG)
    if inside(p, mem):
        block("memory-write", "codebase memory is written only with `bash <plugin>/scripts/memory.sh write <repo id> <file> <<'EOF'` "
              "(it checks size caps, secrets and pinned lines), never with Edit or Write.")
    zone = no_write_zone(p)
    if zone:
        block("no-write-zone", f"{p} is {zone}: it is never written from a bound run.")
    if not pipeline and run and main_checkout(p):
        block("main-checkout-write", f"the orchestrator never edits a repo's main checkout ({p}); sub-agents work in the run's worktrees.")
    if not pipeline or write_allowed(p):
        sys.exit(0)
    if role == "memory-updater" and not run:
        block("memory-updater-write", "the memory updater writes nothing with Edit/Write outside a run; use memory.sh write for memory.")
    if role == "risk-reviewer" and not run:
        block("risk-reviewer-write", "the risk reviewer writes nothing outside a run; give the review in your reply.")
    block("outside-allowed-write", f"{who} may only write inside {where}, not {p}. Never edit a repo main checkout; use the exact worktree path you were given.")

bound = pipeline or bool(run)

# ---- MCP tools -----------------------------------------------------------------------------
# Their effects cannot be seen from here, so in a bound run a pipeline sub-agent calls none, and the orchestrator
# calls only read-verb tools (the last `__` segment starts with get / list / read / search / fetch / view / query /
# find / describe). Unbound sessions are not affected.
MCP_READ_RE = re.compile(r"^(get|list|read|search|fetch|view|query|find|describe)", re.I)
if tool.startswith("mcp__"):
    if pipeline:
        block("mcp-tool", f"{who} calls no MCP tool ({tool}): their effects cannot be checked.")
    if run and not MCP_READ_RE.match(tool.rsplit("__", 1)[-1]):
        block("mcp-write-verb", f"the orchestrator calls only read-verb MCP tools (get / list / read / search / fetch / view / query / find / "
              f"describe), not {tool}: their effects cannot be checked. Writes go through state.sh.")
    sys.exit(0)

# ---- Bash, Monitor, PowerShell: the shell text in tool_input.command ------------------------
if tool == "PowerShell" and bound:
    block("powershell", "PowerShell is not used in a run (its writes, such as Set-Content, Out-File, Remove-Item and Copy-Item, cannot be "
          "checked here); use Bash.")
if tool not in ("Bash", "Monitor", "PowerShell"):
    sys.exit(0)
UNCHECKED = not (bound or mem)  # an ordinary session: only the web QA secret rule below applies
if role == "web-qa" and tool in ("Monitor", "PowerShell"):
    block("webqa-command", f"{who} runs only the driver, `state.sh webqa <run>` and ls / mkdir -p under webqa/ with Bash, not {tool}.")
if bound and not str(inp.get("command") or "").strip():
    block("no-command", f"{tool} without a command text: it cannot be checked.")

GIT_RO = {"status", "log", "diff", "show", "rev-parse", "rev-list", "ls-files", "ls-tree", "cat-file", "grep",
          "blame", "annotate", "describe", "shortlog", "merge-base", "name-rev", "for-each-ref", "show-ref",
          "whatchanged", "help", "version", "var", "check-ignore", "count-objects", "show-branch", "range-diff",
          "cherry", "diff-tree", "diff-files", "diff-index", "ls-remote", "verify-commit"}
GH_WRITES = {"create", "comment", "edit", "close", "reopen", "delete", "merge", "lock", "transfer", "pin", "fork",
             "upload", "set", "add", "remove", "rename", "archive", "sync", "run", "enable", "disable"}
WRAPPERS = {"sudo", "env", "command", "nohup", "time", "exec", "builtin", "nice", "then", "do", "else", "!"}
SAFE_DEV = ("/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty")


def split_heredocs(cmd):
    """(the command text without heredoc bodies, [(the line that opened the heredoc, its body)])."""
    out, docs, skip, header, body = [], [], None, "", []
    for line in cmd.split("\n"):
        if skip is not None:
            if line.strip() == skip:
                docs.append((header, "\n".join(body)))
                skip, body = None, []
            else:
                body.append(line)
            continue
        out.append(line)
        m = re.search(r"(?<!<)<<(?!<)-?\s*['\"]?(\w+)['\"]?", line)
        if m:
            skip, header = m.group(1), line
    if skip is not None:  # an unterminated heredoc still counts
        docs.append((header, "\n".join(body)))
    return "\n".join(out), docs


def strip_heredocs(cmd):
    return split_heredocs(cmd)[0]


def lex(text):
    lx = shlex.shlex(text, posix=True, punctuation_chars=";&|()<>\n`")
    lx.whitespace = " \t\r"
    lx.whitespace_split = True
    lx.commenters = ""
    return list(lx)


def is_punct(t):
    return bool(t) and all(c in ";&|()<>\n`" for c in t)


def ship_gate(what):
    stage, appr = status.get("stage"), status.get("shipApproval")
    if stage != "shipping" or not appr:
        block("ship-gate", f"{what} is allowed only at shipping, after the user's ship approval (stage {stage}, shipApproval "
              f"{'set' if appr else 'missing'}). Ship with `state.sh ship-pr <run> <pr id>`.")


def check_write(t, cwd):
    if t in SAFE_DEV or t.startswith("/dev/fd/"):
        return
    if bound and re.search(r"[$`*?]", t) and re.search(r"\.gitconfig|\.config/git|/git/config\b", t):
        block("git-user-config", GITCONFIG_MSG)
    if re.search(r"[$`*?]", t) or (cwd is None and not os.path.isabs(os.path.expanduser(t))):
        if trust_root and "run-trust" in t.lower():
            block("run-trust", TRUST_MSG)
        if mem and re.search(r"\$\{?DCV2_MEMORY_DIR\b", t):
            block("memory-write", "codebase memory is written only with memory.sh write, never with a shell redirect.")
        if mem and cwd is None and MEM_MENTIONED:
            block("unknown-write-target", "cannot tell where this relative write lands (the command changes into an unknown folder and mentions "
                  "the memory folder); write memory only with memory.sh write.")
        if pipeline:
            block("unknown-write-target", f"cannot check the write target {t!r}: write to a literal absolute path")
        return
    p = real(t, cwd)
    if inside(p, trust_root):
        block("run-trust", TRUST_MSG)
    if inside(p, mem):
        block("memory-write", "codebase memory is written only with memory.sh write, never with a shell redirect, copy or move.")
    if not bound:
        return
    if git_user_config(p):
        block("git-user-config", GITCONFIG_MSG)
    if role == "web-qa" and inside(p, os.path.join(run, "webqa")) and not protected(p):
        return  # mkdir -p under webqa/ (webqa_command_check lets nothing else write there from the shell)
    zone = no_write_zone(p)
    if zone:
        block("no-write-zone", f"{p} is {zone}: it is never written from a bound run.")
    if protected(p):
        block("run-file-write", "write run-folder state only with scripts/state.sh, never with a shell redirect or copy.")
    if bmad_config(p):
        block("bmad-config", BMAD_CFG_MSG)
    if not pipeline and main_checkout(p):
        block("main-checkout-write", f"the orchestrator never writes into a repo's main checkout ({p}).")
    if role in ("qa", "web-qa") and not write_allowed(p):
        block("outside-allowed-write", f"{who} may write only {where}, not {p}, from Bash.")
    if pipeline and not write_allowed(p) and in_checkout(p):
        block("outside-allowed-write", f"{who} may not write {p} from Bash: it is in a git checkout outside {where}.")


def link_operands(args):
    """(operands, target dir) of cp / ln / install style args: the value of -t and -S is not an operand."""
    ops, tdir, i = [], None, 0
    while i < len(args):
        a = args[i]
        if a == "--":
            ops += args[i + 1:]
            break
        if a in ("-t", "--target-directory") and i + 1 < len(args):
            tdir, i = args[i + 1], i + 2
            continue
        if a.startswith("--target-directory="):
            tdir, i = a.split("=", 1)[1], i + 1
            continue
        if a in ("-S", "--suffix") and i + 1 < len(args):
            i += 2
            continue
        if a.startswith("-") and a != "-":
            i += 1
            continue
        ops.append(a)
        i += 1
    return ops, tdir


def link_targets(name, args, cwd):
    """`ln` and `cp -l|-s|--link|--symbolic-link` make a name that later writes follow to the source, so every operand
    is a write target (ln -s <repo>/a <run>/l && echo x > <run>/l writes the repo's a). A relative soft-link source
    is resolved against the link's folder (against the cwd with -r), as the link will be."""
    clusters = [a[1:] for a in args if re.match(r"^-[A-Za-z]+$", a)]
    longs = [a for a in args if a.startswith("--")]
    letters = "".join(clusters)
    if name == "cp" and not ("l" in letters or "s" in letters or "--link" in longs or "--symbolic-link" in longs):
        return None
    soft = "s" in letters or "--symbolic" in longs or "--symbolic-link" in longs
    relative = "r" in letters or "--relative" in longs
    ops, tdir = link_operands(args)
    links = [tdir] if tdir else ops[-1:]
    sources = ops if tdir else ops[:-1]
    if tdir is None and len(ops) == 1:  # `ln -s <target>` makes ./<basename> in the cwd
        links, sources = [os.path.join(cwd, os.path.basename(ops[0].rstrip("/")))] if cwd else [], ops
    out = list(links)
    folder = None
    if soft and not relative and links and "$" not in links[0] and (os.path.isabs(os.path.expanduser(links[0])) or cwd):
        p = real(links[0], cwd or "/")
        folder = p if (tdir or os.path.isdir(p)) else os.path.dirname(p)
    for src in sources:
        s0 = os.path.expanduser(src)
        out.append(os.path.join(folder, s0) if folder and soft and not relative and not os.path.isabs(s0) and "$" not in src else src)
    return out


def write_targets(name, args, cwd=None):
    nonopt = [a for a in args if a and not a.startswith("-")]
    if name in ("ln", "cp"):
        linked = link_targets(name, args, cwd)
        if linked is not None:
            return linked
    if name in ("tee", "rm", "rmdir", "touch", "mkdir", "truncate", "unlink", "shred", "mv"):
        return nonopt
    if name in ("chmod", "chown", "chgrp"):
        return nonopt[1:]
    if name in ("cp", "install", "ln", "rsync", "scp"):
        for i, a in enumerate(args):
            if a in ("-t", "--target-directory") and i + 1 < len(args):
                return [args[i + 1]]
            if a.startswith("--target-directory="):
                return [a.split("=", 1)[1]]
        return nonopt[-1:]
    if name == "dd":
        return [a[3:] for a in args if a.startswith("of=")]
    if name in ("curl", "wget"):
        return [args[i + 1] for i, a in enumerate(args[:-1]) if a in ("-o", "--output", "-O", "--output-document")]
    inplace = (name == "sed" and any(a == "-i" or a.startswith("-i") or a.startswith("--in-place") for a in args)) or \
              (name == "perl" and any(re.match(r"^-[a-zA-Z]*i", a) for a in args))
    if inplace:
        files, skip, script = [], False, any(a in ("-e", "-f", "--expression", "--file") or a.startswith("--expression=") for a in args)
        for a in args:
            if skip:
                skip = False
                continue
            if a in ("-e", "-f", "--expression", "--file"):
                skip = True
                continue
            if not a or a.startswith("-"):
                continue
            if not script:
                script = True
                continue
            files.append(a)
        return files
    return []


def git_readonly(sub, rest):
    if sub in GIT_RO:
        return True
    if sub == "branch":
        if any(a in ("-d", "-D", "-m", "-M", "-c", "-C", "-f", "-u", "--delete", "--move", "--copy", "--force",
                     "--unset-upstream", "--edit-description") or a.startswith("--set-upstream") for a in rest):
            return False
        listing = any(a in ("--list", "-l", "--contains", "--merged", "--no-merged", "--points-at") for a in rest)
        return listing or not [a for a in rest if not a.startswith("-")]
    if sub == "remote":
        return not rest or rest[0] in ("-v", "--verbose", "show", "get-url")
    if sub == "config":
        if any(a in ("--unset", "--unset-all", "--add", "--replace-all", "--rename-section", "--remove-section", "-e", "--edit")
               for a in rest):
            return False
        if any(a in ("--get", "--get-all", "--get-regexp", "--get-urlmatch", "--list", "-l") for a in rest):
            return True
        if rest[:1] in (["get"], ["list"]):
            return True
        rest = [a for a in rest if a not in ("--show-origin", "--show-scope")]  # `git config --show-origin <key>` reads
        # `git config <key>` reads; a key and anything else (a value, even one starting with `-`) writes.
        return len(rest) == 1 and not rest[0].startswith("-") and rest[0] not in ("set", "unset", "edit")
    if sub == "worktree":
        return rest[:1] == ["list"]
    if sub == "stash":
        return rest[:1] in (["list"], ["show"])
    if sub == "tag":
        return not rest or any(a in ("-l", "--list") for a in rest)
    if sub == "symbolic-ref":
        return len([a for a in rest if not a.startswith("-")]) <= 1
    if sub == "reflog":
        return not rest or rest[0] == "show"
    return False


GIT_C_KEYS = ("user.name", "user.email", "core.quotepath", "init.defaultbranch", "commit.gpgsign")
GIT_C_PREFIXES = ("color.", "advice.")


def git_c_ok(kv):
    key, _, value = kv.partition("=")
    key = key.lower()
    if key == "commit.gpgsign":
        return value.lower() in ("false", "0", "no", "off")
    return key in GIT_C_KEYS or key.startswith(GIT_C_PREFIXES)


GIT_ENV_RE = re.compile(r"^(GIT_DIR|GIT_WORK_TREE|GIT_COMMON_DIR|GIT_INDEX_FILE|GIT_OBJECT_DIRECTORY|GIT_ALTERNATE_OBJECT_DIRECTORIES|"
                        r"GIT_NAMESPACE|GIT_TEMPLATE_DIR|GIT_CONFIG_COUNT|GIT_CONFIG_KEY_\w*|GIT_CONFIG_VALUE_\w*|GIT_CONFIG_PARAMETERS|GIT_CONFIG|GIT_CONFIG_GLOBAL|"
                        r"GIT_CONFIG_SYSTEM|GIT_EXEC_PATH|GIT_SSH_COMMAND|GIT_SSH|GIT_ASKPASS|GIT_PROXY_COMMAND|GIT_EXTERNAL_DIFF)=")


def own_branch(target):
    """The run's branch of the recorded worktree target is in (dcv2/<runId>-<pr>), or None."""
    for w in worktrees:
        if target and inside(target, w):
            name = os.path.basename(w)
            return "dcv2/" + name[len("dcv2-"):] if name.startswith("dcv2-") else None
    return None


def shared_refs(sub, rest, target):
    """Worktrees, branches and refs belong to the whole repo (every worktree shares them): nobody in a bound run changes
    them with raw git. Only `state.sh worktree` / `state.sh cleanup` and the app do; the Implementer may create or
    switch to its own PR's branch only."""
    if not run:
        return
    how = ("worktrees are made by `state.sh worktree`; removing worktrees and branches is `state.sh cleanup <run>` "
           "(at done / failed), or the app does it.")
    if sub == "worktree" and rest[:1] != ["list"]:
        block("git-worktree", f"{who} never runs `git worktree {' '.join(rest[:1])}`: {how}")
    if sub in ("update-ref", "prune", "gc", "replace") or (sub == "symbolic-ref" and not git_readonly(sub, rest)) \
            or (sub == "reflog" and not git_readonly(sub, rest)) or (sub == "tag" and not git_readonly(sub, rest)):
        block("git-ref-change", f"{who} never runs `git {sub}` (it changes refs every worktree of the repo shares): {how}")
    if sub == "notes" and rest[:1] not in ([], ["list"], ["show"]):
        block("git-notes", f"{who} never writes git notes (they are shared by every worktree).")
    mine = own_branch(target) if role == "implementer" else None
    if sub in ("fetch", "pull"):
        refs = [a for a in rest if not a.startswith("-")][1:]
        for ref in refs:
            dst = ref.split(":", 1)[1] if ":" in ref else ""
            dst = dst[len("refs/heads/"):] if dst.startswith("refs/heads/") else dst
            if ":" in ref and dst and not dst.startswith("refs/remotes/") and dst != mine:
                block("git-fetch-branch", f"{who} never fetches into a local branch (`git {sub} … {ref}`): branches are shared by every worktree.")
    if sub == "branch" and not git_readonly(sub, rest):
        names = [a for a in rest if not a.startswith("-")]
        if not (mine and not [a for a in rest if a.startswith("-")] and names == [mine]):
            block("git-branch", f"{who} never changes branches with `git branch` (they are shared by every worktree): {how}")
    if sub in ("checkout", "switch"):
        for i, a in enumerate(rest):
            flag = a.split("=", 1)[0]
            if flag in ("-b", "-B", "-c", "-C", "--orphan", "--create", "--force-create") or re.match(r"^-[a-zA-Z]*[bBcC]$", a):
                name = a.split("=", 1)[1] if "=" in a else (rest[i + 1] if i + 1 < len(rest) else "")
                if not mine or name != mine:
                    block("git-branch", f"{who} never creates or resets a branch (`git {sub} {a} {name}`): branches are shared by every worktree.")


def check_git(args, cwd):
    if not bound:
        return
    target, i = cwd, 0
    while i < len(args):
        a = args[i]
        if a == "-C" and i + 1 < len(args):
            v = os.path.expanduser(args[i + 1])
            target = None if "$" in v else (portable.real_path(v) if os.path.isabs(v) else (real(v, target) if target else None))
            i += 2
            continue
        if a == "--config-env" or a.startswith("--config-env="):
            block("git-config-flag", "`git --config-env` is never used in a dcv2 run.")
        if a.startswith("--exec-path="):
            block("git-config-flag", "`git --exec-path=<dir>` is never used in a dcv2 run.")
        if a == "-c" or (a.startswith("-c") and len(a) > 2):
            kv = args[i + 1] if a == "-c" and i + 1 < len(args) else a[2:]
            if not git_c_ok(kv):
                block("git-config-flag", f"`git -c {kv.split('=', 1)[0]}=…` is not allowed in a dcv2 run (only identity, colour, advice and "
                      "similar harmless keys): it could run a program.")
            if a != "-c":
                i += 1
                continue
        if a in ("-c", "--git-dir", "--work-tree", "--namespace", "--config-env") and i + 1 < len(args):
            if a in ("--git-dir", "--work-tree"):
                target = real(args[i + 1], target or "/") if "$" not in args[i + 1] else None
            i += 2
            continue
        if a.startswith("--git-dir=") or a.startswith("--work-tree="):
            v = a.split("=", 1)[1]
            target = real(v, target or "/") if "$" not in v else None
            i += 1
            continue
        if a.startswith("-"):
            i += 1
            continue
        break
    sub = args[i] if i < len(args) else ""
    rest = args[i + 1:]
    if sub == "push":
        if pipeline:
            block("push-forbidden", f"{who} never pushes; the orchestrator ships with state.sh ship-pr after the user's approval.")
        if any(a in ("-f", "--force", "--force-with-lease", "--force-if-includes", "--mirror", "--delete", "-d", "--prune")
               or a.startswith("--force-with-lease=") or a.startswith("+") or (":" in a and a.startswith(":"))
               or re.match(r"^-[a-zA-Z]*[fd]", a) for a in rest):
            block("push-force", "force pushes and remote branch deletes are never allowed.")
        if run and (target is None or main_checkout(target)):
            block("push-target", f"push only from the PR's worktree, never from a repo's main checkout or an unknown folder ({target or 'unknown'}).")
        if any(a in ("--all", "--branches", "--tags") for a in rest):
            block("push-multi", "push one dcv2/ branch at a time (no --all / --tags).")
        refs = [a for a in rest if not a.startswith("-")][1:]
        for ref in refs:
            dst = ref.split(":", 1)[-1]
            dst = dst[len("refs/heads/"):] if dst.startswith("refs/heads/") else dst
            if dst != "HEAD" and not dst.startswith("dcv2/") or (dst == "HEAD" and ":" in ref):
                block("push-ref", f"the run pushes only its own dcv2/ branches (not {ref!r}); ship with state.sh ship-pr.")
        if mode == "github":
            block("push-github-mode", "this run reviews on GitHub: pushes go only through state.sh (draft-pr / push / ready), which check the stage and approvals.")
        ship_gate("git push")
        return
    shared_refs(sub, rest, target)
    check_orchestrator_destructive(sub, rest, target)
    if not pipeline:
        if run and sub in ("remote", "config") and not git_readonly(sub, rest):
            block("git-remote-config", f"the orchestrator never changes git remotes or config (`git {sub} ...`): they are shared by the repo and "
                  "its worktrees, and every push goes to the remote recorded for the run.")
        if run and target and main_checkout(target) and not git_readonly(sub, rest):
            block("main-checkout-git", f"the orchestrator never runs `git {sub}` in a repo's main checkout ({target}); worktrees go through state.sh worktree.")
        return
    if role == "implementer" and target and any(inside(target, w) for w in worktrees):
        if sub in ("remote", "config") and not git_readonly(sub, rest):
            block("git-remote-config", f"{who} never changes git remotes or config (`git {sub} ...`); pushes go to the remote recorded for the run.")
        if sub in ("add", "stage", "commit") and copied:
            force = any(a == "--force" or (re.match(r"^-[a-zA-Z]+$", a) and "f" in a) for a in rest)
            for a in rest:
                if a.startswith("-") or "$" in a:
                    continue
                p = norm(os.path.join(target, os.path.expanduser(a)))
                if p in copied or (force and any(c.startswith(p.rstrip(os.sep) + os.sep) for c in copied)):
                    block("copied-file-commit", f"`git {sub}` would include {a!r}, a file copied in from the main checkout (repos.json copyFiles): "
                          "it is never staged or committed. Stage only the files your story changed.")
        return
    if git_readonly(sub, rest):
        return
    block("git-outside-worktree", f"{who} may not run `git {sub}` in {target or 'an unknown folder'}: "
          + ("outside its worktree only read-only git commands are allowed." if role == "implementer"
             else "it may only run read-only git commands."))


GH_VALUE_FLAGS = ("-R", "--repo", "--hostname", "-X", "--method", "-H", "--header", "-q", "--jq", "-t", "--template")


def check_gh(args):
    if not bound:
        return
    words, i = [], 0
    while i < len(args):
        a = args[i]
        if a in GH_VALUE_FLAGS:
            i += 2
            continue
        if not a.startswith("-"):
            words.append(a)
        i += 1
    sub, verb = (words + ["", ""])[:2]
    if pipeline:
        if sub in ("pr", "api"):
            block("gh-subagent", f"{who} never calls `gh {sub}`; the orchestrator opens PRs at shipping.")
        if verb in GH_WRITES:
            block("gh-write", f"{who} never changes anything on GitHub (`gh {sub} {verb}`).")
        return
    if sub == "api":
        # Only reads: no method flag, or GET. Any short-flag cluster with X sets the method (-X PUT, -XPUT,
        # -iX PUT, -iXPUT); a cluster with f/F, or --field/--raw-field/--input, sends a body.
        method, fields, j = None, False, 0
        while j < len(args):
            a = args[j]
            nxt = args[j + 1] if j + 1 < len(args) else ""
            if a == "--method":
                method, j = nxt, j + 2
                continue
            if a.startswith("--method="):
                method = a.split("=", 1)[1]
            elif a in ("--field", "--raw-field", "--input") or re.match(r"^--(field|raw-field|input)=", a):
                fields = True
            elif re.match(r"^-[A-Za-z]", a) and not a.startswith("--"):
                cluster = a[1:]
                head, x, tail = cluster.partition("X")
                if "f" in head or "F" in head:
                    fields = True
                if x:
                    method = tail if tail else nxt
                    j += 1 if tail else 2
                    continue
            j += 1
        if (method is not None and method.upper() != "GET") or fields:
            block("gh-api-write", "the run never writes to GitHub with `gh api`; every GitHub write goes through a state.sh command.")
        return
    if sub == "pr" and verb in ("merge", "close", "edit", "ready", "reopen", "review", "comment", "lock"):
        block("gh-pr-write", f"the run never runs `gh pr {verb}` itself: it never merges, closes or edits PRs"
              + ("; drafts, comments and `ready` go through state.sh." if mode == "github" else "."))
    if sub == "pr" and verb == "create":
        if mode == "github":
            block("gh-pr-create", "this run reviews on GitHub: PRs are opened only by `state.sh draft-pr` / `state.sh ready`.")
        ship_gate("gh pr create")
        return
    if verb in GH_WRITES:
        block("gh-write", f"the run never changes anything on GitHub with `gh {sub} {verb}`; GitHub writes go through state.sh.")


GH_STATE = ("draft-pr", "push", "post-review", "comment")


def check_state_sh(args):
    """GitHub writes through state.sh: never by a sub-agent; the orchestrator's only in github mode, after the plan /
    ship approval (state.sh checks too)."""
    sub = next((a for a in args[1:] if not a.startswith("-")), "")
    if pipeline:
        if sub in GH_STATE + ("ready", "ship-pr", "cleanup", "request", "qa-serve", "qa"):
            block("state-github-subagent", f"{who} never runs `state.sh {sub}`: only the orchestrator touches GitHub.")
        return
    if sub in GH_STATE:
        if mode != "github":
            block("state-github-local", f"`state.sh {sub}` is for runs that review on GitHub; this run's review mode is local.")
        plan = status.get("plan") if isinstance(status.get("plan"), dict) else {}
        if not plan.get("approval"):
            block("state-plan-approval", f"`state.sh {sub}` needs the plan approval first (stop #1).")
    if sub == "ready" and not status.get("shipApproval"):
        block("state-ship-approval", "`state.sh ready` needs the ship approval first (stop #2).")
    if sub in ("cleanup", "request", "request-wait") and run:
        rest = [a for a in args[1:] if not a.startswith("-")]
        target = re.sub(r"\$\{?DCV2_RUN_DIR\}?", lambda _: run, rest[1] if len(rest) > 1 else "")
        if "$" not in target and not same(os.path.expanduser(target), run):
            block("state-other-run", f"`state.sh {sub}` acts only on this run's folder ({run}), not {target or 'none'}.")


def check_node(args, cwd):
    """BMAD's runtime: only the planner runs the wrapper (bmad/runtime.mjs); nobody runs the bundle behind it.
    The path is normalised first (relative to the cwd when it is known, `..` resolved), so `cd <plugin>/bmad &&
    node ./runtime.mjs` and `bmad/x/../runtime.mjs` are the runtime too."""
    for a in args:
        if a.startswith("-"):
            continue
        if "$" in a or (cwd is None and not os.path.isabs(os.path.expanduser(a))):
            p = os.path.normpath(a)
        else:
            p = real(a, cwd or "/")
        d, b = os.path.split(p)
        if b == "runtime.mjs" and os.path.basename(d) in ("bmad", ""):
            if role != "planner":
                block("bmad-runtime", f"{who} never runs the BMAD runtime: only the planner does, for its bmad pass "
                      "(the orchestrator checks the documents with `state.sh bmad-check`).")


LIVE_SCRIPTS = ("golden.sh", "bmad_spike.sh")


def live_tests_on(wt):
    """repos.json (app-written) gives the repo of worktree wt `liveTests: true`."""
    parent = os.path.dirname(wt)
    name = os.path.basename(parent)
    if not name.endswith("-worktrees"):
        return False
    repo = os.path.join(os.path.dirname(parent), name[: -len("-worktrees")])
    try:
        rj = json.load(open(os.path.join(run, "repos.json")))
    except Exception:
        return False
    for r in (rj.get("repos") if isinstance(rj, dict) else None) or []:
        if isinstance(r, dict) and isinstance(r.get("path"), str) and same(r["path"], repo):
            return r.get("liveTests") is True
    return False


def check_live(script, cwd):
    """golden.sh / bmad_spike.sh launch claude: only the implementer / pr-reviewer, in its worktree, with liveTests."""
    if role not in ("implementer", "pr-reviewer"):
        block("live-test", f"{who} never runs the live Claude tests ({os.path.basename(script)}).")
    if "$" in script or (cwd is None and not os.path.isabs(os.path.expanduser(script))):
        block("live-test", f"cannot tell where {script!r} is: run it as `cd <worktree> && bash scripts/skill-e2e/{os.path.basename(script)} ...`")
    p = real(script, cwd or "/")
    wt = next((w for w in worktrees if inside(p, w)), None)
    if not wt or os.path.relpath(norm(p), norm(wt)) != os.path.join("scripts", "skill-e2e", os.path.basename(norm(p))):
        block("live-test", f"live Claude tests run only as your worktree's scripts/skill-e2e/{os.path.basename(script)}.")
    if not live_tests_on(wt):
        block("live-test", "live Claude tests are off for this repo (the app setting \"Allow live Claude tests in checks\"); "
              "they stay a manual step for the user.")


SHELLS = ("bash", "sh", "zsh", "dash", "ksh")
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
# Programs that run the next word as a command, and their options that take a value.
RUNNERS = {"nohup": (), "caffeinate": (), "stdbuf": (), "command": (), "exec": ("-a",), "builtin": (), "sudo": ("-u", "-g", "-C"),
           "doas": ("-u",), "time": (), "env": ("-u", "--unset", "-C", "--chdir", "-S", "--split-string"),
           "nice": ("-n", "--adjustment"), "timeout": ("-s", "--signal", "-k", "--kill-after"), "npx": ("-p", "--package"),
           "bunx": (), "unbuffer": (), "chronic": ()}


ASSIGN_ANY = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*\+?=")
PROTECTED_ENV_RE = re.compile(r"^(DCV2_\w*|CLAUDE_PLUGIN_ROOT|CLAUDE_CONFIG_DIR)\+?=(.*)$", re.S)
DECLARERS = ("export", "declare", "typeset", "readonly", "local")


def prefix_assignments(words, depth=0):
    """The NAME=value words that set the environment for what a segment runs: bare prefixes, after env / sudo / nice …,
    and the arguments of export / declare / typeset / readonly / local."""
    out, i = [], 0
    while i < len(words):
        w, b = words[i], os.path.basename(words[i])
        if ASSIGN_ANY.match(w):
            out.append(w)
            i += 1
        elif b in DECLARERS:
            out += [x for x in words[i + 1:] if ASSIGN_ANY.match(x) or re.match(r"^[^=\s]*\$[^=\s]*=", x)]
            break
        elif b in RUNNERS or w in WRAPPERS:
            i += 1
            while i < len(words) and words[i].startswith("-"):
                opt, i = words[i], i + 1
                if b == "env" and (opt in ("-S", "--split-string") or opt.startswith("--split-string=")) and depth < 3:
                    text = opt.split("=", 1)[1] if "=" in opt else (words[i] if i < len(words) else "")
                    i += 0 if "=" in opt else 1
                    try:
                        out += prefix_assignments(shlex.split(text), depth + 1)
                    except ValueError:
                        pass
                elif opt in RUNNERS.get(b, ()) and i < len(words):
                    i += 1
            if b == "timeout" and i < len(words):
                i += 1
        else:
            break
    return out


def check_env(w):
    """DCV2_*, CLAUDE_PLUGIN_ROOT and CLAUDE_CONFIG_DIR decide what the guard, state.sh, memory.sh and the BMAD runtime
    trust, so nothing in a dcv2 session (a bound run, or any tab with DCV2_MEMORY_DIR) sets them. Allowed: DCV2_E2E_*
    (live-suite knobs) and DCV2_RUN_DIR / DCV2_MEMORY_DIR given the session's own value (the planner prefixes runtime
    calls with its own run folder). `env -u NAME` and `unset` are not assignments."""
    m = PROTECTED_ENV_RE.match(w)
    if not m:
        if re.match(r"^[^=\s]*\$[^=\s]*=", w):
            block("env-unknown", f"cannot tell which variable `{w.split('=', 1)[0]}=…` sets in a dcv2 session; name it literally.")
        return
    name, val = m.group(1), m.group(2)
    if name.startswith("DCV2_E2E_"):
        return
    own = {"DCV2_RUN_DIR": run, "DCV2_MEMORY_DIR": mem}.get(name)
    if own and not w.startswith(name + "+="):
        if val in ("$" + name, "${" + name + "}"):
            return
        v = os.path.expanduser(val)
        if os.path.isabs(v) and "$" not in v and same(v, own):
            return
    block("env-setting", f"setting {name} is not allowed in a dcv2 session: it redirects what the guard, state.sh, memory.sh and the plugin trust "
          f"(use `env -u {name} …` to unset it for one command; a plain terminal is unaffected).")


def unwrap(words):
    """The command a segment really runs: assignments, `then`/`do`/…, and runners (timeout, nice, env, npx, …) with
    their options dropped."""
    while words:
        w, b = words[0], os.path.basename(words[0])
        if ASSIGN.match(w) or (w in WRAPPERS and b not in RUNNERS):
            words = words[1:]
            continue
        if b in ("npm", "pnpm", "yarn") and len(words) > 1 and words[1] in ("exec", "dlx", "x"):
            b, words = "npx", words[1:]
        if b not in RUNNERS:
            break
        words = words[1:]
        while words and (words[0].startswith("-") or (b == "env" and ASSIGN.match(words[0]))):
            opt, words = words[0], words[1:]
            if opt == "--":
                break
            if b == "env" and (opt in ("-S", "--split-string") or opt.startswith("--split-string=")) and (words or "=" in opt):
                text = opt.split("=", 1)[1] if "=" in opt else words.pop(0)
                try:
                    words = shlex.split(text) + words
                except ValueError:
                    block("env-parse", "could not parse `env -S` text")
                continue
            if opt in RUNNERS[b] and words:
                words = words[1:]
        if b == "timeout" and words:
            words = words[1:]  # the duration
    return words


def shell_c(args):
    """The command string of `bash [-lxe…]c [-o opt] [--] '<command>'` (any flag cluster with c), else None."""
    i, has_c = 0, False
    while i < len(args):
        a = args[i]
        if a == "--":
            i += 1
            break
        if a in ("-o", "+o", "-O", "+O", "--rcfile", "--init-file"):
            i += 2
            continue
        if re.match(r"^[-+][a-zA-Z]+$", a):
            has_c = has_c or (a[0] == "-" and "c" in a)
            i += 1
            continue
        if a.startswith("--"):
            i += 1
            continue
        break
    return args[i] if has_c and i < len(args) else None


def shell_script(args):
    """The script file `bash [options] [--] <script>` runs (None with -c, or none given)."""
    i = 0
    while i < len(args):
        a = args[i]
        if a == "-c" or (re.match(r"^-[a-zA-Z]+$", a) and "c" in a):
            return None
        if a == "--":
            i += 1
            break
        if a in ("-o", "+o", "-O", "+O", "--rcfile", "--init-file"):
            i += 2
            continue
        if a.startswith("-") or a.startswith("+"):
            i += 1
            continue
        break
    return args[i] if i < len(args) else None


def check_security(args):
    """No reading the app's keychain items (ClickUp token, trust key) from inside a run."""
    sub = next((a for a in args if not a.startswith("-")), "")
    if sub in ("dump-keychain", "find-generic-password", "find-internet-password", "export", "find-key", "find-certificate"):
        block("keychain", "the app's keychain items are never read from inside a run.")


# ---- interpreters, stdin scripts, files a command wrote itself (CAP-12) -------------------------------------------
# An interpreter's inline code (python3 -c, node -e, perl -e, ruby -e, a heredoc or here-string fed to it) is a program
# the guard cannot read like a shell command, so a bound run's rules look at what it NAMES. This is best effort (a
# string-built path evades it); the PROTECTED checks of Edit/Write and shell writes remain.
# Windows spellings count too: `py` (the launcher) and an optional `.exe` after every name.
INTERP_FAMILY = ((re.compile(r"^(python[\d.]*|py)(\.exe)?$"), "python"), (re.compile(r"^node(js)?(\.exe)?$"), "node"),
                 (re.compile(r"^perl[\d.]*(\.exe)?$"), "perl"), (re.compile(r"^ruby[\d.]*(\.exe)?$"), "ruby"))
STDIN_INTERPRETERS = ("python", "node", "perl", "ruby", "deno", "bun", "osascript", "php", "lua", "Rscript")
WRITE_PRIM_RE = re.compile(
    r"\bopen\s*\([^,)]*,\s*(?:mode\s*=\s*)?['\"][^'\"]*[wax+]|\bopen\s*\([^)]*\bmode\s*=\s*['\"][^'\"]*[wax+]|\.write_text\b|\.write_bytes\b|\bwriteFile(Sync)?\b|\bappendFile(Sync)?\b|"
    r"\bfs\.(write|rm|unlink|rename|copy|mkdir|rmdir)|\bcreateWriteStream\b|\bos\.(system|popen|remove|unlink|rename|replace|rmdir|"
    r"removedirs|exec\w*|spawn\w*)\b|\bsubprocess\b|\bPopen\b|\bshutil\.|\bchild_process\b|\bsystem\s*\(|\bexec\s*\(|"
    r"\bFile\.(write|open|delete|rename)|\bFileUtils\b|\bIO\.(write|popen)", re.I)
WRITTEN = set()  # real paths this command has already written (redirects, tee, cp, mv, ...)


def interp_family(name):
    for rx, fam in INTERP_FAMILY:
        if rx.match(name):
            return fam
    return None


def interp_parse(fam, args):
    """(inline code strings, script operand) of an interpreter's args: `-c`/`-e`/`--eval`/`-p` code, and the first
    non-option operand (the script, `-` for stdin, or the module of `python -m`)."""
    code, script, i = [], None, 0
    while i < len(args):
        a = args[i]
        nxt = args[i + 1] if i + 1 < len(args) else None
        if a == "--":
            script = nxt
            break
        if a == "-":
            script = "-"
            break
        if a.startswith("--"):
            flag, eq, val = a.partition("=")
            if fam == "node" and flag in ("--eval", "--print"):
                if eq:
                    code.append(val)
                elif nxt is not None:
                    code.append(nxt)
                    i += 1
            i += 1
            continue
        if a.startswith("-"):
            cl = a[1:]
            if fam == "python" and re.fullmatch(r"\d[\d.]*(-\d+)?", cl):  # the `py` launcher's version flag: -3, -3.12, -3-64
                i += 1
                continue
            if fam == "python" and "c" in cl:
                at = cl.index("c")
                if at == len(cl) - 1 and nxt is not None:
                    code.append(nxt)
                    i += 2
                else:
                    code.append(cl[at + 1:])
                    i += 1
                continue
            if fam == "python" and cl.endswith("m") and nxt is not None:
                script = nxt
                break
            if fam == "node" and cl and cl[-1] in "ep" and nxt is not None:
                code.append(nxt)
                i += 2
                continue
            if fam == "node" and a in ("-r", "--require", "--import", "--loader") and nxt is not None:
                i += 2
                continue
            if fam in ("perl", "ruby") and cl and cl[-1] in ("eE" if fam == "perl" else "e") and nxt is not None:
                code.append(nxt)
                i += 2
                continue
            i += 1
            continue
        script = a
        break
    return code, script


def check_code(name, texts, cwd):
    """Inline code (or a heredoc / here-string fed to an interpreter) in a bound run: it may not name a protected run file
    (path-aware, as `protected()`), the run's signed records, or, with a write primitive, the memory or plugin folders; the
    orchestrator may not run inline code with a write primitive at all (its writes go through state.sh and Edit/Write).

    The protected-name match is PATH-AWARE, like `protected()`: a name such as status.json counts only when it resolves to a
    run folder (the run itself, or a folder holding a request.md), not as a casefolded substring anywhere in the code. This
    departs from the architecture's AD-9 wording (a casefolded substring match) on purpose: the repo tracks
    fixtures/run/<run>/status.json and repos.json, which an Implementer legitimately reads from its worktree
    (`python3 -c "...open('fixtures/run/r-x/status.json')..."`), and a substring rule would block that (SM-C1, no false
    positives). Code that names DCV2_RUN_DIR or the run folder's path together with a protected name is still matched as
    a casefolded substring, since the path is then the run folder whatever the code joins it with."""
    if not bound:
        return
    text = "\n".join(texts)
    low = text.casefold()
    writes = bool(WRITE_PRIM_RE.search(text))
    if run and not pipeline and writes:
        block("interpreter-write", f"the orchestrator never runs inline {name} code that writes or runs programs (open(...,'w'), write_text, writeFile, "
              "os.system, subprocess, ...): it cannot be checked; write run files with state.sh and edit with Edit/Write.")
    if "run-trust" in low:
        block("run-trust", TRUST_MSG)
    if run and ("dcv2_run_dir" in low or norm(run) in low) and any(n in low for n in PROTECTED_CF):
        block("interpreter-write", f"inline {name} code that names the run folder and a protected run file (status.json, writeback.json, .guard.json, "
              "meta.json, repos.json, ...) is not allowed: write run-folder state only with scripts/state.sh.")
    for piece in re.split(r"""[\s'"`(),=\[\]{}<>;:|&+*]+""", text):
        if not piece or "$" in piece:
            continue
        looks_like_path = "/" in piece or piece.startswith("~")
        if os.path.basename(piece).casefold() in PROTECTED_CF:
            if os.path.isabs(os.path.expanduser(piece)) or cwd is not None:
                p = real(piece, cwd or "/")
                if protected(p):
                    block("interpreter-write", f"inline {name} code names {piece!r}, a run file only state.sh (or the app) writes: use state.sh.")
            else:
                block("interpreter-write", f"inline {name} code names {piece!r} and the working folder is not known: it may be a run file; use state.sh.")
        if looks_like_path and (os.path.isabs(os.path.expanduser(piece)) or cwd is not None):
            p = real(piece, cwd or "/")
            if inside(p, trust_root):
                block("run-trust", TRUST_MSG)
            if writes and (inside(p, mem) or any(inside(p, r) for r in
                                                  [r for r in (os.environ.get("CLAUDE_PLUGIN_ROOT") or "", GUARD_PLUGIN) if r])):
                block("memory-write", f"inline {name} code with a write names {piece!r}: codebase memory is written only with memory.sh write, "
                      "and the plugin is never written from a run.")


def note_written(t, cwd):
    """Remember a file the command wrote (a redirect target or a write operand) for the `runs what it wrote` rule."""
    if not t or re.search(r"[$`*?]", t) or t in SAFE_DEV or t.startswith("/dev/"):
        return
    if os.path.isabs(os.path.expanduser(t)) or cwd is not None:
        WRITTEN.add(norm(real(t, cwd or "/")))


def check_runs_written(path, cwd, what):
    if bound and path and "$" not in path and (os.path.isabs(os.path.expanduser(path)) or cwd is not None) \
            and norm(real(path, cwd or "/")) in WRITTEN:
        block("write-then-run", f"this command writes {path} and then runs it ({what}): the code is never seen by the guard. Write the file in one "
              "command (Edit/Write) and run it in the next.")


def worktree_hit(p):
    return bool(p) and any(inside(p, w) for w in worktrees)


def check_orchestrator_destructive(sub, rest, target):
    """The orchestrator (no agent type) in a bound run never runs git commands that discard files in a run worktree
    (git clean, reset --hard, checkout -- <path> / checkout ., restore, stash drop|clear): the Implementer's work lives
    there. Implementers keep these commands inside their own worktree."""
    if not run or pipeline or not worktrees:
        return
    kind = None
    if sub == "clean":
        kind = "git clean"
    elif sub == "reset" and any(a == "--hard" or a == "--merge" or a == "--keep" for a in rest):
        kind = "git reset --hard"
    elif sub == "checkout" and ("--" in rest or "." in rest or any(a in ("-f", "--force") for a in rest)):
        kind = "git checkout -- <path>"
    elif sub == "restore":
        kind = "git restore"
    elif sub == "stash" and rest[:1] in (["drop"], ["clear"]):
        kind = f"git stash {rest[0]}"
    if not kind:
        return
    if target is None or worktree_hit(target):
        block("worktree-discard", f"the orchestrator never runs `{kind}` in a run worktree (or where the folder cannot be told): it discards the "
              "Implementer's files. The Implementer cleans up inside its own worktree.")


def check_orchestrator_rm(args, cwd):
    """`rm -r` (any recursive flag) by the orchestrator with the cwd or a target inside a run worktree."""
    if not run or pipeline or not worktrees:
        return
    recursive = any(a in ("--recursive",) or (re.match(r"^-[A-Za-z]+$", a) and ("r" in a or "R" in a)) for a in args)
    if not recursive:
        return
    targets = [a for a in args if a and not a.startswith("-")]
    unknown = cwd is None and any(not os.path.isabs(os.path.expanduser(t)) for t in targets)
    if worktree_hit(cwd) or unknown or any(
            "$" in t or re.search(r"[*?]", t) or worktree_hit(real(t, cwd or "/")) for t in targets):
        block("worktree-discard", "the orchestrator never runs `rm -r` in a run worktree, or on a path that cannot be told apart from one: it deletes "
              "the Implementer's files. Worktrees are removed with `state.sh cleanup` (or by the app).")


PROC_SUBST_RE = re.compile(r"(?:^|[\s;&|(])(?:source|\.|bash|sh|zsh|dash|ksh|python[\d.]*|py|nodejs?|perl[\d.]*|ruby[\d.]*)(?:\.exe)?\s+(?:-[\w-]+\s+)*<\(")


def check_program_source(name, words, args, script, piped, docs, stdin_text, stdin_files, cwd, depth):
    """Where a shell or an interpreter gets the program it runs (bound runs): not from a pipe or /dev/stdin, not from a file the
    command itself wrote; inline code and heredocs are checked by what they name (`check_code`)."""
    fam = interp_family(name)
    shell = name in SHELLS or name in ("source", ".")
    is_interp = fam is not None or name in STDIN_INTERPRETERS
    if not (shell or is_interp):
        if "/" in words[0] and "$" not in words[0]:
            check_runs_written(words[0], cwd, "run as a program")
        return
    here = [body for header, body in docs if re.search(r"(?<![\w./-])" + re.escape(name) + r"(?![\w-])", header)] + list(stdin_text)
    if shell:
        code = shell_c(args) if name in SHELLS else None
        operand = script
        if operand is not None and name in SHELLS:  # `sh -s -- <args>`: the operands are arguments, the program is stdin
            lead = args[:args.index(operand)] if operand in args else []
            if any(re.match(r"^-[a-zA-Z]*s[a-zA-Z]*$", a) for a in lead):
                operand = None
        if code is not None:
            return  # `bash -c '<text>'` is analysed by the caller
        if operand is None and name in SHELLS:
            if piped:
                block("pipe-program", f"`| {name}` runs a program that comes from a pipe: it cannot be checked. Save it with Edit/Write, read it, "
                      "then run the file in a later command.")
            for body in here:  # a heredoc / here-string fed to a shell is a command text: look at it as one
                if depth < 4:
                    analyze(body, cwd, depth + 1)
            for f in stdin_files:
                check_runs_written(f, cwd, f"fed to {name}")
        elif operand in ("/dev/stdin", "-") or (operand or "").startswith("/dev/fd/"):
            block("stdin-program", f"`{name} {operand}` runs a program from stdin: it cannot be checked.")
        elif operand:
            check_runs_written(operand, cwd, f"run with {name}")
        return
    code, operand = interp_parse(fam, args) if fam else ([], next((a for a in args if not a.startswith("-")), None))
    if operand in ("/dev/stdin",) or (operand or "").startswith("/dev/fd/"):
        block("stdin-program", f"`{name} {operand}` runs a program from stdin: it cannot be checked.")
    if not code and operand in (None, "-"):
        if piped:
            block("pipe-program", f"`| {name}` runs a program that comes from a pipe: it cannot be checked. (Reading piped JSON is fine with "
                  f"`| {name} -c '<code>'`.)")
        code = list(here)  # a heredoc / here-string is the program
        for f in stdin_files:
            check_runs_written(f, cwd, f"fed to {name}")
    elif operand and "$" not in operand and fam:
        check_runs_written(operand, cwd, f"run with {name}")
    if fam and code:
        check_code(name, code + here, cwd)


def analyze(cmd, cwd, depth=0):
    if depth > 4:
        if pipeline:
            block("nesting-depth", "command nesting too deep to check")
        return cwd
    text, docs = split_heredocs(cmd)
    if bound and PROC_SUBST_RE.search(text):
        block("process-substitution", "a shell or interpreter reading its program from <(...) is never run in a dcv2 run: the program cannot be checked.")
    try:
        toks = lex(text)
    except ValueError:
        if pipeline:
            block("parse-error", "could not parse this command (unbalanced quotes?); write it more simply")
        if bound and re.search(r"\bgit\b.*\bpush\b|\bgh\b.*\bpr\b", text):
            block("parse-error", "could not parse this push / PR command")
        return cwd
    segs, seg, flags, after_pipe = [], [], [], False
    for t in toks:
        if not is_punct(t):
            seg.append(t)
            continue
        for op in re.findall(r"&>>|&>|>>|>&|>\||<<<|<<|<&|<>|>|<|&&|\|\||\|&|[;|()\n`&]", t):
            if "<" in op or ">" in op:
                seg.append(op)
            else:
                segs.append(seg)
                flags.append(after_pipe)
                seg = []
                after_pipe = op in ("|", "|&")
    segs.append(seg)
    flags.append(after_pipe)
    for seg, piped in zip(segs, flags):
        words, targets, i = [], [], 0
        stdin_text, stdin_files = [], []  # a here-string / a `< file` the segment's program reads as its stdin
        while i < len(seg):
            t = seg[i]
            if is_punct(t):
                nxt = seg[i + 1] if i + 1 < len(seg) and not is_punct(seg[i + 1]) else None
                if "<" in t and ">" not in t:
                    if nxt is not None:
                        (stdin_text if t == "<<<" else stdin_files if t == "<" else []).append(nxt)
                    i += 2 if nxt is not None else 1
                    continue
                if t.endswith("&") and nxt is not None and (nxt.isdigit() or nxt == "-"):
                    i += 2
                    continue
                if nxt is not None:
                    targets.append(nxt)
                i += 2 if nxt is not None else 1
                continue
            if t.isdigit() and i + 1 < len(seg) and is_punct(seg[i + 1]):
                i += 1
                continue
            words.append(t)
            i += 1
        for w in words + targets:
            for a, b in re.findall(r"\$\(([^()]*)\)|`([^`]*)`", w):
                analyze(a or b, cwd, depth + 1)
        for t in targets:
            check_write(t, cwd)
            if bound:
                note_written(t, cwd)
        if bound:
            for w in words:
                if GIT_ENV_RE.match(w):
                    block("git-env", f"setting {w.split('=', 1)[0]} is not allowed in a dcv2 run: it changes which repo git works on "
                          "or which programs it runs.")
        if run or mem:
            for w in prefix_assignments(words):
                check_env(w)
        words = unwrap(words)
        if not words:
            continue
        name, args = re.sub(r"\.exe$", "", os.path.basename(words[0]), flags=re.I), words[1:]  # bash.exe is bash
        script = shell_script(args) if name in SHELLS else (args[0] if name in ("source", ".") and args else None)
        if bound:
            check_program_source(name, words, args, script, piped, docs, stdin_text, stdin_files, cwd, depth)
        if pipeline:
            if "$" in words[0] and "claude" in CMD:
                block("claude-launch", f"cannot tell which program `{words[0]}` is, and the command mentions claude: {who} never launches claude.")
            if any("@anthropic-ai/claude-code" in w for w in words) or name == "claude-code":
                block("claude-launch", f"{who} never launches claude (the @anthropic-ai/claude-code package).")
            if name in ("find",):
                for i, a in enumerate(args):
                    if a in ("-exec", "-execdir", "-ok", "-okdir") and i + 1 < len(args):
                        analyze(" ".join(shlex.quote(x) for x in args[i + 1:] if x not in (";", "+")), cwd, depth + 1)
        if bound and any("ade-runtime.mjs" in w for w in words):
            block("bmad-runtime", "ade-runtime.mjs is the bundle behind the BMAD wrapper and bypasses its write checks; run `node <plugin>/bmad/runtime.mjs <command>` (planner only).")
        if name in ("cd", "pushd"):
            d = next((a for a in args if not a.startswith("-")), "~")
            if mem and re.search(r"\$\{?DCV2_MEMORY_DIR\b", d):
                cwd = mem
                continue
            cwd = None if ("$" in d or cwd is None and not os.path.isabs(os.path.expanduser(d))) else real(d, cwd or "/")
        elif name == "rm" and run and not pipeline:
            check_orchestrator_rm(args, cwd)
            for t in write_targets(name, args, cwd):
                check_write(t, cwd)
        elif name == "git":
            check_git(args, cwd)
        elif name == "gh":
            check_gh(args)
        elif name == "security" and bound:
            check_security(args)
        elif name in ("node", "nodejs") and bound:
            check_node(args, cwd)
        elif name == "claude" and pipeline:
            block("claude-launch", f"{who} never launches claude. Live Claude tests run only as `bash scripts/skill-e2e/golden.sh` or "
                  "`bmad_spike.sh` in your worktree, when the repo allows them (repos.json liveTests).")
        elif name in LIVE_SCRIPTS and pipeline:
            check_live(words[0], cwd)
        elif script and os.path.basename(script) in LIVE_SCRIPTS and pipeline:
            check_live(script, cwd)
        elif name == "state.sh" and bound:  # run directly by path
            check_state_sh([words[0]] + args)
        elif script and os.path.basename(script) == "state.sh" and bound:
            check_state_sh(args[args.index(script):])
        elif name in SHELLS and shell_c(args) is not None:
            analyze(shell_c(args), cwd, depth + 1)
        elif name == "eval":
            analyze(" ".join(args), cwd, depth + 1)
        elif name == "xargs" and args:
            rest = [a for a in args if not a.startswith("-")]
            if rest:
                analyze(" ".join(rest), cwd, depth + 1)
        else:
            for t in write_targets(name, args, cwd):
                check_write(t, cwd)
                if bound:
                    note_written(t, cwd)
    return cwd


# ---- the web-qa agent's shell: the driver, state.sh webqa, ls and mkdir -p under webqa/ ---------------------------------
WEBQA_FLAG_OK = re.compile(r"^-[A-Za-z0-9]+$")


def webqa_under(path):
    """A literal path that resolves inside <run>/webqa."""
    if not path or "$" in path or re.search(r"[*?\[`]", path):
        return False
    return inside(real(os.path.expanduser(path), run), os.path.join(run, "webqa"))


def webqa_simple_command(words):
    """True for exactly one of the three allowed commands (the variables already written out; no env prefix, no wrapper)."""
    if not words:
        return False
    name = words[0]
    plugin_real = [portable.real_path(r) for r in (os.environ.get("CLAUDE_PLUGIN_ROOT") or "", GUARD_PLUGIN) if r]

    def plugin_script(path, script):
        return bool(path) and "$" not in path and any(same(path, os.path.join(r, "scripts", script)) for r in plugin_real)

    if name == "node":
        args = words[2:]
        if len(words) < 2 or not plugin_script(words[1], "qa-browser.mjs"):
            return False
        i = 0
        while i < len(args):
            flag = args[i]
            val = args[i + 1] if i + 1 < len(args) else None
            if val is None:
                return False
            if flag == "--out" and "$" not in val and same(real(val, run), os.path.join(run, "webqa", "shots")):
                pass
            elif flag == "--steps" and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*\.jsonl", os.path.basename(val)) \
                    and "$" not in val and same(os.path.dirname(real(val, run)), os.path.join(run, "webqa", "steps")):
                pass
            elif flag == "--timeout" and re.fullmatch(r"\d{1,7}", val):
                pass
            else:
                return False
            i += 2
        return True
    if name == "bash":
        return len(words) == 4 and plugin_script(words[1], "state.sh") and words[2] == "webqa" and "$" not in words[3] and same(real(words[3], run), run)
    if name == "ls":
        paths = [a for a in words[1:] if not a.startswith("-")]
        flags = [a for a in words[1:] if a.startswith("-")]
        return bool(paths) and all(WEBQA_FLAG_OK.match(f) for f in flags) and all(webqa_under(a) for a in paths)
    if name == "mkdir":
        return len(words) >= 3 and words[1] == "-p" and all(webqa_under(a) for a in words[2:])
    return False


WEBQA_REDIRECT_OK = ("2>&1", ">&2", "1>&2", ">/dev/null", "2>/dev/null", "1>/dev/null")


def webqa_command_check(cmd):
    """The web-qa agent's Bash: every simple command is the driver, `state.sh webqa <run>`, or ls / mkdir -p under <run>/webqa. No env
    prefix, command or process substitution, backtick, heredoc, pipe, background `&` or redirect (but to /dev/null and 2>&1)."""
    fail = lambda why: block("webqa-command", f"{who} may run only `node <plugin>/scripts/qa-browser.mjs --out <run>/webqa/shots --steps <run>/webqa/steps/<name>.jsonl "
                             f"[--timeout <ms>]`, `bash <plugin>/scripts/state.sh webqa <run>`, and ls / mkdir -p under <run>/webqa: {why}.")
    text = expand_vars(cmd)
    if re.search(r"\$|`|<\(|>\(|<<", text):
        fail("no variable, command or process substitution, backtick or heredoc")
    try:
        lx = shlex.shlex(text, posix=True, punctuation_chars=";&|()<>\n")
        lx.whitespace = " \t\r"
        lx.whitespace_split = True
        lx.commenters = ""
        toks = list(lx)
    except ValueError:
        fail("the command does not parse")
    segs, seg, i = [], [], 0
    while i < len(toks):
        t = toks[i]
        if is_punct(t):
            nxt = toks[i + 1] if i + 1 < len(toks) else ""
            if t == "&&" or set(t) <= {";", "\n"}:
                segs.append(seg)
                seg = []
            elif t in (">", ">>", ">|") or t.endswith(">") and set(t) <= {">"}:
                if seg and seg[-1] in ("1", "2"):
                    seg.pop()  # the fd of `2>`
                if nxt != "/dev/null":
                    fail("redirects are not allowed (but to /dev/null and 2>&1)")
                i += 1
            elif t == ">&":
                if seg and seg[-1] in ("1", "2"):
                    seg.pop()
                if nxt not in ("1", "2"):
                    fail("redirects are not allowed (but to /dev/null and 2>&1)")
                i += 1
            elif "<" in t:
                fail("redirects are not allowed")
            elif t == "&":
                fail("no background jobs")
            elif "|" in t:
                fail("no pipes")
            else:
                fail("no subshells, groups or redirects")
        else:
            seg.append(t)
        i += 1
    segs.append(seg)
    segs = [x for x in segs if x]
    if not segs:
        fail("no command")
    for words in segs:
        if ASSIGN_ANY.match(words[0]):
            fail("no environment variable prefix")
        if not webqa_simple_command(words):
            fail(f"{os.path.basename(words[0])} with these arguments is not on the list")


CLICKUP_HELPER_RE = re.compile(r"(?:^|[\s/\"'=(;|&`])clickup\.(?:sh|py)(?![\w.-])", re.I)
CLICKUP_CREATE_RE = re.compile(r"\bcreate\b", re.I)
CLICKUP_ARGS_RE = re.compile(r"\bcreate\b[\s\"',\[\]]*(\d+)[\s\"',]+([^\s\"',\]\)]+)", re.I)
ASKING_MODES = ("default", "acceptEdits", "plan")  # the permission modes that show prompts; any other mode cannot be asked


def printable(text, limit):
    return "".join(c if c.isprintable() else " " for c in text)[:limit]


def clickup_create(cmd):
    """The whole command text (heredoc bodies and quoted interpreter code included): the ClickUp helper's file name and `create`."""
    cmd = expand_vars(cmd)
    return bool(CLICKUP_HELPER_RE.search(cmd) and CLICKUP_CREATE_RE.search(cmd))


def clickup_ask(cmd):
    m = CLICKUP_ARGS_RE.search(expand_vars(cmd))
    reason = f"Create ClickUp tickets from {printable(m.group(2), 200)} in list {m.group(1)}?" if m else "Create ClickUp tickets?"
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask",
                                              "permissionDecisionReason": reason}}))
    sys.exit(0)


CMD = str(inp.get("command") or "")
secret_command_check(CMD, cwd0)
CLICKUP = clickup_create(CMD)
if CLICKUP:
    if bound:
        block("clickup-create", "ClickUp tickets are not created inside an Eagle-Eye run (to read the helper, use the Read tool; "
              "to commit a message that names it, use git commit -F <file>).")
    if str(data.get("permission_mode") or "") not in ASKING_MODES:
        block("clickup-create", "This session's permission mode may skip prompts, so the guard cannot ask you. To create the tickets, "
              "type the command yourself with the ! prefix: ! " + printable(" ".join(CMD.split()), 300))
if role == "web-qa":
    webqa_command_check(CMD)
if UNCHECKED:
    if CLICKUP:
        clickup_ask(CMD)
    sys.exit(0)
if bound and "DCV2_TEST_LOCAL_REMOTES" in CMD:
    block("test-remotes", "DCV2_TEST_LOCAL_REMOTES is set only by the test suites, never from inside a run.")
MEM_MENTIONED = bool(mem) and ("DCV2_MEMORY_DIR" in CMD or mem.casefold() in CMD.casefold())
analyze(CMD, cwd0)
if CLICKUP:
    clickup_ask(CMD)
sys.exit(0)
PY

DCV2_LIB_DIR="$_glib" "${DCV2_PY[@]}" -I -c "$PY" "$GUARD_PLUGIN" <<<"$input"
rc=$?
case $rc in
  0|2) exit $rc ;;
  *) fallback ;;
esac
