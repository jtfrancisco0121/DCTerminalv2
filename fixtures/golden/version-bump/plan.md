# Plan

## Summary
Add the exclamation mark to shout().

## Stories

### S1: Add the exclamation mark
- Repo: shout
- PR: PR1
- Tickets: none
- Files: index.js
- Changes: `shout(s)` returns `s.toUpperCase() + "!"`.
- Acceptance criteria:
  - [ ] `shout("hi")` returns `"HI!"`.
  - [ ] `shout("")` returns `"!"`.

## Pull requests

### PR1: Add the exclamation mark
- Repo: shout
- Stories: S1
- Depends on: none
- Epic: none
- Why: shout() drops the exclamation mark the callers expect.
- Check: `npm run check`
