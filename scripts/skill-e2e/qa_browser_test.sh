#!/usr/bin/env bash
# Unit test of plugin/scripts/qa-browser.mjs against a stub CDP endpoint (no real browser, no network, no Claude).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
QB="$ROOT/plugin/scripts/qa-browser.mjs"
STUB="$ROOT/scripts/skill-e2e/qa_stub_browser.mjs"
d="$(mktemp -d)"; d="$(cd "$d" && pwd -P)"
trap 'rm -rf "$d"; [[ -n "${stubpid:-}" ]] && kill -9 "$stubpid" 2>/dev/null || true' EXIT
unset DCV2_RUN_DIR DCV2_QA_BROWSER DCV2_TEST_MODE || true

fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
expect() {
  local want="$1" label="$2" got=0; shift 2
  out="$("$@" 2>&1)" || got=$?
  if [[ $got == "$want" ]]; then ok "$label -> $got"; else bad "$label -> $got (want $want): ${out:0:300}"; fi
}
has() { if grep -qF -- "$2" <<<"$out"; then ok "$1"; else bad "$1: no '$2' in: ${out:0:300}"; fi; }

if ! command -v node >/dev/null 2>&1 || ! node -e 'process.exit(typeof WebSocket === "function" ? 0 : 1)' 2>/dev/null; then
  echo "skip: node 22+ with a global WebSocket is not available"; echo PASS; exit 0
fi

case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) echo "skip: qa-browser.mjs drives a macOS browser; its process-group tests are POSIX only"; echo PASS; exit 0 ;; esac

RUN="$d/run"; mkdir -p "$RUN/reviews"
export DCV2_RUN_DIR="$RUN"
OUT="$RUN/reviews/qa"
export DCV2_QA_BROWSER="$STUB" DCV2_TEST_MODE=auto-approve STUB_PID_FILE="$d/stub.pid"
chmod +x "$STUB"

echo "# steps against the stub"
steps="$d/steps.jsonl"
cat > "$steps" <<'EOF'
{"op":"open","url":"http://127.0.0.1:1530/"}
{"op":"wait-text","text":"Start","timeoutMs":1000}
{"op":"screenshot","name":"S1.1"}
EOF
expect 0 "open, wait-text, screenshot" node "$QB" --out "$OUT" --steps "$steps"
[[ "$(grep -c '^{' <<<"$out")" == 3 ]] && ok "  three JSON result lines" || bad "  result lines: $out"
has "  open ok" '"op":"open","ok":true'
[[ -s "$OUT/S1.1.png" ]] && ok "  one PNG under the out folder" || bad "  no PNG"
[[ "$(ls "$OUT" | wc -l | tr -d ' ')" == 1 ]] && ok "  nothing else written" || bad "  out folder: $(ls "$OUT")"
sleep 0.3
stubpid="$(cat "$d/stub.pid")"
if kill -0 "$stubpid" 2>/dev/null; then bad "  the stub browser is still running after exit"; else ok "  the stub browser is gone after exit"; fi

printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' '{"op":"wait-text","text":"Never shown","timeoutMs":300}' '{"op":"text"}' > "$steps"
expect 1 "a failing step exits 1 and the others still run" node "$QB" --out "$OUT" --steps "$steps"
has "  the failure names the text" 'text not seen'
has "  later steps still answer" '"op":"text","ok":true'

echo "# refusals"
printf '%s\n' '{"op":"open","url":"http://example.com/"}' > "$steps"
expect 2 "open http://example.com/ is refused" node "$QB" --out "$OUT" --steps "$steps"
printf '%s\n' '{"op":"open","url":"http://localhost.evil.com/"}' > "$steps"
expect 2 "open http://localhost.evil.com/ is refused" node "$QB" --out "$OUT" --steps "$steps"
printf '%s\n' '{"op":"open","url":"https://127.0.0.1/"}' > "$steps"
expect 2 "open https://127.0.0.1/ is refused" node "$QB" --out "$OUT" --steps "$steps"
printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1/"}' > "$steps"
expect 2 "an out folder outside the run is refused" node "$QB" --out "$d/elsewhere" --steps "$steps"
expect 2 "an out folder that is the run folder's reviews/ itself is refused" node "$QB" --out "$RUN/reviews" --steps "$steps"
printf '%s\n' '{"op":"screenshot","name":"../x"}' > "$steps"
expect 2 "a screenshot name with a path is refused" node "$QB" --out "$OUT" --steps "$steps"
printf '%s\n' '{"op":"explode"}' > "$steps"
expect 2 "an unknown op is refused" node "$QB" --out "$OUT" --steps "$steps"

echo "# the 5 MB cap"
printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' '{"op":"screenshot","name":"big"}' > "$steps"
expect 1 "a screenshot over 5 MB fails" env STUB_BIG=1 node "$QB" --out "$OUT" --steps "$steps"
has "  says why" "over 5 MB"
[[ ! -e "$OUT/big.png" ]] && ok "  no big PNG written" || bad "  big.png was written"

echo "# scroll, select, press with modifiers, full-page shots, page errors"
log="$d/stub.log"; : > "$log"
printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' \
  '{"op":"scroll","selector":".log"}' '{"op":"scroll","text":"Receipt"}' '{"op":"scroll","by":600}' '{"op":"scroll","by":{"x":0,"y":-40}}' \
  '{"op":"select","selector":"select#theme","label":"Light"}' '{"op":"select","selector":"select#theme","value":"light"}' \
  '{"op":"press","key":"Meta+K"}' '{"op":"press","key":"PageDown"}' '{"op":"press","key":"K","modifiers":["shift"]}' '{"op":"press","key":"Escape"}' \
  '{"op":"screenshot","name":"full1","fullPage":true}' '{"op":"screenshot","name":"view1"}' > "$steps"
expect 0 "scroll, select, press and full-page screenshot all succeed" env STUB_LOG="$log" node "$QB" --out "$OUT" --steps "$steps"
[[ "$(grep -c '"ok":true' <<<"$out")" == 13 ]] && ok "  thirteen ok steps" || bad "  step results: $out"
node -e '
const rows = require("fs").readFileSync(process.argv[1], "utf8").trim().split("\n").map((l) => JSON.parse(l));
const keys = rows.filter((r) => r.method === "Input.dispatchKeyEvent").map((r) => r.params);
const fail = (m) => { console.error("  " + m); process.exit(1); };
const metaK = keys.filter((k) => k.key === "k" && k.modifiers === 4);
if (metaK.length !== 2 || metaK[0].type !== "rawKeyDown" || metaK[1].type !== "keyUp") fail("Meta+K events: " + JSON.stringify(metaK));
if (metaK[0].code !== "KeyK" || metaK[0].windowsVirtualKeyCode !== 75 || metaK[0].nativeVirtualKeyCode !== 75 || "text" in metaK[0]) fail("Meta+K fields: " + JSON.stringify(metaK[0]));
const pd = keys.filter((k) => k.key === "PageDown");
if (pd.length !== 2 || pd[0].code !== "PageDown" || pd[0].windowsVirtualKeyCode !== 34 || pd[0].modifiers !== 0) fail("PageDown: " + JSON.stringify(pd));
const shiftK = keys.find((k) => k.key === "K" && k.type === "keyDown");
if (!shiftK || shiftK.modifiers !== 8 || shiftK.text !== "K" || shiftK.windowsVirtualKeyCode !== 75) fail("Shift+K: " + JSON.stringify(shiftK));
const esc = keys.find((k) => k.key === "Escape");
if (!esc || esc.windowsVirtualKeyCode !== 27 || esc.code !== "Escape") fail("Escape: " + JSON.stringify(esc));
const shots = rows.filter((r) => r.method === "Page.captureScreenshot").map((r) => r.params);
const full = shots[0], view = shots[1];
if (!full.captureBeyondViewport || full.clip.height !== 2400 || full.clip.width !== 1280) fail("fullPage params: " + JSON.stringify(full));
if (view.captureBeyondViewport || view.clip) fail("a plain screenshot must not use the clip: " + JSON.stringify(view));
if (!rows.some((r) => r.method === "scrollBy" && r.params.expression.includes("scrollBy(0, 600)"))) fail("no scrollBy(0, 600)");
if (!rows.some((r) => r.method === "scrollBy" && r.params.expression.includes("scrollBy(0, -40)"))) fail("no scrollBy(0, -40)");
if (rows.filter((r) => r.method === "scrollIntoView").length !== 2) fail("scrollIntoView for selector and text");
const sel = rows.filter((r) => r.method === "select");
if (sel.length !== 2 || !sel.every((r) => r.params.expression.includes("\"input\"") && r.params.expression.includes("\"change\""))) fail("select must fire input and change");
' "$log" && ok "  CDP: Meta+K and PageDown carry code, key codes and the modifier bitmask; fullPage uses captureBeyondViewport; scroll and select ran" || bad "  CDP calls (see above)"
[[ -s "$OUT/full1.png" && -s "$OUT/view1.png" ]] && ok "  both screenshots written" || bad "  screenshots missing"

printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' '{"op":"select","selector":"select#theme","label":"Missing Option"}' '{"op":"scroll","selector":".nope"}' > "$steps"
expect 1 "a missing option and a missing scroll target fail their steps" env STUB_NO_SCROLL=1 node "$QB" --out "$OUT" --steps "$steps"
has "  the missing option is named" 'no option label \"Missing Option\"'
has "  the scroll failure says why" 'nothing to scroll to'

printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' '{"op":"text"}' > "$steps"
expect 0 "page errors do not fail the run" env STUB_PAGE_ERRORS=1 node "$QB" --out "$OUT" --steps "$steps"
[[ "$(grep -c '"event":"page-error"' <<<"$out")" == 3 ]] && ok "  three page-error lines (exception, console, log)" || bad "  page-error lines: $out"
has "  the exception line" '{"event":"page-error","kind":"exception","message":"Error: boom at app.js:1","step":1}'
has "  the console.error line" '{"event":"page-error","kind":"console","message":"request failed 500","step":1}'
has "  the log error line" '{"event":"page-error","kind":"console","message":"net::ERR_FAILED","step":1}'
if grep -qF 'just a log' <<<"$out"; then bad "  console.log must not be reported"; else ok "  console.log is not reported"; fi

echo "# new step refusals"
for bad_step in '{"op":"press","key":"Hyper+K"}' '{"op":"press","key":"K","modifiers":["super"]}' '{"op":"press","key":"F13"}' \
  '{"op":"scroll"}' '{"op":"scroll","selector":"a","by":3}' '{"op":"scroll","by":"far"}' '{"op":"scroll","by":{"z":1}}' \
  '{"op":"select","selector":"s"}' '{"op":"select","selector":"s","label":"a","value":"b"}' '{"op":"select","label":"a"}' \
  '{"op":"screenshot","name":"x","fullPage":"yes"}'; do
  printf '%s\n' "$bad_step" > "$steps"
  expect 2 "refused: $bad_step" node "$QB" --out "$OUT" --steps "$steps"
done

echo "# a full-page capture over 5 MB writes no file"
printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' '{"op":"screenshot","name":"bigfull","fullPage":true}' > "$steps"
expect 1 "an over-cap full-page screenshot fails" env STUB_BIG=1 node "$QB" --out "$OUT" --steps "$steps"
has "  says why" "over 5 MB"
[[ ! -e "$OUT/bigfull.png" ]] && ok "  no file written" || bad "  bigfull.png was written"

echo "# no browser"
expect 3 "no browser found exits 3" env DCV2_QA_BROWSER="$d/none" node "$QB" --out "$OUT" --steps "$steps"
has "  names the problem" "no Chrome-family browser found"
expect 2 "DCV2_QA_BROWSER is refused outside test mode" env -u DCV2_TEST_MODE node "$QB" --out "$OUT" --steps "$steps"

echo "# the browser dies with a SIGKILLed driver"
rm -f "$d/stub.pid"
printf '%s\n' '{"op":"open","url":"http://127.0.0.1:1530/"}' '{"op":"wait-text","text":"Never shown","timeoutMs":60000}' > "$steps"
node "$QB" --out "$OUT" --steps "$steps" >/dev/null 2>&1 &
drv=$!
for _ in $(seq 1 50); do [[ -s "$d/stub.pid" ]] && break; sleep 0.1; done
stubpid="$(cat "$d/stub.pid" 2>/dev/null || true)"
if [[ -z "$stubpid" ]] || ! kill -0 "$stubpid" 2>/dev/null; then
  bad "  the stub browser did not start"
else
  sleep 0.5
  kill -9 "$drv" 2>/dev/null || true
  wait "$drv" 2>/dev/null || true
  gone=no
  for _ in $(seq 1 30); do kill -0 "$stubpid" 2>/dev/null || { gone=yes; break; }; sleep 0.1; done
  [[ $gone == yes ]] && ok "  the browser exited within 3 s of kill -9 of the driver" || bad "  the browser survived kill -9 of the driver"
fi

[[ $fails == 0 ]] && echo PASS || { echo FAILED; exit 1; }
