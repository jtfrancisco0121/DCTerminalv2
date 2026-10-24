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
# Everyone in a run: no `security dump-keychain`, and no `security find-*-password` naming dcterminalv2.
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
        if os.path.realpath(str(g.get("runDir") or "")) == run:
            wmap = {k: os.path.realpath(w) for k, w in (g.get("worktrees") or {}).items() if isinstance(w, str) and os.path.isabs(w)}
            worktrees = list(wmap.values())
            repos = [os.path.realpath(r) for r in g.get("repos") or [] if isinstance(r, str) and os.path.isabs(r)]
            for k, files in (g.get("copied") or {}).items():  # files copied in from the main checkout (never committed)
                if k in wmap and isinstance(files, list):
                    copied.update(os.path.normpath(os.path.join(wmap[k], str(f))) for f in files)
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
             "tickets.json", "meta.json", "writeback-posted.json", "tickets-pulls.json", "tickets-update.md", "app-checks.json")
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
    if role == "memory-updater" and not run:
        return False
    if inside(p, run):
        return True
    return role == "implementer" and any(inside(p, w) for w in worktrees)


def main_checkout(p):
    """Inside a repo's main checkout recorded for this run (worktrees live beside the repo, not in it)."""
    return any(inside(p, r) for r in repos)


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
    if inside(p, mem):
        block("codebase memory is written only with `bash <plugin>/scripts/memory.sh write <repo id> <file> <<'EOF'` "
              "(it checks size caps, secrets and pinned lines), never with Edit or Write.")
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
    if re.search(r"[$`*?]", t) or (cwd is None and not os.path.isabs(os.path.expanduser(t))):
        if trust_root and "run-trust" in t:
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
    if protected(p):
        block("write run-folder state only with scripts/state.sh, never with a shell redirect or copy.")
    if not pipeline and main_checkout(p):
        block(f"the orchestrator never writes into a repo's main checkout ({p}).")
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
    if not pipeline:
        if run and target and main_checkout(target) and not git_readonly(sub, rest) and sub != "worktree":
            block(f"the orchestrator never runs `git {sub}` in a repo's main checkout ({target}); worktrees go through state.sh worktree.")
        return
    if role == "implementer" and target and any(inside(target, w) for w in worktrees):
        if sub in ("add", "stage", "commit") and copied:
            force = any(a == "--force" or (re.match(r"^-[a-zA-Z]+$", a) and "f" in a) for a in rest)
            for a in rest:
                if a.startswith("-") or "$" in a:
                    continue
                p = os.path.normpath(os.path.join(target, os.path.expanduser(a)))
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
        method = None
        for j, a in enumerate(args):
            if a in ("-X", "--method") and j + 1 < len(args):
                method = args[j + 1]
            elif a.startswith("--method="):
                method = a.split("=", 1)[1]
            elif re.match(r"^-X\S", a):
                method = a[2:]
        fields = any(a in ("-f", "-F", "--field", "--raw-field", "--input") or
                     re.match(r"^(-[fF]\S|--field=|--raw-field=|--input=)", a) for a in args)
        if (method and method.upper() != "GET") or fields:
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
    """The orchestrator's GitHub writes through state.sh: github mode, after the plan / ship approval (state.sh checks too)."""
    sub = next((a for a in args[1:] if not a.startswith("-")), "")
    if sub in GH_STATE:
        if mode != "github":
            block(f"`state.sh {sub}` is for runs that review on GitHub; this run's review mode is local.")
        plan = status.get("plan") if isinstance(status.get("plan"), dict) else {}
        if not plan.get("approval"):
            block(f"`state.sh {sub}` needs the plan approval first (stop #1).")
    if sub == "ready" and not status.get("shipApproval"):
        block("`state.sh ready` needs the ship approval first (stop #2).")


def check_security(args):
    """No reading the app's keychain items (ClickUp token, trust key) from inside a run."""
    sub = next((a for a in args if not a.startswith("-")), "")
    if sub == "dump-keychain" or (sub in ("find-generic-password", "find-internet-password", "export")
                                  and any("dcterminalv2" in a.lower() for a in args)):
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
        while words and (re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", words[0]) or words[0] in WRAPPERS):
            words = words[1:]
        if not words:
            continue
        name, args = os.path.basename(words[0]), words[1:]
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
        elif name in ("bash", "sh") and args and os.path.basename(args[0]) == "state.sh" and run and not pipeline:
            check_state_sh(args)
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


CMD = str(inp.get("command") or "")
MEM_MENTIONED = bool(mem) and ("DCV2_MEMORY_DIR" in CMD or mem in CMD)
analyze(CMD, cwd0)
sys.exit(0)
PY

python3 -c "$PY" <<<"$input"
rc=$?
case $rc in
  0|2) exit $rc ;;
  *) fallback ;;
esac
