# Plan: Add a hosted pipeline

## Summary
Add a GitHub Actions workflow that runs npm run check on every push.

## Stories

### S1: Add the workflow
- Repo: DCTerminalv2
- PR: PR1
- Why: nothing runs the gate when the owner is away
- Changes: `.github/workflows/check.yml` runs `npm run check`.
- Acceptance criteria:
  - [ ] a push runs the check on GitHub

## Pull requests

### PR1: Hosted pipeline
- Repo: DCTerminalv2
- Stories: S1
- Depends on: none
- Why: nothing runs the gate when the owner is away
- Check: `npm run check`

## Manual steps
None.

## Risks
| Risk | Severity | Mitigation |
|---|---|---|

## Open questions
None.
