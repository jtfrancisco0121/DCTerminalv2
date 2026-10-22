#!/usr/bin/env bash
# Codebase memory (D9 + A6). The only code that knows the memory layout. Needs python3.
#
# Layout under the memory root ($DCV2_MEMORY_DIR, or --root <dir> anywhere in the arguments):
#   <repo id>/index.md                 always loaded: what the repo is, map, commands, gotchas, topic list
#   <repo id>/topics/<topic>.md        on demand; header: "covers: <paths>" + "checked-at: <commit>"
#   <repo id>/topics/contracts.md      binding (A6): rules every agent must follow
#   workspaces/<ws id>/workspace.md    a folder of repos: how they connect (binding)
#   workspaces/<ws id>/topics/*.md     workspace topics; contracts.md is binding
#
#   memory.sh id <repo path>                     print the repo id
#   memory.sh compose <start folder> <out file>  write the launch system-prompt file (bounded)
#   memory.sh status <start folder>              JSON for the app panel
#   memory.sh write <id | workspace:<id>> <relative file>  < content
#                                                the only write path for agents: caps, secret scan,
#                                                pinned lines, topic header; rejects with a reason
#   memory.sh touched <repo path> <from> [<to>]  agent helper: topics whose covered paths changed in
#                                                `git diff <from>..<to>` (to defaults to HEAD), and
#                                                changed paths no topic covers
#
# Errors: exit 1 with one stderr line. Nothing secret is printed.
set -euo pipefail

command -v python3 >/dev/null 2>&1 && python3 -c 'import sys' >/dev/null 2>&1 || {
  echo "memory.sh: python3 is missing or does not run" >&2; exit 1; }

IFS= read -r -d '' PY <<'PY' || true
import fnmatch, hashlib, json, os, re, subprocess, sys, tempfile

CAPS = {"index": 8 * 1024, "topic": 12 * 1024, "workspace": 8 * 1024}
COMPOSE_CAP = 40 * 1024
PIN = "<!-- pinned -->"
TOPIC_RE = re.compile(r"^topics/([a-z0-9][a-z0-9-]{0,62})\.md$")
SCRIPT = os.path.realpath(os.environ.get("DCV2_MEMORY_SH") or "")
TRUST = ("Trust rule: memory is a hint, the code is the truth. If a remembered file, function or command is not "
         "where memory says, trust the code, fix the memory entry (memory.sh write), and continue.")

SECRETS = [
    ("a private key", re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")),
    ("an AWS access key id", re.compile(r"\b(AKIA|ASIA|AGPA|AIDA|AROA)[0-9A-Z]{16}\b")),
    ("an AWS secret key", re.compile(r"(?i)aws_?secret_?access_?key\s*[=:]\s*\S{8,}")),
    ("a GitHub token", re.compile(r"\b(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})")),
    ("an API key (sk-)", re.compile(r"\bsk-[A-Za-z0-9_-]{16,}")),
    ("a token (pk_/sk_/rk_)", re.compile(r"\b(pk|sk|rk)_(live_|test_)?[A-Za-z0-9_]{12,}")),
    ("a Slack token", re.compile(r"\bxox[abposr]-[A-Za-z0-9-]{10,}")),
    ("a Google API key", re.compile(r"\bAIza[0-9A-Za-z_-]{30,}")),
    ("a JWT", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+")),
    ("a password in a URL", re.compile(r"[a-z][a-z0-9+.-]*://[^/\s:@]+:[^/\s@]{3,}@")),
    ("a secret assignment (.env style)", re.compile(
        r"(?i)(^|[\s\"'{,])(export\s+)?[A-Za-z0-9_]*(secret|token|passw(or)?d|api_?key|private_?key|access_?key|client_?secret|auth_?key|credential)[A-Za-z0-9_]*"
        r"\s*[=:]\s*[\"']?(?![$<{%]|\*+|x{3,}|\.\.\.|changeme|example|placeholder|your[_-]|none\b|null\b|true\b|false\b)[^\s\"'`<>]{8,}")),
]


def die(msg):
    print("memory.sh: " + msg, file=sys.stderr)
    sys.exit(1)


def git(*args, cwd=None):
    try:
        return subprocess.run(["git", *args], capture_output=True, text=True, cwd=cwd, timeout=30)
    except (OSError, subprocess.TimeoutExpired) as e:
        return subprocess.CompletedProcess(args, 1, "", str(e))


def read(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return None


def atomic_write(dest, text):
    d = os.path.dirname(os.path.abspath(dest))
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix="." + os.path.basename(dest) + ".", suffix=".tmp", dir=d)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(text)
        os.replace(tmp, dest)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def secret_hits(text):
    """(line number, kind) for every line that looks like a secret; never the value."""
    hits = []
    for n, line in enumerate(text.splitlines(), 1):
        for kind, rx in SECRETS:
            if rx.search(line):
                hits.append((n, kind))
                break
    return hits


def redact(text):
    lines = text.splitlines()
    for n, _ in secret_hits(text):
        lines[n - 1] = "[line removed by memory.sh: it looks like a secret]"
    return "\n".join(lines)


# ---- ids and folders ---------------------------------------------------------------------

def slug(s):
    return re.sub(r"[^a-z0-9._-]+", "-", s.lower()).strip("-.") or "x"


def toplevel(path):
    r = git("-C", path, "rev-parse", "--show-toplevel")
    return os.path.realpath(r.stdout.strip()) if r.returncode == 0 and r.stdout.strip() else None


def url_id(url):
    url = url.strip()
    m = re.match(r"^[a-z][a-z0-9+.-]*://(?:[^@/]*@)?([^/:]+)(?::\d+)?/(.+)$", url, re.I)
    if not m:
        m = re.match(r"^(?:[^@/]+@)?([^/:]+):(?!/)(.+)$", url)  # scp-like: git@host:owner/name
    if not m or url.startswith("file:"):
        return None
    host, path = m.group(1), m.group(2)
    parts = [p for p in path.strip("/").split("/") if p]
    if parts and parts[-1].lower().endswith(".git"):
        parts[-1] = parts[-1][:-4]
    if len(parts) < 2 or "." not in host and host != "localhost":
        return None
    return "__".join(slug(p) for p in [host] + parts)


def repo_id(path):
    real = os.path.realpath(path)
    if not os.path.isdir(real):
        die(f"no folder {path}")
    top = toplevel(real)
    if top:
        real = top
        r = git("-C", real, "remote", "get-url", "origin")
        if r.returncode == 0:
            rid = url_id(r.stdout)
            if rid:
                return rid
    h = hashlib.sha1(real.encode()).hexdigest()[:8]
    return f"local__{slug(os.path.basename(real))}-{h}"


def in_play(folder):
    """(workspace folder or None, [repo paths]) for a start folder."""
    real = os.path.realpath(folder)
    if not os.path.isdir(real):
        die(f"no folder {folder}")
    top = toplevel(real)
    if top:
        return None, [top]
    repos = []
    for name in sorted(os.listdir(real)):
        p = os.path.join(real, name)
        if not name.startswith(".") and os.path.isdir(p) and os.path.exists(os.path.join(p, ".git")):
            repos.append(os.path.realpath(p))
    return real, repos


def root_dir(required=True):
    r = ROOT or os.environ.get("DCV2_MEMORY_DIR") or ""
    if not r:
        if required:
            die("no memory root: set DCV2_MEMORY_DIR or pass --root <dir>")
        return None
    return os.path.realpath(os.path.expanduser(r))


# ---- topics ------------------------------------------------------------------------------

def topic_header(text):
    """{covers: [...], checkedAt: str|None} from the first lines of a topic file."""
    covers, checked = [], None
    for line in text.splitlines()[:12]:
        m = re.match(r"^\s*covers:\s*(.+?)\s*$", line, re.I)
        if m:
            covers = [c.strip().strip("`") for c in m.group(1).split(",") if c.strip()]
        m = re.match(r"^\s*checked-at:\s*(.+?)\s*$", line, re.I)
        if m:
            checked = m.group(1).strip("` ")
    return {"covers": covers, "checkedAt": checked}


def covered(path, pattern):
    pattern = pattern.rstrip("/") if not any(c in pattern for c in "*?[") else pattern
    if any(c in pattern for c in "*?["):
        return fnmatch.fnmatch(path, pattern) or fnmatch.fnmatch(path, pattern.rstrip("/") + "/*")
    return pattern in ("", ".") or path == pattern or path.startswith(pattern + "/")


def changed_paths(repo, since, to="HEAD"):
    """Changed paths since a commit, or None when the commit is unknown here."""
    if not since or not re.fullmatch(r"[0-9a-fA-F]{7,40}", since):
        return None
    if git("-C", repo, "rev-parse", "--verify", "--quiet", since + "^{commit}").returncode != 0:
        return None
    r = git("-C", repo, "diff", "--name-only", since, to)
    return r.stdout.split("\n") if r.returncode == 0 else None


def checked_map(checked):
    """'abc123' -> {'': 'abc123'}; 'repo=abc, other=def' -> {'repo': 'abc', 'other': 'def'}."""
    if not checked:
        return {}
    if "=" not in checked and "@" not in checked:
        return {"": checked}
    out = {}
    for part in re.split(r"[,\s]+", checked):
        if "=" in part or "@" in part:
            k, v = re.split(r"[=@]", part, maxsplit=1)
            out[k.strip()] = v.strip()
    return out


def topic_stale(text, repo=None, repos_by_name=None):
    h = topic_header(text)
    cm = checked_map(h["checkedAt"])
    if not h["covers"] or not cm:
        return True
    if repo:  # a repo topic
        paths = changed_paths(repo, cm.get("") or next(iter(cm.values())))
        return paths is None or any(covered(p, c) for p in paths if p for c in h["covers"])
    for c in h["covers"]:  # a workspace topic: covers are "<repo>/<path>"
        name, _, sub = c.partition("/")
        rp = (repos_by_name or {}).get(name)
        if not rp:
            continue
        paths = changed_paths(rp, cm.get(name) or cm.get(""))
        if paths is None or any(covered(p, sub) for p in paths if p):
            return True
    return False


def topics_of(d, repo=None, repos_by_name=None):
    out = []
    td = os.path.join(d, "topics")
    if os.path.isdir(td):
        for f in sorted(os.listdir(td)):
            if TOPIC_RE.match("topics/" + f):
                text = read(os.path.join(td, f)) or ""
                out.append({"name": f[:-3], "checkedAt": topic_header(text)["checkedAt"],
                            "stale": topic_stale(text, repo, repos_by_name)})
    return out


# ---- commands ----------------------------------------------------------------------------

def cmd_id(path):
    print(repo_id(path))


def head_of(repo):
    r = git("-C", repo, "rev-parse", "HEAD")
    return r.stdout.strip() if r.returncode == 0 else None


def cmd_status(folder):
    root = root_dir(required=False)
    ws, repos = in_play(folder)
    names = {os.path.basename(r): r for r in repos}
    out = {"schema": 1, "root": root, "workspace": None, "repos": []}
    if ws:
        wid = repo_id(ws)
        wdir = os.path.join(root, "workspaces", wid) if root else None
        wpath = os.path.join(wdir, "workspace.md") if wdir else None
        out["workspace"] = {"present": bool(wpath and os.path.isfile(wpath)), "path": wpath, "id": wid,
                            "topics": topics_of(wdir, None, names) if wdir else []}
    for r in repos:
        rid = repo_id(r)
        d = os.path.join(root, rid) if root else None
        idx = os.path.join(d, "index.md") if d else None
        topics = topics_of(d, r) if d else []
        out["repos"].append({"name": os.path.basename(r), "path": r, "id": rid,
                             "present": bool(idx and os.path.isfile(idx)), "head": head_of(r), "topics": topics,
                             "staleCount": sum(1 for t in topics if t["stale"]),
                             "indexBytes": os.path.getsize(idx) if idx and os.path.isfile(idx) else 0})
    print(json.dumps(out, indent=2))


def section(title, body, cap):
    body = redact(body.strip())
    if len(body.encode()) > cap:
        body = body.encode()[:cap].decode("utf-8", "ignore").rsplit("\n", 1)[0] + "\n[... cut at the size cap]"
    return f"## {title}\n\n{body}\n"


def cmd_compose(folder, out):
    root = root_dir(required=False)
    ws, repos = in_play(folder)
    names = {os.path.basename(r): r for r in repos}
    me = SCRIPT or "memory.sh"
    head = ["# Codebase memory (dcv2)", "", TRUST,
            "Contracts (workspace.md and every `contracts` topic) are binding: follow them. Other topics are descriptive notes."]
    if not root:
        text = "\n".join(head + ["", "No memory root is configured for this session (DCV2_MEMORY_DIR is not set): work from the code."]) + "\n"
        atomic_write(out, text)
        print(f"wrote {out} (no memory root)")
        return
    head += [f"Memory root: `{root}`. A repo's notes are `<root>/<repo id>/topics/<topic>.md`: read one only when the task touches the "
             f"paths it covers. Change memory only with `bash {me} write <repo id> <file> <<'EOF'` (the guard blocks Edit/Write there).", ""]
    parts, stale, any_mem = [], [], False
    if ws:
        wid = repo_id(ws)
        wdir = os.path.join(root, "workspaces", wid)
        wtext = read(os.path.join(wdir, "workspace.md"))
        if wtext is not None:
            any_mem = True
            parts.append(section(f"Workspace {os.path.basename(ws)} (binding; id workspace:{wid})", wtext, CAPS["workspace"]))
        ctext = read(os.path.join(wdir, "topics", "contracts.md"))
        if ctext is not None:
            any_mem = True
            parts.append(section("Workspace contracts (binding)", ctext, CAPS["topic"]))
        stale += [f"workspace:{wid} topics/{t['name']}.md" for t in topics_of(wdir, None, names) if t["stale"]]
    for r in repos:
        rid = repo_id(r)
        d = os.path.join(root, rid)
        itext = read(os.path.join(d, "index.md"))
        if itext is None:
            parts.append(f"## Repo {os.path.basename(r)} ({rid})\n\nNo memory yet for `{r}`: `/dcv2:onboard` creates it.\n")
            continue
        any_mem = True
        parts.append(section(f"Repo {os.path.basename(r)} (id {rid}, at {r})", itext, CAPS["index"]))
        ctext = read(os.path.join(d, "topics", "contracts.md"))
        if ctext is not None:
            parts.append(section(f"Repo {os.path.basename(r)} contracts (binding)", ctext, CAPS["topic"]))
        stale += [f"{rid} topics/{t['name']}.md (checked at {t['checkedAt'] or 'unknown'})" for t in topics_of(d, r) if t["stale"]]
    if not any_mem:
        text = "\n".join(head + [f"No codebase memory yet for `{os.path.realpath(folder)}`: `/dcv2:onboard` creates it. Until then, work from the code."]) + "\n"
        atomic_write(out, text)
        print(f"wrote {out} (no memory yet)")
        return
    if stale:
        parts.append("## Stale topics\n\nTheir covered paths changed since `checked-at`: refresh one from `git diff <checked-at>..HEAD` before relying on it.\n\n"
                     + "\n".join(f"- {s}" for s in stale) + "\n")
    text = "\n".join(head) + "\n"
    for i, p in enumerate(parts):
        if len((text + p).encode()) > COMPOSE_CAP:
            text += f"\n[{len(parts) - i} more memory sections left out: over the {COMPOSE_CAP // 1024} KB launch cap; read them from the memory root]\n"
            break
        text += "\n" + p
    atomic_write(out, text)
    print(f"wrote {out} ({len(text.encode())} bytes, {len(stale)} stale topics)")


def cmd_write(target, rel):
    root = root_dir()
    content = sys.stdin.read()
    if target.startswith("workspace:"):
        tid, kind_dir = target[len("workspace:"):], "workspaces"
        allowed = rel == "workspace.md" or TOPIC_RE.match(rel)
    else:
        tid, kind_dir = target, None
        allowed = rel == "index.md" or TOPIC_RE.match(rel)
    if not re.fullmatch(r"[a-z0-9][a-z0-9._-]*(__[a-z0-9._-]+)*", tid or "") or ".." in tid:
        die(f"REJECTED: {target!r} is not a repo id (memory.sh id <repo path>) or workspace:<id>")
    if not allowed:
        die(f"REJECTED: {rel!r} is not a memory file: use index.md (repo), workspace.md (workspace:<id>) or topics/<name>.md (lowercase, a-z 0-9 -)")
    base = os.path.join(root, kind_dir, tid) if kind_dir else os.path.join(root, tid)
    dest = os.path.join(base, rel)
    kind = "topic" if rel.startswith("topics/") else ("workspace" if rel == "workspace.md" else "index")
    size = len(content.encode())
    if not content.strip():
        die("REJECTED: empty content")
    if size > CAPS[kind]:
        die(f"REJECTED: {rel} is {size} bytes, over the {CAPS[kind]} byte cap for a {kind} file: condense it (keep only what is expensive to rediscover)")
    hits = secret_hits(content)
    if hits:
        die("REJECTED: looks like a secret on line " + ", ".join(f"{n} ({k})" for n, k in hits[:5]) + ": remove it (never store tokens, keys or .env values)")
    if kind == "topic":
        h = topic_header(content)
        if not h["covers"] or not h["checkedAt"]:
            die("REJECTED: a topic starts with a header: `# <title>`, then `covers: <path>, <path>` and `checked-at: <commit sha>` "
                "(workspace topics: covers `<repo>/<path>`, checked-at `<repo>=<sha>, ...`)")
        bad = [c for c in h["covers"] if c.startswith("/") or ".." in c.split("/")]
        if bad:
            die(f"REJECTED: covers must be paths relative to the repo (got {bad[0]!r})")
        if not all(re.fullmatch(r"[0-9a-fA-F]{7,40}", v) for v in checked_map(h["checkedAt"]).values()) or not checked_map(h["checkedAt"]):
            die("REJECTED: checked-at must be a commit sha (or `<repo>=<sha>, ...` in a workspace topic)")
    old = read(dest)
    if old is not None:
        new_lines = set(l.rstrip() for l in content.splitlines())
        for line in old.splitlines():
            if PIN in line and line.rstrip() not in new_lines:
                die(f"REJECTED: a pinned line was removed or changed (line starting {line.strip()[:50]!r}): keep every `{PIN}` line exactly as it is")
    atomic_write(dest, content if content.endswith("\n") else content + "\n")
    print(f"wrote {target} {rel} ({size} bytes)")


def cmd_touched(repo, since, to):
    repo = toplevel(repo) or die(f"{repo} is not a git repo")
    root = root_dir()
    paths = changed_paths(repo, since, to)
    if paths is None:
        die(f"{since!r} is not a commit in {repo}")
    paths = [p for p in paths if p]
    rid = repo_id(repo)
    td = os.path.join(root, rid, "topics")
    hit, seen = [], set()
    if os.path.isdir(td):
        for f in sorted(os.listdir(td)):
            if not TOPIC_RE.match("topics/" + f):
                continue
            h = topic_header(read(os.path.join(td, f)) or "")
            mine = [p for p in paths if any(covered(p, c) for c in h["covers"])]
            if mine:
                seen.update(mine)
                hit.append(f"topic topics/{f} (checked-at {h['checkedAt']}): {len(mine)} changed: " + ", ".join(mine[:8]) + (" ..." if len(mine) > 8 else ""))
    print(f"repo {rid} {repo}: {len(paths)} changed paths since {since[:12]}")
    print("\n".join(hit) if hit else "no topic touched")
    rest = [p for p in paths if p not in seen]
    if rest:
        print(f"not covered by any topic ({len(rest)}): " + ", ".join(rest[:15]) + (" ..." if len(rest) > 15 else ""))


ROOT = None
args = sys.argv[1:]
if "--root" in args:
    i = args.index("--root")
    if i + 1 >= len(args):
        die("--root needs a folder")
    ROOT = args[i + 1]
    del args[i:i + 2]
cmd, rest = (args[0] if args else ""), args[1:]
need = {"id": 1, "compose": 2, "status": 1, "write": 2, "touched": 2}
if cmd not in need or len(rest) < need[cmd]:
    die("usage: memory.sh id <repo> | compose <folder> <out> | status <folder> | write <id> <file> < content | touched <repo> <from> [<to>]")
if cmd == "id": cmd_id(rest[0])
elif cmd == "compose": cmd_compose(rest[0], rest[1])
elif cmd == "status": cmd_status(rest[0])
elif cmd == "write": cmd_write(rest[0], rest[1])
elif cmd == "touched": cmd_touched(rest[0], rest[1], rest[2] if len(rest) > 2 else "HEAD")
PY

DCV2_MEMORY_SH="${BASH_SOURCE[0]}" python3 -c "$PY" "$@"
