# Security policy

## Reporting a vulnerability

Please report it privately through GitHub: open the repository's **Security** tab, choose **Report a vulnerability**, and describe what you found and how to reproduce it. Do not open a public issue or pull request for a vulnerability.

Include the app version (Settings, or the DMG name), your OS, and the steps. Never include real tokens, keys or the contents of your `.env` files; redact them first.

## Supported versions

Only the latest release is supported. Fixes land on the default branch and ship in the next release.

## Scope

DCTerminalv2 runs locally: it launches Claude Code, runs checks in git worktrees and keeps secrets in the OS keychain. Reports about the guard (writes outside a run's worktree), the Receipt secret scan, the `dcterminalv2://` link handler, or the release script are in scope. Problems in Claude Code itself belong with its maintainers.
