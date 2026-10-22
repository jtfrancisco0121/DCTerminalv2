#!/usr/bin/env bash
# PreToolUse guard for the eagle-eye run (plugin hook: Edit, Write, NotebookEdit, MultiEdit, Bash).
#
# The run it guards is the one named by the env DCV2_RUN_DIR, set when claude is launched
# (the app and the e2e test set it; `state.sh preflight` refuses to start a run without it).
# Hooks inherit it from the claude process, so the orchestrator cannot change it and ticket
# text cannot reach it. The run's allowed worktrees are listed in <run>/.guard.json, which
# only `state.sh worktree` writes.
#
# Pipeline sub-agents (agent_type implementer, planner, plan-reviewer, pr-reviewer, with or
# without the "dcv2:" prefix):
#   - Edit/Write: the real path (symlinks and .. resolved) must be inside the run folder, or,
#     for the implementer only, inside one of the run's recorded worktrees.
#   - Bash: never `git push`, `gh pr`, `gh api` (or gh issue/repo/release writes). Git commands
#     that change anything are allowed only for the implementer inside a recorded worktree;
#     elsewhere only read-only git. Redirects, tee, sed -i, cp/mv/rm/touch/mkdir etc. into a
#     git checkout (a repo's main checkout, another worktree) are blocked unless the target is
#     an allowed place. Best effort: commands that write through other programs (node, python,
#     npm scripts, xargs) are not seen.
#   - No DCV2_RUN_DIR, python3 missing, or a payload that does not parse: blocked (fail closed).
# The orchestrator (no agent_type) in a bound run: `git push` and `gh pr create` only when
# status.json has stage shipping and a shipApproval (state.sh ship-pr does both itself); never
# a force push or a branch delete; never `gh pr merge|close|edit`.
# Everyone: status.json, writeback.json and .guard.json in a run folder are written only by
# state.sh. Sessions with no DCV2_RUN_DIR and no pipeline agent are not affected otherwise.
# Exit 2 blocks the tool call (stderr is shown to the model).
input="$(cat)"
PIPE_RE='"agent_type"[[:space:]]*:[[:space:]]*"(dcv2:)?(implementer|planner|plan-reviewer|pr-reviewer)"'

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

ROLES = ("implementer", "planner", "plan-reviewer", "pr-reviewer")
agent = str(data.get("agent_type") or "")
role = agent[len("dcv2:"):] if agent.startswith("dcv2:") else agent
pipeline = role in ROLES
tool = str(data.get("tool_name") or "")
inp = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
cwd0 = str(data.get("cwd") or "/")

run = os.environ.get("DCV2_RUN_DIR") or ""
run = os.path.realpath(run) if run else ""
worktrees, repos, status = [], [], {}
if run:
    try:
        g = json.load(open(os.path.join(run, ".guard.json")))
        if os.path.realpath(str(g.get("runDir") or "")) == run:
            worktrees = [os.path.realpath(w) for w in (g.get("worktrees") or {}).values() if isinstance(w, str) and os.path.isabs(w)]
            repos = [os.path.realpath(r) for r in g.get("repos") or [] if isinstance(r, str) and os.path.isabs(r)]
    except Exception:
        pass
    try:
        status = json.load(open(os.path.join(run, "status.json")))
        status = status if isinstance(status, dict) else {}
    except Exception:
        status = {}
PROTECTED = ("status.json", "writeback.json", ".guard.json")
who = agent or "the orchestrator"
where = "your worktree or the run folder" if role == "implementer" else "the run folder"


def block(msg):
    print("Blocked by the dcv2 guard: " + msg, file=sys.stderr)
    sys.exit(2)


def real(p, base):
    return os.path.realpath(os.path.join(base, os.path.expanduser(p)))


def inside(p, root):
    return bool(root) and (p == root or p.startswith(root.rstrip(os.sep) + os.sep))


def protected(p):
    if os.path.basename(p) not in PROTECTED:
        return False
    d = os.path.dirname(p)
    return d == run or os.path.isfile(os.path.join(d, "request.md"))


def write_allowed(p):
    if inside(p, run):
        return True
    return role == "implementer" and any(inside(p, w) for w in worktrees)


def in_checkout(p):
    if any(inside(p, r) for r in repos) or any(c.endswith("-worktrees") for c in p.split(os.sep)):
        return True
    d = p
    while True:
        if os.path.exists(os.path.join(d, ".git")):
            return True
        nd = os.path.dirname(d)
        if nd == d:
            return False
        d = nd


if pipeline and not run:
    block(f"{who} runs only inside an Eagle-Eye run, and none is bound (DCV2_RUN_DIR is not set at launch).")

# ---- Edit / Write ------------------------------------------------------------------------
if tool in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
    path = inp.get("file_path") or inp.get("notebook_path") or ""
    if not path:
        if pipeline:
            block(f"{tool} without a file path")
        sys.exit(0)
    p = real(str(path), cwd0)
    if protected(p):
        block("write run-folder state only with scripts/state.sh (put / approve / writeback / worktree), never with Edit or Write.")
    if not pipeline or write_allowed(p):
        sys.exit(0)
    block(f"{who} may only write inside {where}, not {p}. Never edit a repo main checkout; use the exact worktree path you were given.")

if tool != "Bash" or not (pipeline or run):
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
    if re.search(r"[$`*?]", t) or (cwd is None and not os.path.isabs(os.path.expanduser(t))):
        if pipeline:
            block(f"cannot check the write target {t!r}: write to a literal absolute path")
        return
    p = real(t, cwd)
    if protected(p):
        block("write run-folder state only with scripts/state.sh, never with a shell redirect or copy.")
    if pipeline and not write_allowed(p) and in_checkout(p):
        block(f"{who} may not write {p} from Bash: it is in a git checkout outside {where}.")


def write_targets(name, args):
    nonopt = [a for a in args if a and not a.startswith("-")]
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
        return any(a in ("--get", "--get-all", "--get-regexp", "--list", "-l") for a in rest)
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


def check_git(args, cwd):
    target, i = cwd, 0
    while i < len(args):
        a = args[i]
        if a == "-C" and i + 1 < len(args):
            v = os.path.expanduser(args[i + 1])
            target = None if "$" in v else (os.path.realpath(v) if os.path.isabs(v) else (real(v, target) if target else None))
            i += 2
            continue
        if a in ("-c", "--git-dir", "--work-tree", "--namespace") and i + 1 < len(args):
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
        ship_gate("git push")
        return
    if not pipeline:
        return
    if role == "implementer" and target and any(inside(target, w) for w in worktrees):
        return
    if git_readonly(sub, rest):
        return
    block(f"{who} may not run `git {sub}` in {target or 'an unknown folder'}: "
          + ("outside its worktree only read-only git commands are allowed." if role == "implementer"
             else "it may only run read-only git commands."))


def check_gh(args):
    words = [a for a in args if not a.startswith("-")]
    sub, verb = (words + ["", ""])[:2]
    if pipeline:
        if sub in ("pr", "api"):
            block(f"{who} never calls `gh {sub}`; the orchestrator opens PRs at shipping.")
        if verb in GH_WRITES:
            block(f"{who} never changes anything on GitHub (`gh {sub} {verb}`).")
        return
    if sub == "pr" and verb in ("merge", "close", "edit", "ready", "reopen", "review", "comment", "lock"):
        block(f"the run never runs `gh pr {verb}`: it never merges, closes or edits PRs.")
    if sub == "pr" and verb == "create":
        ship_gate("gh pr create")


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
        if re.search(r"\bgit\b.*\bpush\b|\bgh\b.*\bpr\b", text):
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
        while words and (re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", words[0]) or words[0] in WRAPPERS):
            words = words[1:]
        if not words:
            continue
        name, args = os.path.basename(words[0]), words[1:]
        if name in ("cd", "pushd"):
            d = next((a for a in args if not a.startswith("-")), "~")
            cwd = None if ("$" in d or cwd is None and not os.path.isabs(os.path.expanduser(d))) else real(d, cwd or "/")
        elif name == "git":
            check_git(args, cwd)
        elif name == "gh":
            check_gh(args)
        elif name in ("bash", "sh", "zsh", "dash", "ksh") and "-c" in args and args.index("-c") + 1 < len(args):
            analyze(args[args.index("-c") + 1], cwd, depth + 1)
        elif name == "eval":
            analyze(" ".join(args), cwd, depth + 1)
        elif name == "xargs" and args:
            rest = [a for a in args if not a.startswith("-")]
            if rest:
                analyze(" ".join(rest), cwd, depth + 1)
        else:
            for t in write_targets(name, args):
                check_write(t, cwd)
    return cwd


analyze(str(inp.get("command") or ""), cwd0)
sys.exit(0)
PY

python3 -c "$PY" <<<"$input"
rc=$?
case $rc in
  0|2) exit $rc ;;
  *) fallback ;;
esac
