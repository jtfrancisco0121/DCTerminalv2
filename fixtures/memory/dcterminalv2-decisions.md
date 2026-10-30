# Decisions
covers: docs/, plugin/roles/, scripts/
checked-at: @SHA@
- 2026-10-30 | No hosted CI: the gate is the local npm run check | the owner runs everything locally and a hosted pipeline would be a second, drifting gate | rules out: GitHub Actions or any other hosted CI workflow
- 2026-10-30 | No phone app: the phone reaches a run through remote-control and typed requests and pushes | remote-control already gives a phone a live session, so a native client adds upkeep for no new reach | rules out: a companion phone app or its own protocol
- 2026-10-30 | Approvals are asked only in the chat, by the orchestrator | an approval card elsewhere can be answered without the plan in front of the user | rules out: approving a plan or a ship from a push, a notification button or a panel-only control
- 2026-10-30 | One PR per repo per run | splitting a run into several PRs per repo slowed runs and made reviews harder to follow | rules out: split-by-size PR rules, one PR per epic and stacked PRs
