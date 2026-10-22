#!/usr/bin/env bash
# Run-folder writes for the eagle-eye orchestrator.
#
#   state.sh put <file.json>   < json     validate the JSON on stdin, then write it
#                                          atomically (<file>.tmp + mv). Invalid JSON:
#                                          exit 1, the file is left as it was.
#   state.sh log <run dir> <stage> <text>  append "<UTC time> <stage> <text>" to log.md
#   state.sh now                           print the UTC time (ISO-8601)
#   state.sh worktree <repo path> <run id> <pr id> [<base commit>]
#                                          make (or reuse / re-make) the PR's worktree beside
#                                          the repo and print "branch=<b> worktree=<path>".
#                                          Never touches the repo's main checkout.
#
#   state.sh writeback <run dir>           write writeback.json (atomic) from plan.md's
#                                          "## Tickets" section and status.json; only when
#                                          tickets.json exists. Needs python3.
#
# For status.json, `put` also sets "updatedAt" to the current UTC time.
set -euo pipefail

validate() { # <file>: JSON parses (python3, else node, else macOS JavaScript)
  local f="$1"
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$f" <<'PY'
import json, sys
try:
    doc = json.load(open(sys.argv[1]))
except json.JSONDecodeError as e:
    sys.exit(f"JSON syntax error: {e}")
if isinstance(doc, dict) and "stage" in doc:
    stages = {"planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"}
    states = {"pending", "building", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"}
    problems = []
    if doc.get("schema") != 1: problems.append("schema must be 1")
    if doc.get("stage") not in stages: problems.append(f"stage {doc.get('stage')!r} not one of {sorted(stages)}")
    if doc.get("waitingFor") not in (None, "plan-approval", "ship-approval"): problems.append("bad waitingFor")
    if not isinstance(doc.get("prs"), list): problems.append("prs must be a list")
    for pr in doc.get("prs") or []:
        if pr.get("state") not in states: problems.append(f"{pr.get('id')}: state {pr.get('state')!r} not one of {sorted(states)}")
    if problems:
        sys.exit("status.json: " + "; ".join(problems))
PY
  elif command -v node >/dev/null 2>&1; then
    node -e 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))' "$f"
  elif command -v osascript >/dev/null 2>&1; then
    osascript -l JavaScript -e 'ObjC.import("Foundation"); function run(a) { JSON.parse($.NSString.stringWithContentsOfFileEncodingError(a[0], 4, null).js); }' "$f" >/dev/null
  fi
}

case "${1:-}" in
  put)
    dest="${2:?usage: state.sh put <file.json>}"
    tmp="$dest.tmp"
    cat > "$tmp"
    if [[ "$(basename "$dest")" == status.json ]] && command -v python3 >/dev/null 2>&1; then
      python3 - "$tmp" "$(date -u +%FT%TZ)" <<'PY' 2>/dev/null || true
import json, sys
path, now = sys.argv[1], sys.argv[2]
doc = json.load(open(path))
if isinstance(doc, dict):
    doc["updatedAt"] = now
    open(path, "w").write(json.dumps(doc, indent=2) + "\n")
PY
    fi
    if ! err="$(validate "$tmp" 2>&1)"; then
      rm -f "$tmp"
      echo "NOT WRITTEN: invalid JSON for $dest: $err" >&2
      exit 1
    fi
    if [[ "$(basename "$dest")" == status.json ]] && command -v python3 >/dev/null 2>&1; then
      # test-mode: halt-after=<PR> (tests only; see docs/run-folder.md). Ignored once
      # the run was resumed. Keeps the halted state exact even if the model forgets.
      if ! halt="$(python3 - "$tmp" "$(dirname "$dest")" <<'PY'
import json, os, re, sys
doc, run = json.load(open(sys.argv[1])), sys.argv[2]
def read(name):
    try:
        return open(os.path.join(run, name)).read()
    except OSError:
        return ""
m = re.search(r"^test-mode:\s*halt-after=(\S+)\s*$", read("request.md"), re.M)
if not m or "resumed at stage" in read("log.md"):
    sys.exit(0)
pr_id = m.group(1)
prs = {p.get("id"): p for p in doc.get("prs") or []}
if prs.get(pr_id, {}).get("state") not in ("reviewed",):
    sys.exit(0)
others = [i for i, p in prs.items() if i != pr_id and p.get("state") != "pending"]
if others or doc.get("stage") != "building":
    print(f"NOT WRITTEN: test-mode halt-after={pr_id}: {pr_id} is reviewed, so the run must stop now with stage 'building' and every other PR 'pending'. Write that, log 'halted after {pr_id} (test-mode)', reply 'HALTED after {pr_id}' and stop.")
    sys.exit(1)
print(f"HALT (test-mode): {pr_id} is reviewed. Log 'halted after {pr_id} (test-mode)', reply 'HALTED after {pr_id}' and stop now. Launch nothing else.")
PY
)"; then
        rm -f "$tmp"
        echo "$halt" >&2
        exit 1
      fi
    fi
    if [[ "$(basename "$dest")" == status.json ]] && command -v python3 >/dev/null 2>&1; then
      if ! gate="$(python3 - "$tmp" "$(dirname "$dest")" <<'PY'
import json, os, re, sys
doc, run = json.load(open(sys.argv[1])), sys.argv[2]
req = open(os.path.join(run, "request.md")).read() if os.path.isfile(os.path.join(run, "request.md")) else ""
if doc.get("stage") == "done" and os.path.isfile(os.path.join(run, "tickets.json")) and not os.path.isfile(os.path.join(run, "writeback.json")):
    print("NOT WRITTEN: tickets.json exists but writeback.json does not. Run `state.sh writeback <run>` first, then write stage done.")
    sys.exit(1)
if doc.get("waitingFor") and re.search(r"^test-mode:\s*auto-approve\s*$", req, re.M):
    print(f"AUTO-APPROVE (test-mode): do not wait for the user. Log the auto-approval and continue past {doc['waitingFor']} now.")
PY
)"; then
        rm -f "$tmp"; echo "$gate" >&2; exit 1
      fi
    fi
    mv -f "$tmp" "$dest"
    echo "wrote $dest"
    [[ -z "${gate:-}" ]] || echo "$gate"
    [[ -z "${halt:-}" ]] || echo "$halt"
    ;;
  log)
    run="${2:?usage: state.sh log <run dir> <stage> <text>}"; stage="${3:?stage}"; shift 3
    text="$*"; text="${text//$'\n'/ }"
    printf '%s %s %s\n' "$(date -u +%FT%TZ)" "$stage" "$text" >> "$run/log.md"
    ;;
  now)
    date -u +%FT%TZ
    ;;
  writeback)
    run="${2:?usage: state.sh writeback <run dir>}"
    [[ -f "$run/tickets.json" ]] || { echo "no tickets.json: no writeback needed"; exit 0; }
    if ! python3 - "$run" "$(date -u +%FT%TZ)" > "$run/writeback.json.tmp" <<'PY'
import json, os, re, sys
run, now = sys.argv[1], sys.argv[2]
plan = open(os.path.join(run, "plan.md")).read()
st = json.load(open(os.path.join(run, "status.json")))
titles = dict(re.findall(r"^###\s+(S\d+)\s*[:\-\u2014]+\s*(.+?)\s*$", plan, re.M))
section = re.search(r"^## Tickets\s*$(.*?)(?=^## |\Z)", plan, re.M | re.S)
if not section:
    sys.exit("plan.md has no '## Tickets' section")
story_pr = {}
for pr in st.get("prs") or []:
    for sid in (pr.get("stories") or {}).get("ids") or []:
        story_pr[sid] = pr
drafts = []
for line in section.group(1).splitlines():
    line = line.strip()
    if not line.startswith("- ") or "|" not in line:
        continue
    cells = [c.strip() for c in line[2:].split("|")]
    if len(cells) < 4:
        continue
    tid, cid, title, url = cells[:4]
    stories = []
    status = None
    for c in cells[4:]:
        if c.lower().startswith("stories:"):
            stories = [s.strip() for s in c.split(":", 1)[1].split(",") if s.strip()]
        elif c.lower().startswith("ship status:"):
            v = c.split(":", 1)[1].strip()
            status = None if v in ("", "-") else v
    prs, lines = [], []
    for sid in stories:
        pr = story_pr.get(sid)
        if pr and not any(p["id"] == pr["id"] for p in prs):
            prs.append({"id": pr["id"], "repo": pr.get("repo"), "url": pr.get("prUrl")})
    for p in prs:
        pr = next(x for x in st["prs"] if x["id"] == p["id"])
        where = pr.get("prUrl") or f"local branch `{pr.get('branch')}` (not pushed: {pr.get('note') or 'no PR opened'})"
        chk = pr.get("check") or {}
        rev = pr.get("review") or {}
        lines.append(f"- {pr['id']} ({pr.get('repo')}): {where}. Check `{chk.get('command')}`: {chk.get('result')}. Review: {rev.get('verdict')} (round {rev.get('round')}).")
    done = [f"- {s}: {titles.get(s, '')}".rstrip(": ") for s in stories]
    comment = "Eagle-Eye run " + st.get("runId", "") + "\n\nDone:\n" + "\n".join(done or ["- (no stories mapped)"]) + "\n\nPull requests:\n" + "\n".join(lines or ["- none"])
    opened = prs and all(p["url"] for p in prs)
    drafts.append({
        "ticket": {"id": tid, "customId": None if cid in ("", "-") else cid, "title": title, "url": url},
        "prs": prs,
        "comment": comment,
        "status": status if opened else None,
        "statusNote": None if opened else ("Status left unchanged: not every PR was opened." if prs else "No PR covers this ticket."),
    })
print(json.dumps({"schema": 1, "runId": st.get("runId"), "createdAt": now, "drafts": drafts}, indent=2))
PY
    then
      rm -f "$run/writeback.json.tmp"; echo "NOT WRITTEN: writeback.json" >&2; exit 1
    fi
    mv -f "$run/writeback.json.tmp" "$run/writeback.json"
    echo "wrote $run/writeback.json"
    ;;
  worktree)
    repo="${2:?usage: state.sh worktree <repo path> <run id> <pr id> [<base commit>]}"
    run_id="${3:?run id}"; pr="${4:?pr id}"; base="${5:-}"
    repo="$(cd "$repo" && pwd -P)"
    pr_lc="$(printf '%s' "$pr" | tr '[:upper:]' '[:lower:]')"
    branch="dcv2/$run_id-$pr_lc"
    wt="$(dirname "$repo")/$(basename "$repo")-worktrees/dcv2-$run_id-$pr_lc"
    if git -C "$repo" worktree list --porcelain | grep -qxF "worktree $wt" && [[ -d "$wt" ]]; then
      : # already there: reuse
    else
      git -C "$repo" worktree prune
      mkdir -p "$(dirname "$wt")"
      if git -C "$repo" show-ref --verify --quiet "refs/heads/$branch"; then
        git -C "$repo" worktree add -q "$wt" "$branch" >&2
      else
        git -C "$repo" worktree add -q -b "$branch" "$wt" ${base:+"$base"} >&2
      fi
    fi
    echo "branch=$branch worktree=$wt"
    ;;
  *)
    echo "usage: state.sh put <file.json> | log <run> <stage> <text> | now | worktree <repo> <run id> <pr id> [<base>]" >&2
    exit 2
    ;;
esac
