#!/usr/bin/env bash
# Unit test of plugin/scripts/clickup.sh (the ClickUp helper): no Claude call, no network, no keychain item.
# Every helper call goes through run_helper: a fake account id no keychain holds, a stub folder (DCV2_TEST_MODE +
# DCV2_CLICKUP_STUB) for the token and ClickUp's answers, and none of the caller's DCV2_* values (inside an
# Eagle-Eye run DCV2_RUN_DIR and a real DCV2_ACCOUNT_ID are exported).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CU="$ROOT/plugin/scripts/clickup.sh"
unset DCV2_RUN_DIR DCV2_ACCOUNT_ID DCV2_TEST_MODE DCV2_CLICKUP_STUB || true

case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) echo "skip: the ClickUp helper is macOS-only for now"; echo PASS; exit 0 ;; esac
command -v python3 >/dev/null 2>&1 || { echo "skip: no python3"; echo PASS; exit 0; }

d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
slash="/tmp/dcv2-clickup-test.$$.$RANDOM"
trap 'rm -rf "$d" "$slash"' EXIT

FAKE_TOKEN="dcv2-fake-token-7f3a91c2"
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
out=""; err=""; rc=0

# The one way to call the helper: env -u for the caller's values, then the fake account id, then the case's variables.
run_helper() {
  rc=0
  env -u DCV2_RUN_DIR -u DCV2_ACCOUNT_ID -u DCV2_TEST_MODE -u DCV2_CLICKUP_STUB \
    DCV2_ACCOUNT_ID=dcv2-test-none "$@" >"$d/out" 2>"$d/err" || rc=$?
  out="$(cat "$d/out")"; err="$(cat "$d/err")"
  # Keep every output to check for the token at the end.
  cat "$d/out" "$d/err" >> "$d/all-output"
}
helper() { run_helper bash "$CU" "$@"; }
# A call with the stub folder $1 and test mode on, then the helper's arguments.
stubbed() { local s="$1"; shift; run_helper DCV2_TEST_MODE=1 DCV2_CLICKUP_STUB="$s" bash "$CU" "$@"; }

want_rc()  { if [[ "$rc" == "$1" ]]; then ok "$2 -> $rc"; else bad "$2 -> $rc (want $1): ${out:0:200} ${err:0:200}"; fi; }
refused()  { if [[ "$rc" != 0 ]]; then ok "$1 -> refused ($rc)"; else bad "$1 -> exit 0"; fi; }
has_out()  { if grep -qF -- "$2" <<<"$out"; then ok "$1"; else bad "$1: no '$2' in stdout: ${out:0:300}"; fi; }
has_err()  { if grep -qF -- "$2" <<<"$err"; then ok "$1"; else bad "$1: no '$2' in stderr: ${err:0:300}"; fi; }
count_in() { grep -c -- "$2" "$1" 2>/dev/null || true; }

new_stub() { # a stub folder with the fake token and platform darwin
  local s="$d/stub.$1"; mkdir -p "$s/responses"
  printf '%s\n' "$FAKE_TOKEN" > "$s/token"; echo darwin > "$s/platform"; : > "$s/requests.log"
  echo "$s"
}
posts() { count_in "$1/requests.log" '^POST '; }
reads() { count_in "$1/requests.log" '^token-read'; }

mkdir -p "$d/drafts" "$slash"
draft="$d/drafts/tickets.json"
cat > "$draft" <<'JSON'
{"epic": {"name": "Epic: Search", "md": "Search epic"},
 "tickets": [{"name": "Add search box", "md": "Body 1", "tags": ["ui"]},
             {"name": "Index titles", "md": "Body 2"}]}
JSON

echo "# preview and the digest"
helper preview "$draft"
want_rc 0 "preview"
has_out "preview prints the epic" "EPIC: Epic: Search"
has_out "preview numbers the tickets with tags" "1. Add search box  [ui]"
has_out "preview counts" "2 ticket(s) + 1 epic"
digest="$(sed -n 's/^digest: //p' <<<"$out")"
want="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest()[:12])' "$draft")"
[[ "$digest" == "$want" && ${#digest} == 12 ]] && ok "digest is the first 12 hex of the SHA-256 of the bytes" || bad "digest '$digest' != '$want'"

echo "# create needs the digest"
S="$(new_stub refuse)"
stubbed "$S" create 555 "$draft";                         refused "create without --yes"
stubbed "$S" create 555 "$draft" --yes;                   refused "create with --yes and no digest"
stubbed "$S" create 555 "$draft" --yes 000000000000;      refused "create with a wrong digest"
has_err "wrong digest names the preview" "run preview again"
stubbed "$S" create abc "$draft" --yes "$digest";         refused "create with a list id that is not digits"
cp "$draft" "$d/drafts/edited.json"
helper preview "$d/drafts/edited.json"; ed="$(sed -n 's/^digest: //p' <<<"$out")"
sed -i.bak 's/Body 2/Body 2 edited/' "$d/drafts/edited.json" && rm -f "$d/drafts/edited.json.bak"
stubbed "$S" create 555 "$d/drafts/edited.json" --yes "$ed"; refused "create after the file was edited post-preview"
[[ "$(posts "$S")" == 0 && "$(reads "$S")" == 0 ]] && ok "no POST and no token read for the refusals" || bad "a refusal reached the token or ClickUp: $(cat "$S/requests.log")"

echo "# happy create, re-run, lookup"
S="$(new_stub happy)"
stubbed "$S" create 555 "$draft" --yes "$digest"
want_rc 0 "create epic + 2 tickets"
[[ "$(posts "$S")" == 3 ]] && ok "3 POSTs" || bad "POST count $(posts "$S"): $(cat "$S/requests.log")"
[[ "$(grep '"name": "Epic: Search"' "$S/requests.log" | grep -c '"parent"' || true)" == 0 ]] && ok "the epic has no parent" || bad "epic POST: $(cat "$S/requests.log")"
[[ "$(grep -c '"parent": "stub1"' "$S/requests.log")" == 2 ]] && ok "both tickets carry the epic as parent" || bad "parents: $(cat "$S/requests.log")"
grep -q '"tags": \["ui"\]' "$S/requests.log" && ok "tags are sent" || bad "no tags in the log"
[[ "$(sed -n '2p' "$S/requests.log")" == "GET /list/555/task?subtasks=true&include_closed=true&page=0" ]] && ok "one lookup page, before the first POST" || bad "order: $(cat "$S/requests.log")"
[[ "$(grep -c '^created ' <<<"$out")" == 3 ]] && ok "3 created lines" || bad "output: $out"
python3 - "$draft.created.json" <<'PY' && ok "ledger holds the list and 3 tasks" || bad "ledger content"
import json, sys
led = json.load(open(sys.argv[1]))
assert led["list"] == "555" and len(led["tasks"]) == 3 and all("id" in v and "url" in v for v in led["tasks"].values())
PY
before="$(posts "$S")"
stubbed "$S" create 555 "$draft" --yes "$digest"
want_rc 0 "re-run"
[[ "$(grep -c '^skip (already created) ' <<<"$out")" == 3 && "$(posts "$S")" == "$before" ]] && ok "re-run: 3 skips, no new POST" || bad "re-run: $out / $(cat "$S/requests.log")"
stubbed "$S" create 556 "$draft" --yes "$digest"
refused "the ledger records another list id"

echo "# a task of the same name and parent already in the list"
S="$(new_stub lookup)"
cat > "$d/drafts/one.json" <<'JSON'
{"epic": null, "tickets": [{"name": "Already there", "md": "x"}, {"name": "New one", "md": "y"}]}
JSON
helper preview "$d/drafts/one.json"; od="$(sed -n 's/^digest: //p' <<<"$out")"
cat > "$S/responses/GET_list_777_task.json" <<'JSON'
{"tasks": [{"id": "aa1", "name": "Already there", "url": "https://app.clickup.com/t/aa1", "parent": null},
           {"id": "aa2", "name": "New one", "url": "https://app.clickup.com/t/aa2", "parent": "someone"}], "last_page": true}
JSON
stubbed "$S" create 777 "$d/drafts/one.json" --yes "$od"
want_rc 0 "create with a ticket already in the list"
has_out "same name and parent is skipped" "skip (already created) https://app.clickup.com/t/aa1 Already there"
[[ "$(posts "$S")" == 1 ]] && grep -q '"name": "New one"' "$S/requests.log" && ok "only the ticket with another parent is posted" || bad "posts: $(cat "$S/requests.log")"

echo "# refusals before any token read"
S="$(new_stub run)"
run_helper DCV2_RUN_DIR=/nonexistent DCV2_TEST_MODE=1 DCV2_CLICKUP_STUB="$S" bash "$CU" create 555 "$draft" --yes "$digest"
refused "create inside a run"; has_err "run refusal text" "not created inside an Eagle-Eye run"
run_helper DCV2_RUN_DIR=/nonexistent DCV2_TEST_MODE=1 DCV2_CLICKUP_STUB="$S" bash "$CU" lists
refused "lists inside a run"
[[ "$(reads "$S")" == 0 && "$(wc -l < "$S/requests.log" | tr -d ' ')" == 0 ]] && ok "no token-read and no request from a run" || bad "run reached the token: $(cat "$S/requests.log")"
run_helper DCV2_ACCOUNT_ID= DCV2_TEST_MODE=1 DCV2_CLICKUP_STUB="$S" bash "$CU" create 555 "$draft" --yes "$digest"
refused "no account id"; has_err "account text" "no account id"
run_helper DCV2_ACCOUNT_ID='bad id/..' DCV2_TEST_MODE=1 DCV2_CLICKUP_STUB="$S" bash "$CU" create 555 "$draft" --yes "$digest"
refused "account id with bad characters"
echo linux > "$S/platform"
stubbed "$S" create 555 "$draft" --yes "$digest"
refused "stub platform linux"; has_err "platform text" "macOS-only"
[[ "$(reads "$S")" == 0 ]] && ok "no token read for the context refusals" || bad "token read: $(cat "$S/requests.log")"

echo "# where the draft may live"
cat > "$slash/t.json" <<'JSON'
{"epic": null, "tickets": [{"name": "A", "md": "a"}]}
JSON
helper preview "$slash/t.json";  want_rc 0 "a draft under /tmp"
helper preview "$draft";         want_rc 0 "a draft under the system temp folder"
helper preview "/usr/share/dcv2-clickup-test.json"; refused "a draft outside both"
has_err "location text" "system temp folder or /tmp"
ln -s /etc/hosts "$d/drafts/link.json"
helper preview "$d/drafts/link.json"; refused "a symlink pointing outside"

echo "# validation"
bad_draft() { printf '%s' "$2" > "$d/drafts/v-$1.json"; helper preview "$d/drafts/v-$1.json"; refused "$1"; }
bad_draft "duplicate names" '{"epic": null, "tickets": [{"name": "A", "md": ""}, {"name": "A", "md": ""}]}'
bad_draft "ticket named like the epic" '{"epic": {"name": "E", "md": ""}, "tickets": [{"name": "E", "md": ""}]}'
bad_draft "no tickets" '{"epic": null, "tickets": []}'
bad_draft "empty name" '{"epic": null, "tickets": [{"name": " ", "md": ""}]}'
bad_draft "not json" 'nope'
bad_draft "over-long name" "{\"epic\": null, \"tickets\": [{\"name\": \"$(printf 'x%.0s' $(seq 1 300))\", \"md\": \"\"}]}"
python3 - "$d/drafts/v-many.json" <<'PY'
import json, sys
json.dump({"epic": None, "tickets": [{"name": f"t{i}", "md": ""} for i in range(51)]}, open(sys.argv[1], "w"))
PY
helper preview "$d/drafts/v-many.json"; refused "more than 50 tickets"

echo "# the test seam is off without DCV2_TEST_MODE"
S="$(new_stub seam)"
run_helper DCV2_CLICKUP_STUB="$S" bash "$CU" lists
refused "lists with a stub folder but no test mode"; has_err "falls through to the real token lookup" "No ClickUp token"
[[ "$(wc -l < "$S/requests.log" | tr -d ' ')" == 0 ]] && ok "the stub was not used" || bad "stub used without test mode"

echo "# lists"
S="$(new_stub lists)"
echo '{"teams": [{"id": "9", "name": "Team"}]}' > "$S/responses/GET_team.json"
echo '{"spaces": [{"id": "8", "name": "Space"}]}' > "$S/responses/GET_team_9_space.json"
echo '{"lists": [{"id": "111", "name": "Loose"}]}' > "$S/responses/GET_space_8_list.json"
echo '{"folders": [{"id": "7", "name": "Folder", "lists": [{"id": "222", "name": "Inside"}]}]}' > "$S/responses/GET_space_8_folder.json"
stubbed "$S" lists
want_rc 0 "lists"
[[ "$out" == "$(printf '111\tTeam / Space / Loose\n222\tTeam / Space / Folder / Inside')" ]] && ok "one line per list with its path" || bad "lists output: $out"
[[ "$(posts "$S")" == 0 ]] && ok "lists makes no POST" || bad "POST from lists"

echo "# the caller's environment does not matter (as inside an Eagle-Eye run)"
S="$(new_stub env)"; cp "$draft" "$d/drafts/env.json"
( export DCV2_RUN_DIR=/nonexistent DCV2_ACCOUNT_ID=default DCV2_TEST_MODE=0 DCV2_CLICKUP_STUB=/nonexistent
  stubbed "$S" create 555 "$d/drafts/env.json" --yes "$digest"
  [[ "$rc" == 0 && "$(posts "$S")" == 3 ]] ) && ok "happy path with DCV2_RUN_DIR and DCV2_ACCOUNT_ID=default exported" || bad "happy path depends on the caller's env"

echo "# the token is never written"
leaks=0
grep -qF -- "$FAKE_TOKEN" "$d/all-output" && leaks=1
for f in "$d"/stub.*/requests.log "$d"/drafts/*.created.json; do
  grep -qF -- "$FAKE_TOKEN" "$f" && { leaks=1; echo "token in $f"; }
done
[[ "$leaks" == 0 ]] && ok "the fake token is in no output, ledger or log" || bad "the fake token leaked"

echo "# source rules"
! grep -E '"(PUT|DELETE|PATCH)"' "$ROOT/plugin/scripts/lib/clickup.py" >/dev/null && ok "POST is the only write method" || bad "clickup.py write methods"
! grep -rqF "CLICKUP_TOKEN_FILE" "$ROOT/plugin" && ok "no CLICKUP_TOKEN_FILE under plugin/" || bad "CLICKUP_TOKEN_FILE referenced"
[[ -z "$(find "$ROOT/plugin" -name __pycache__ | head -1)" ]] && ok "no __pycache__ under plugin/" || bad "__pycache__ under plugin/"

[[ "$fails" == 0 ]] && echo PASS || { echo "SOME FAILED"; exit 1; }
