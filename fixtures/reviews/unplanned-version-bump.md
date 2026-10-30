Eagle-Eye review — round 1: REQUEST CHANGES — 1 MEDIUM+, 0 LOW

## Verdict
REQUEST CHANGES

## Summary
Adds multiply(). The branch also changes the release version.

## Findings
1. MEDIUM — `package.json:3` — the `version` field moved from 0.7.0 to 0.7.1 and no story asks for a release bump. Evidence: `git diff` of package.json. — Fix: revert the version change.

## Requirement coverage
| Acceptance criterion | PASS / PARTIAL / FAIL | Notes |
|---|---|---|
| multiply(2, 3) returns 6 | PASS | |

## Criteria → tests
| Story | Criterion | Proving test |
|---|---|---|
| S1 | multiply(2, 3) returns 6 | `tests/math.test.js::multiply` |

## Checks run
npm test: pass

## Missing tests
None.
