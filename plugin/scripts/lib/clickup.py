"""ClickUp helper of the persona roles (stdlib only; started by clickup.sh). See docs/skills.md, "ClickUp helper".

  clickup.sh lists                                   read-only: every list with its id
  clickup.sh preview <file>                          read-only: what `create` would make, and its digest
  clickup.sh create <list id> <file> --yes <digest>  creates the epic (optional) and its tickets

The tickets file (in the system temp folder or /tmp):
  {"epic": {"name": "...", "md": "..."} | null,
   "tickets": [{"name": "...", "md": "...", "tags": ["bug"]}]}
With an epic, the tickets become its subtasks. The only write call is POST /list/<id>/task. The token comes from the
macOS keychain item of the tab's Claude account (DCV2_ACCOUNT_ID) and goes only into the request header.
"""
import sys

sys.dont_write_bytecode = True  # no __pycache__ inside the plugin folder (the installed app bundle is sealed)

import hashlib
import json
import os
import re
import subprocess
import tempfile
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # this file's own folder, never the caller's environment
import dcv2_portable as portable

API = "https://api.clickup.com/api/v2"
KEYCHAIN_SERVICE = "com.jtfrancisco.dcterminalv2.clickup"
MAX_TICKETS = 50
MAX_NAME = 250
MAX_MD = 50000
MAX_TAGS = 20
MAX_FILE = 1000000
MAX_PAGES = 20
USAGE = "usage: clickup.sh lists | preview <file> | create <list id> <file> --yes <digest> (the digest is printed by preview)"


class Refused(Exception):
    """A refusal or error to show as one line on stderr (exit 1)."""


def clean(text, limit=300):
    """Printable, single-line text for output (a ticket name must not move the cursor or fake a line)."""
    text = "".join(c if c.isprintable() else " " for c in str(text))
    return text[:limit]


# ---- the test seam (AD-13): only with DCV2_TEST_MODE and a stub folder ----------------------------------------------
class Stub:
    def __init__(self, folder):
        self.folder = folder
        self.posts = 0

    def log(self, line):
        with open(os.path.join(self.folder, "requests.log"), "a", encoding="utf-8") as f:
            f.write(line + "\n")

    def platform(self):
        try:
            with open(os.path.join(self.folder, "platform"), encoding="utf-8") as f:
                return f.read().strip() or sys.platform
        except OSError:
            return sys.platform

    def token(self):
        self.log("token-read")
        with open(os.path.join(self.folder, "token"), encoding="utf-8") as f:
            return f.read().strip()

    def call(self, method, path, body):
        self.log(f"{method} {path} {json.dumps(body, sort_keys=True, ensure_ascii=False) if body is not None else ''}".rstrip())
        name = method + "_" + path.split("?")[0].strip("/").replace("/", "_") + ".json"
        canned = os.path.join(self.folder, "responses", name)
        if os.path.exists(canned):
            with open(canned, encoding="utf-8") as f:
                return json.load(f)
        if method == "POST":
            self.posts += 1
            return {"id": f"stub{self.posts}", "url": f"https://app.clickup.com/t/stub{self.posts}"}
        if re.search(r"^/list/\d+/task$", path.split("?")[0]):
            return {"tasks": [], "last_page": True}
        return {}


def test_stub():
    folder = os.environ.get("DCV2_CLICKUP_STUB", "")
    if os.environ.get("DCV2_TEST_MODE") and folder and os.path.isdir(folder):
        return Stub(folder)
    return None


# ---- refusals before any token read ---------------------------------------------------------------------------------
def check_context(stub):
    if os.environ.get("DCV2_RUN_DIR"):
        raise Refused("ClickUp tickets are not created inside an Eagle-Eye run.")
    if (stub.platform() if stub else sys.platform) != "darwin":
        raise Refused("The ClickUp helper is macOS-only for now.")
    account = os.environ.get("DCV2_ACCOUNT_ID", "")
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", account):
        raise Refused("Open this tab from DCTerminalv2 (no account id).")
    return account


def _under(path, root):
    root = root.rstrip(os.sep)
    return path.startswith(root + os.sep)


def check_location(path):
    """The real path of the tickets file; it must be inside the system temp folder or /tmp (symlinks resolved)."""
    real = os.path.realpath(path)
    roots = {os.path.realpath(tempfile.gettempdir()), os.path.realpath("/tmp")}
    if not any(_under(real, root) for root in roots):
        raise Refused("The tickets file must be in the system temp folder or /tmp (not in a repo).")
    return real


def read_bytes(real):
    try:
        with open(real, "rb") as f:
            data = f.read(MAX_FILE + 1)
    except OSError:
        raise Refused("Cannot read the tickets file.")
    if len(data) > MAX_FILE:
        raise Refused("The tickets file is too large.")
    return data


def digest_of(data):
    return hashlib.sha256(data).hexdigest()[:12]


def _text(value, what, limit, required):
    if value is None and not required:
        return ""
    if not isinstance(value, str) or (required and not value.strip()):
        raise Refused(f"The tickets file has {what} that is not text" + (" or is empty." if required else "."))
    if len(value) > limit:
        raise Refused(f"The tickets file has {what} longer than {limit} characters.")
    return value


def parse_plan(data):
    try:
        plan = json.loads(data.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        raise Refused("The tickets file is not valid JSON.")
    if not isinstance(plan, dict):
        raise Refused("The tickets file must be a JSON object with \"epic\" and \"tickets\".")
    epic = plan.get("epic")
    tickets = plan.get("tickets")
    if not isinstance(tickets, list) or not 1 <= len(tickets) <= MAX_TICKETS:
        raise Refused(f"The tickets file needs 1 to {MAX_TICKETS} tickets.")
    items = []
    if epic is not None:
        if not isinstance(epic, dict):
            raise Refused("The epic must be an object with \"name\" and \"md\", or null.")
        items.append({"name": _text(epic.get("name"), "an epic name", MAX_NAME, True), "md": _text(epic.get("md"), "epic text", MAX_MD, False)})
    names = [it["name"] for it in items]
    out = []
    for t in tickets:
        if not isinstance(t, dict):
            raise Refused("Every ticket must be an object with \"name\" and \"md\".")
        name = _text(t.get("name"), "a ticket name", MAX_NAME, True)
        tags = t.get("tags")
        if tags is not None and (
            not isinstance(tags, list) or len(tags) > MAX_TAGS
            or not all(isinstance(g, str) and 0 < len(g) <= 50 for g in tags)
        ):
            raise Refused("A ticket's tags must be a list of up to 20 short texts.")
        if name in names:
            raise Refused("Two tickets (or a ticket and the epic) have the same name: " + clean(name, 80))
        names.append(name)
        out.append({"name": name, "md": _text(t.get("md"), "ticket text", MAX_MD, False), "tags": tags or []})
    return (items[0] if items else None), out


# ---- ClickUp ------------------------------------------------------------------------------------------------------
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **kw):
        return None  # the Authorization header must never follow a redirect


class Api:
    def __init__(self, account, stub):
        self.account = account
        self.stub = stub
        self._token = None

    def token(self):
        if self._token is None:
            if self.stub:
                try:
                    tok = self.stub.token()
                except OSError:
                    tok = ""
            else:
                try:
                    r = subprocess.run(
                        ["/usr/bin/security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", self.account, "-w"],
                        capture_output=True, timeout=120,
                    )
                    tok = r.stdout.decode("utf-8", "replace").strip() if r.returncode == 0 else ""
                except (OSError, subprocess.SubprocessError):
                    tok = ""
            if not tok:
                raise Refused("No ClickUp token for this account: set it in Settings › ClickUp.")
            if not re.fullmatch(r"[\x21-\x7e]+", tok):
                raise Refused("The ClickUp token for this account looks invalid: set it again in Settings › ClickUp.")
            self._token = tok
        return self._token

    def call(self, method, path, body=None):
        token = self.token()
        if self.stub:
            return self.stub.call(method, path, body)
        req = urllib.request.Request(
            API + path, method=method,
            data=None if body is None else json.dumps(body).encode("utf-8"),
            headers={"Authorization": token, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.build_opener(NoRedirect).open(req, timeout=30) as r:
                return json.loads(r.read(20000000).decode("utf-8"))
        except urllib.error.HTTPError as e:
            if e.code == 429:
                raise Refused("ClickUp rate limit: run the same create again later (nothing is duplicated).")
            ecode = ""
            try:
                found = json.loads(e.read(65536).decode("utf-8", "replace")).get("ECODE")
                if isinstance(found, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,40}", found):
                    ecode = " " + found
            except Exception:
                pass
            raise Refused(f"ClickUp {method} {clean(path.split('?')[0], 100)}: HTTP {e.code}{ecode}")
        except urllib.error.URLError as e:
            if "CERTIFICATE" in str(e.reason).upper():
                raise Refused("ClickUp connection failed: TLS certificate not trusted (install this Python's certificates). Nothing was created by this call.")
            raise Refused("ClickUp connection failed. Run the same command again later.")
        except (OSError, ValueError):
            raise Refused("ClickUp connection failed or answered something unreadable. Run the same command again later.")


def cmd_lists(argv, stub):
    if argv:
        raise Refused(USAGE)
    api = Api(check_context(stub), stub)
    for t in api.call("GET", "/team")["teams"]:
        for s in api.call("GET", f"/team/{t['id']}/space?archived=false")["spaces"]:
            for l in api.call("GET", f"/space/{s['id']}/list?archived=false")["lists"]:
                print(f"{l['id']}\t{clean(t['name'])} / {clean(s['name'])} / {clean(l['name'])}")
            for f in api.call("GET", f"/space/{s['id']}/folder?archived=false")["folders"]:
                for l in f["lists"]:
                    print(f"{l['id']}\t{clean(t['name'])} / {clean(s['name'])} / {clean(f['name'])} / {clean(l['name'])}")


def cmd_preview(argv, stub):
    if len(argv) != 1:
        raise Refused(USAGE)
    data = read_bytes(check_location(argv[0]))
    epic, tickets = parse_plan(data)
    if epic:
        print("EPIC:", clean(epic["name"]))
    for i, t in enumerate(tickets, 1):
        tags = f"  [{', '.join(clean(g, 50) for g in t['tags'])}]" if t["tags"] else ""
        print(f"{'  ' if epic else ''}{i}. {clean(t['name'])}{tags}")
    print(f"{len(tickets)} ticket(s){' + 1 epic' if epic else ''}")
    print("digest:", digest_of(data))


def load_ledger(path, list_id):
    if not os.path.exists(path):
        return {"list": list_id, "tasks": {}}
    try:
        with open(path, encoding="utf-8") as f:
            led = json.load(f)
    except (OSError, ValueError):
        raise Refused("The ledger next to the tickets file cannot be read: delete it only if nothing was created from this file.")
    tasks = led.get("tasks") if isinstance(led, dict) else None
    if not isinstance(tasks, dict) or not all(
        isinstance(v, dict) and isinstance(v.get("id"), str) and isinstance(v.get("url"), str) for v in tasks.values()
    ):
        raise Refused("The ledger next to the tickets file is not in this helper's format: use a new tickets file.")
    if led.get("list") != list_id:
        raise Refused("This tickets file was already used for another list; use a new file for this one.")
    return led


def save_ledger(path, led):
    tmp = f"{path}.{os.getpid()}.tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(led, f, indent=1)
    portable.replace(tmp, path)


def cmd_create(argv, stub):
    if not (len(argv) == 4 and argv[2] == "--yes" and re.fullmatch(r"\d{1,20}", argv[0])):
        raise Refused(USAGE)
    list_id, path, given = argv[0], argv[1], argv[3]
    account = check_context(stub)
    real = check_location(path)
    data = read_bytes(real)  # read once: the digest, the validation and every POST use these bytes
    epic, tickets = parse_plan(data)
    if given != digest_of(data):
        raise Refused("The draft changed since the preview (or no digest): run preview again.")
    ledger_path = real + ".created.json"
    ledger = load_ledger(ledger_path, list_id)
    api = Api(account, stub)
    remote = []

    def lookup():
        """One paged read of the list's tasks (open and closed, with subtasks), before the first POST."""
        if remote:
            return
        remote.append([])
        for page in range(MAX_PAGES):
            answer = api.call("GET", f"/list/{list_id}/task?subtasks=true&include_closed=true&page={page}")
            found = answer.get("tasks") or []
            remote[0].extend(t for t in found if isinstance(t, dict))
            if answer.get("last_page") or not found:
                break

    def make(item, parent):
        name = item["name"]
        have = ledger["tasks"].get(name)
        if not have:
            lookup()
            for t in remote[0]:
                if t.get("name") == name and (t.get("parent") or None) == parent and isinstance(t.get("id"), str):
                    have = {"id": t["id"], "url": str(t.get("url") or "")}
                    ledger["tasks"][name] = have
                    save_ledger(ledger_path, ledger)
                    break
        if have:
            print("skip (already created)", clean(have["url"]), clean(name))
            return have
        body = {"name": name, "markdown_content": item["md"], "markdown_description": item["md"]}
        if item.get("tags"):
            body["tags"] = item["tags"]
        if parent:
            body["parent"] = parent
        made = api.call("POST", f"/list/{list_id}/task", body)
        if not (isinstance(made, dict) and isinstance(made.get("id"), str) and isinstance(made.get("url"), str)):
            raise Refused("ClickUp answered something unexpected to the create call; check the list before running it again.")
        ledger["tasks"][name] = {"id": made["id"], "url": made["url"]}
        save_ledger(ledger_path, ledger)
        print("created", clean(made["url"]), clean(name))
        return ledger["tasks"][name]

    parent = make(epic, None)["id"] if epic else None
    for t in tickets:
        make(t, parent)


def main(argv):
    stub = test_stub()
    commands = {"lists": cmd_lists, "preview": cmd_preview, "create": cmd_create}
    if not argv or argv[0] not in commands:
        raise Refused(USAGE)
    commands[argv[0]](argv[1:], stub)
    sys.stdout.flush()


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except Refused as e:
        print(str(e), file=sys.stderr)
        sys.exit(1)
    except SystemExit:
        raise
    except BaseException:  # never a traceback: it could carry request data
        print("The ClickUp helper failed unexpectedly (nothing more was done). Run preview again, then retry.", file=sys.stderr)
        sys.exit(1)
