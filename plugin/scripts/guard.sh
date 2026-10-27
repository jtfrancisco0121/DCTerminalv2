#!/usr/bin/env bash
# PreToolUse guard for the eagle-eye run (plugin hook: Edit, Write, NotebookEdit, MultiEdit, Bash).
#
# The run it guards is the one named by the env DCV2_RUN_DIR, set when claude is launched
# (the app and the e2e test set it; `state.sh preflight` refuses to start a run without it).
# Hooks inherit it from the claude process, so the orchestrator cannot change it and ticket
# text cannot reach it. The run's allowed worktrees are listed in <run>/.guard.json, which
# only `state.sh worktree` writes.
#
# Pipeline sub-agents (agent_type implementer, planner, plan-reviewer, pr-reviewer, memory-updater,
# with or without the "dcv2:" prefix):
#   - Edit/Write: the real path (symlinks and .. resolved) must be inside the run folder, or,
#     for the implementer only, inside one of the run's recorded worktrees.
#   - Bash: never `git push`, `gh pr`, `gh api` (or gh issue/repo/release writes). Git commands
#     that change anything are allowed only for the implementer inside a recorded worktree;
#     elsewhere only read-only git. Redirects, tee, sed -i, cp/mv/rm/touch/mkdir etc. into a
#     git checkout (a repo's main checkout, another worktree) are blocked unless the target is
#     an allowed place. Best effort: commands that write through other programs (node, python,
#     npm scripts, xargs) are not seen.
#   - No DCV2_RUN_DIR, python3 missing, or a payload that does not parse: blocked (fail closed).
#     Exception: the memory-updater also runs at the end of a standalone /dcv2:developer session
#     (no run): then it may not Edit/Write anything (it writes memory only with memory.sh write).
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
# Everyone in a run: no `security dump-keychain|find-generic-password|find-internet-password|export`.
# Sub-agents never run state.sh's GitHub commands; the Implementer never changes git remotes/config.
# BMAD (bmad/ in the plugin, <run>/bmad in the run): in a bound run only the planner may run
# `node .../bmad/runtime.mjs` (the wrapper itself refuses writes outside <run>/bmad); any command
# naming ade-runtime.mjs (the bundle behind the wrapper) is blocked for everyone; <run>/bmad/_bmad/
# config*.toml and _bmad/custom/ are written only by `state.sh bmad-init`, never with Edit/Write or
# a shell write. Repo and worktree rules are unchanged.
# Cleanup (0.5.0): the orchestrator never runs `git worktree <anything but list>` or a changing `git branch`
# anywhere in a bound run (and never `git push --delete`): `state.sh cleanup` does it, at done / failed only.
# <run>/requests/ is written only by `state.sh request` and the app; app-cleanup.json only by the app.
# Live tests: a pipeline sub-agent never runs `claude`; `scripts/skill-e2e/golden.sh` / `bmad_spike.sh` (which
# launch claude) only the implementer or pr-reviewer runs, inside its recorded worktree, when repos.json gives
# that repo `liveTests: true` (the app setting "Allow live Claude tests in checks"). Best effort, like the rest.
# Everyone: status.json, writeback.json and .guard.json in a run folder are written only by
# state.sh. When DCV2_MEMORY_DIR is set (every dcv2 launch), nobody writes into it with
# Edit/Write or a shell redirect/copy: agents use memory.sh write (the app's editor writes as the
# user, outside Claude). Sessions with no DCV2_RUN_DIR and no pipeline agent are not affected otherwise.
# Exit 2 blocks the tool call (stderr is shown to the model).
input="$(cat)"
PIPE_RE='"agent_type"[[:space:]]*:[[:space:]]*"(dcv2:)?(implementer|planner|plan-reviewer|pr-reviewer|memory-updater)"'

fallback() { # python3 missing or broken, or the payload did not parse: fail closed where it matters
  if grep -Eq "$PIPE_RE" <<<"$input"; then
    echo "Blocked by the dcv2 guard: this call could not be checked (python3 missing or broken, or an unreadable hook payload)." >&2
    exit 2
  fi
  if [[ -n "${DCV2_RUN_DIR:-}" ]] && grep -Eq 'git[^"]*push|gh[^"]*pr' <<<"$input"; then
    echo "Blocked by the dcv2 guard: push / PR commands cannot be checked (python3 missing or broken)." >&2
    exit 2
  fi
  exit 0
}

command -v python3 >/dev/null 2>&1 || fallback

IFS= read -r -d '' PY <<'PY' || true
import json, os, re, shlex, sys

try:
    data = json.loads(sys.stdin.read())
    if not isinstance(data, dict):
        raise ValueError("not an object")
except Exception:
    sys.exit(3)

ROLES = ("implementer", "planner", "plan-reviewer", "pr-reviewer", "memory-updater")
agent = str(data.get("agent_type") or "")
role = agent[len("dcv2:"):] if agent.startswith("dcv2:") else agent
pipeline = role in ROLES
tool = str(data.get("tool_name") or "")
inp = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
cwd0 = str(data.get("cwd") or "/")

def norm(p):
    """The identity of a path for every allow / deny decision: the real path (symlinks and .. resolved), casefolded
    (the default macOS volume ignores case, so STATUS.json is status.json and /Users/X is /Users/x)."""
    return os.path.realpath(p).casefold() if p else ""


def same(a, b):
    return bool(a) and bool(b) and norm(a) == norm(b)


run = os.environ.get("DCV2_RUN_DIR") or ""
run = os.path.realpath(run) if run else ""
# The app's signed ClickUp records of each run: <app data>/run-trust (the run is <app data>/runs/<id>).
trust_root = os.path.join(os.path.dirname(os.path.dirname(run)), "run-trust") if run else ""
TRUST_MSG = "the app's run records (run-trust) are written only by the app, never by Claude."
mem = os.environ.get("DCV2_MEMORY_DIR") or ""
mem = os.path.realpath(os.path.expanduser(mem)) if mem else ""
worktrees, repos, status, copied, mode = [], [], {}, set(), "local"
if run:
    try:
        g = json.load(open(os.path.join(run, ".guard.json")))
        if norm(str(g.get("runDir") or "")) == norm(run):
            wmap = {k: os.path.realpath(w) for k, w in (g.get("worktrees") or {}).items() if isinstance(w, str) and os.path.isabs(w)}
            worktrees = list(wmap.values())
            repos = [os.path.realpath(r) for r in g.get("repos") or [] if isinstance(r, str) and os.path.isabs(r)]
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
# Written only by state.sh (status, writeback, guard) or only by the app
# (the ClickUp snapshot and records, meta.json, the app's own check results).
PROTECTED = ("status.json", "writeback.json", ".guard.json",
             "tickets.json", "meta.json", "repos.json", "writeback-posted.json", "tickets-pulls.json", "tickets-update.md", "app-checks.json",
             "app-cleanup.json")
who = agent or "the orchestrator"
where = "your worktree or the run folder" if role == "implementer" else "the run folder"


def block(msg):
    print("Blocked by the dcv2 guard: " + msg, file=sys.stderr)
    sys.exit(2)


def real(p, base):
    return os.path.realpath(os.path.join(base, os.path.expanduser(p)))


def inside(p, root):
    if not root:
        return False
    p, root = norm(p), norm(root)
    return p == root or p.startswith(root.rstrip(os.sep) + os.sep)


def git_user_config(p):
    """The user's own git config (~/.gitconfig, $XDG_CONFIG_HOME/git/): the app's network git reads it (credential
    helpers run from there), so nothing in a run writes it."""
    home = os.path.realpath(os.path.expanduser("~"))
    xdg = os.path.realpath(os.environ.get("XDG_CONFIG_HOME") or os.path.join(home, ".config"))
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
    q = p
    while q and q != os.path.dirname(q):  # requests/ of any run folder: state.sh request and the app only
        if os.path.basename(q).casefold() == "requests" and (same(os.path.dirname(q), run) or has_request_md(os.path.dirname(q))):
            return True
        q = os.path.dirname(q)
    if os.path.basename(p).casefold() not in PROTECTED_CF:
        return False
    d = os.path.dirname(p)
    return same(d, run) or has_request_md(d)


def no_write_zone(p):
    """Bound runs (everyone, the orchestrator too) never write the plugin (its guard and hooks), Claude's own
    configuration, or the app's data outside the run folder (and, for the implementer, its worktrees)."""
    if not run:
        return None
    home = os.path.realpath(os.path.expanduser("~"))
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


GUARD_PLUGIN = os.path.realpath(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1] else ""


def write_allowed(p):
    if role == "memory-updater" and not run:
        return False
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


if pipeline and not run and role != "memory-updater":
    block(f"{who} runs only inside an Eagle-Eye run, and none is bound (DCV2_RUN_DIR is not set at launch).")

# ---- Edit / Write ------------------------------------------------------------------------
if tool in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
    path = inp.get("file_path") or inp.get("notebook_path") or ""
    if not path:
        if pipeline:
            block(f"{tool} without a file path")
        sys.exit(0)
    p = real(str(path), cwd0)
    if inside(p, trust_root):
        block(TRUST_MSG)
    if protected(p):
        block("write run-folder state only with scripts/state.sh (put / approve / writeback / worktree), never with Edit or Write.")
    if bmad_config(p):
        block(BMAD_CFG_MSG)
    if (run or pipeline) and git_user_config(p):
        block(GITCONFIG_MSG)
    if inside(p, mem):
        block("codebase memory is written only with `bash <plugin>/scripts/memory.sh write <repo id> <file> <<'EOF'` "
              "(it checks size caps, secrets and pinned lines), never with Edit or Write.")
    zone = no_write_zone(p)
    if zone:
        block(f"{p} is {zone}: it is never written from a bound run.")
    if not pipeline and run and main_checkout(p):
        block(f"the orchestrator never edits a repo's main checkout ({p}); sub-agents work in the run's worktrees.")
    if not pipeline or write_allowed(p):
        sys.exit(0)
    if role == "memory-updater" and not run:
        block("the memory updater writes nothing with Edit/Write outside a run; use memory.sh write for memory.")
    block(f"{who} may only write inside {where}, not {p}. Never edit a repo main checkout; use the exact worktree path you were given.")

bound = pipeline or bool(run)
if tool != "Bash" or not (bound or mem):
    sys.exit(0)

# ---- Bash --------------------------------------------------------------------------------
GIT_RO = {"status", "log", "diff", "show", "rev-parse", "rev-list", "ls-files", "ls-tree", "cat-file", "grep",
          "blame", "annotate", "describe", "shortlog", "merge-base", "name-rev", "for-each-ref", "show-ref",
          "whatchanged", "help", "version", "var", "check-ignore", "count-objects", "show-branch", "range-diff",
          "cherry", "diff-tree", "diff-files", "diff-index", "ls-remote", "verify-commit"}
GH_WRITES = {"create", "comment", "edit", "close", "reopen", "delete", "merge", "lock", "transfer", "pin", "fork",
             "upload", "set", "add", "remove", "rename", "archive", "sync", "run", "enable", "disable"}
WRAPPERS = {"sudo", "env", "command", "nohup", "time", "exec", "builtin", "nice", "then", "do", "else", "!"}
SAFE_DEV = ("/dev/null", "/dev/stdout", "/dev/stderr", "/dev/tty")


def strip_heredocs(cmd):
    out, skip = [], None
    for line in cmd.split("\n"):
        if skip is not None:
            if line.strip() == skip:
                skip = None
            continue
        out.append(line)
        m = re.search(r"(?<!<)<<(?!<)-?\s*['\"]?(\w+)['\"]?", line)
        if m:
            skip = m.group(1)
    return "\n".join(out)


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
        block(f"{what} is allowed only at shipping, after the user's ship approval (stage {stage}, shipApproval "
              f"{'set' if appr else 'missing'}). Ship with `state.sh ship-pr <run> <pr id>`.")


def check_write(t, cwd):
    if t in SAFE_DEV or t.startswith("/dev/fd/"):
        return
    if bound and re.search(r"[$`*?]", t) and re.search(r"\.gitconfig|\.config/git|/git/config\b", t):
        block(GITCONFIG_MSG)
    if re.search(r"[$`*?]", t) or (cwd is None and not os.path.isabs(os.path.expanduser(t))):
        if trust_root and "run-trust" in t.lower():
            block(TRUST_MSG)
        if mem and re.search(r"\$\{?DCV2_MEMORY_DIR\b", t):
            block("codebase memory is written only with memory.sh write, never with a shell redirect.")
        if mem and cwd is None and MEM_MENTIONED:
            block("cannot tell where this relative write lands (the command changes into an unknown folder and mentions "
                  "the memory folder); write memory only with memory.sh write.")
        if pipeline:
            block(f"cannot check the write target {t!r}: write to a literal absolute path")
        return
    p = real(t, cwd)
    if inside(p, trust_root):
        block(TRUST_MSG)
    if inside(p, mem):
        block("codebase memory is written only with memory.sh write, never with a shell redirect, copy or move.")
    if not bound:
        return
    if git_user_config(p):
        block(GITCONFIG_MSG)
    zone = no_write_zone(p)
    if zone:
        block(f"{p} is {zone}: it is never written from a bound run.")
    if protected(p):
        block("write run-folder state only with scripts/state.sh, never with a shell redirect or copy.")
    if bmad_config(p):
        block(BMAD_CFG_MSG)
    if not pipeline and main_checkout(p):
        block(f"the orchestrator never writes into a repo's main checkout ({p}).")
    if pipeline and not write_allowed(p) and in_checkout(p):
        block(f"{who} may not write {p} from Bash: it is in a git checkout outside {where}.")


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
        block(f"{who} never runs `git worktree {' '.join(rest[:1])}`: {how}")
    if sub in ("update-ref", "prune", "gc", "replace") or (sub == "symbolic-ref" and not git_readonly(sub, rest)) \
            or (sub == "reflog" and not git_readonly(sub, rest)) or (sub == "tag" and not git_readonly(sub, rest)):
        block(f"{who} never runs `git {sub}` (it changes refs every worktree of the repo shares): {how}")
    if sub == "notes" and rest[:1] not in ([], ["list"], ["show"]):
        block(f"{who} never writes git notes (they are shared by every worktree).")
    mine = own_branch(target) if role == "implementer" else None
    if sub in ("fetch", "pull"):
        refs = [a for a in rest if not a.startswith("-")][1:]
        for ref in refs:
            dst = ref.split(":", 1)[1] if ":" in ref else ""
            dst = dst[len("refs/heads/"):] if dst.startswith("refs/heads/") else dst
            if ":" in ref and dst and not dst.startswith("refs/remotes/") and dst != mine:
                block(f"{who} never fetches into a local branch (`git {sub} … {ref}`): branches are shared by every worktree.")
    if sub == "branch" and not git_readonly(sub, rest):
        names = [a for a in rest if not a.startswith("-")]
        if not (mine and not [a for a in rest if a.startswith("-")] and names == [mine]):
            block(f"{who} never changes branches with `git branch` (they are shared by every worktree): {how}")
    if sub in ("checkout", "switch"):
        for i, a in enumerate(rest):
            flag = a.split("=", 1)[0]
            if flag in ("-b", "-B", "-c", "-C", "--orphan", "--create", "--force-create") or re.match(r"^-[a-zA-Z]*[bBcC]$", a):
                name = a.split("=", 1)[1] if "=" in a else (rest[i + 1] if i + 1 < len(rest) else "")
                if not mine or name != mine:
                    block(f"{who} never creates or resets a branch (`git {sub} {a} {name}`): branches are shared by every worktree.")


def check_git(args, cwd):
    if not bound:
        return
    target, i = cwd, 0
    while i < len(args):
        a = args[i]
        if a == "-C" and i + 1 < len(args):
            v = os.path.expanduser(args[i + 1])
            target = None if "$" in v else (os.path.realpath(v) if os.path.isabs(v) else (real(v, target) if target else None))
            i += 2
            continue
        if a == "--config-env" or a.startswith("--config-env="):
            block("`git --config-env` is never used in a dcv2 run.")
        if a.startswith("--exec-path="):
            block("`git --exec-path=<dir>` is never used in a dcv2 run.")
        if a == "-c" or (a.startswith("-c") and len(a) > 2):
            kv = args[i + 1] if a == "-c" and i + 1 < len(args) else a[2:]
            if not git_c_ok(kv):
                block(f"`git -c {kv.split('=', 1)[0]}=…` is not allowed in a dcv2 run (only identity, colour, advice and "
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
            block(f"{who} never pushes; the orchestrator ships with state.sh ship-pr after the user's approval.")
        if any(a in ("-f", "--force", "--force-with-lease", "--force-if-includes", "--mirror", "--delete", "-d", "--prune")
               or a.startswith("--force-with-lease=") or a.startswith("+") or (":" in a and a.startswith(":"))
               or re.match(r"^-[a-zA-Z]*[fd]", a) for a in rest):
            block("force pushes and remote branch deletes are never allowed.")
        if run and (target is None or main_checkout(target)):
            block(f"push only from the PR's worktree, never from a repo's main checkout or an unknown folder ({target or 'unknown'}).")
        if any(a in ("--all", "--branches", "--tags") for a in rest):
            block("push one dcv2/ branch at a time (no --all / --tags).")
        refs = [a for a in rest if not a.startswith("-")][1:]
        for ref in refs:
            dst = ref.split(":", 1)[-1]
            dst = dst[len("refs/heads/"):] if dst.startswith("refs/heads/") else dst
            if dst != "HEAD" and not dst.startswith("dcv2/") or (dst == "HEAD" and ":" in ref):
                block(f"the run pushes only its own dcv2/ branches (not {ref!r}); ship with state.sh ship-pr.")
        if mode == "github":
            block("this run reviews on GitHub: pushes go only through state.sh (draft-pr / push / ready), which check the stage and approvals.")
        ship_gate("git push")
        return
    shared_refs(sub, rest, target)
    if not pipeline:
        if run and sub in ("remote", "config") and not git_readonly(sub, rest):
            block(f"the orchestrator never changes git remotes or config (`git {sub} ...`): they are shared by the repo and "
                  "its worktrees, and every push goes to the remote recorded for the run.")
        if run and target and main_checkout(target) and not git_readonly(sub, rest):
            block(f"the orchestrator never runs `git {sub}` in a repo's main checkout ({target}); worktrees go through state.sh worktree.")
        return
    if role == "implementer" and target and any(inside(target, w) for w in worktrees):
        if sub in ("remote", "config") and not git_readonly(sub, rest):
            block(f"{who} never changes git remotes or config (`git {sub} ...`); pushes go to the remote recorded for the run.")
        if sub in ("add", "stage", "commit") and copied:
            force = any(a == "--force" or (re.match(r"^-[a-zA-Z]+$", a) and "f" in a) for a in rest)
            for a in rest:
                if a.startswith("-") or "$" in a:
                    continue
                p = norm(os.path.join(target, os.path.expanduser(a)))
                if p in copied or (force and any(c.startswith(p.rstrip(os.sep) + os.sep) for c in copied)):
                    block(f"`git {sub}` would include {a!r}, a file copied in from the main checkout (repos.json copyFiles): "
                          "it is never staged or committed. Stage only the files your story changed.")
        return
    if git_readonly(sub, rest):
        return
    block(f"{who} may not run `git {sub}` in {target or 'an unknown folder'}: "
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
            block(f"{who} never calls `gh {sub}`; the orchestrator opens PRs at shipping.")
        if verb in GH_WRITES:
            block(f"{who} never changes anything on GitHub (`gh {sub} {verb}`).")
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
            block("the run never writes to GitHub with `gh api`; every GitHub write goes through a state.sh command.")
        return
    if sub == "pr" and verb in ("merge", "close", "edit", "ready", "reopen", "review", "comment", "lock"):
        block(f"the run never runs `gh pr {verb}` itself: it never merges, closes or edits PRs"
              + ("; drafts, comments and `ready` go through state.sh." if mode == "github" else "."))
    if sub == "pr" and verb == "create":
        if mode == "github":
            block("this run reviews on GitHub: PRs are opened only by `state.sh draft-pr` / `state.sh ready`.")
        ship_gate("gh pr create")
        return
    if verb in GH_WRITES:
        block(f"the run never changes anything on GitHub with `gh {sub} {verb}`; GitHub writes go through state.sh.")


GH_STATE = ("draft-pr", "push", "post-review", "comment")


def check_state_sh(args):
    """GitHub writes through state.sh: never by a sub-agent; the orchestrator's only in github mode, after the plan /
    ship approval (state.sh checks too)."""
    sub = next((a for a in args[1:] if not a.startswith("-")), "")
    if pipeline:
        if sub in GH_STATE + ("ready", "ship-pr", "cleanup", "request"):
            block(f"{who} never runs `state.sh {sub}`: only the orchestrator touches GitHub.")
        return
    if sub in GH_STATE:
        if mode != "github":
            block(f"`state.sh {sub}` is for runs that review on GitHub; this run's review mode is local.")
        plan = status.get("plan") if isinstance(status.get("plan"), dict) else {}
        if not plan.get("approval"):
            block(f"`state.sh {sub}` needs the plan approval first (stop #1).")
    if sub == "ready" and not status.get("shipApproval"):
        block("`state.sh ready` needs the ship approval first (stop #2).")
    if sub in ("cleanup", "request", "request-wait") and run:
        rest = [a for a in args[1:] if not a.startswith("-")]
        target = re.sub(r"\$\{?DCV2_RUN_DIR\}?", lambda _: run, rest[1] if len(rest) > 1 else "")
        if "$" not in target and not same(os.path.expanduser(target), run):
            block(f"`state.sh {sub}` acts only on this run's folder ({run}), not {target or 'none'}.")


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
                block(f"{who} never runs the BMAD runtime: only the planner does, for its bmad pass "
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
        block(f"{who} never runs the live Claude tests ({os.path.basename(script)}).")
    if "$" in script or (cwd is None and not os.path.isabs(os.path.expanduser(script))):
        block(f"cannot tell where {script!r} is: run it as `cd <worktree> && bash scripts/skill-e2e/{os.path.basename(script)} ...`")
    p = real(script, cwd or "/")
    wt = next((w for w in worktrees if inside(p, w)), None)
    if not wt or os.path.relpath(norm(p), norm(wt)) != os.path.join("scripts", "skill-e2e", os.path.basename(norm(p))):
        block(f"live Claude tests run only as your worktree's scripts/skill-e2e/{os.path.basename(script)}.")
    if not live_tests_on(wt):
        block("live Claude tests are off for this repo (the app setting \"Allow live Claude tests in checks\"); "
              "they stay a manual step for the user.")


SHELLS = ("bash", "sh", "zsh", "dash", "ksh")
INTERPRETERS = ("python", "python3", "node", "nodejs", "perl", "ruby", "osascript", "deno", "bun")
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
# Programs that run the next word as a command, and their options that take a value.
RUNNERS = {"nohup": (), "caffeinate": (), "stdbuf": (), "command": (), "exec": ("-a",), "builtin": (), "sudo": ("-u", "-g", "-C"),
           "doas": ("-u",), "time": (), "env": ("-u", "--unset", "-C", "--chdir", "-S", "--split-string"),
           "nice": ("-n", "--adjustment"), "timeout": ("-s", "--signal", "-k", "--kill-after"), "npx": ("-p", "--package"),
           "bunx": (), "unbuffer": (), "chronic": ()}


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
                    block("could not parse `env -S` text")
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
        block("the app's keychain items are never read from inside a run.")


def analyze(cmd, cwd, depth=0):
    if depth > 4:
        if pipeline:
            block("command nesting too deep to check")
        return cwd
    text = strip_heredocs(cmd)
    try:
        toks = lex(text)
    except ValueError:
        if pipeline:
            block("could not parse this command (unbalanced quotes?); write it more simply")
        if bound and re.search(r"\bgit\b.*\bpush\b|\bgh\b.*\bpr\b", text):
            block("could not parse this push / PR command")
        return cwd
    segs, seg = [], []
    for t in toks:
        if not is_punct(t):
            seg.append(t)
            continue
        for op in re.findall(r"&>>|&>|>>|>&|>\||<<<|<<|<&|<>|>|<|&&|\|\||\|&|[;|()\n`&]", t):
            if "<" in op or ">" in op:
                seg.append(op)
            else:
                segs.append(seg)
                seg = []
    segs.append(seg)
    for seg in segs:
        words, targets, i = [], [], 0
        while i < len(seg):
            t = seg[i]
            if is_punct(t):
                nxt = seg[i + 1] if i + 1 < len(seg) and not is_punct(seg[i + 1]) else None
                if "<" in t and ">" not in t:
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
            for w in words:
                if GIT_ENV_RE.match(w):
                    block(f"setting {w.split('=', 1)[0]} is not allowed in a dcv2 run: it changes which repo git works on "
                          "or which programs it runs.")
        words = unwrap(words)
        if not words:
            continue
        name, args = os.path.basename(words[0]), words[1:]
        script = shell_script(args) if name in SHELLS else (args[0] if name in ("source", ".") and args else None)
        if pipeline:
            if "$" in words[0] and "claude" in CMD:
                block(f"cannot tell which program `{words[0]}` is, and the command mentions claude: {who} never launches claude.")
            if any("@anthropic-ai/claude-code" in w for w in words) or name == "claude-code":
                block(f"{who} never launches claude (the @anthropic-ai/claude-code package).")
            if name in ("find",):
                for i, a in enumerate(args):
                    if a in ("-exec", "-execdir", "-ok", "-okdir") and i + 1 < len(args):
                        analyze(" ".join(shlex.quote(x) for x in args[i + 1:] if x not in (";", "+")), cwd, depth + 1)
        if bound and any("ade-runtime.mjs" in w for w in words):
            block("ade-runtime.mjs is the bundle behind the BMAD wrapper and bypasses its write checks; run `node <plugin>/bmad/runtime.mjs <command>` (planner only).")
        if name in ("cd", "pushd"):
            d = next((a for a in args if not a.startswith("-")), "~")
            if mem and re.search(r"\$\{?DCV2_MEMORY_DIR\b", d):
                cwd = mem
                continue
            cwd = None if ("$" in d or cwd is None and not os.path.isabs(os.path.expanduser(d))) else real(d, cwd or "/")
        elif name == "git":
            check_git(args, cwd)
        elif name == "gh":
            check_gh(args)
        elif name == "security" and bound:
            check_security(args)
        elif name in ("node", "nodejs") and bound:
            check_node(args, cwd)
        elif name == "claude" and pipeline:
            block(f"{who} never launches claude. Live Claude tests run only as `bash scripts/skill-e2e/golden.sh` or "
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
    return cwd


CMD = str(inp.get("command") or "")
if bound and "DCV2_TEST_LOCAL_REMOTES" in CMD:
    block("DCV2_TEST_LOCAL_REMOTES is set only by the test suites, never from inside a run.")
MEM_MENTIONED = bool(mem) and ("DCV2_MEMORY_DIR" in CMD or mem.casefold() in CMD.casefold())
analyze(CMD, cwd0)
sys.exit(0)
PY

python3 -c "$PY" "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." 2>/dev/null && pwd -P)" <<<"$input"
rc=$?
case $rc in
  0|2) exit $rc ;;
  *) fallback ;;
esac
