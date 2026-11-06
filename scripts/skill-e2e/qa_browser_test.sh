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
