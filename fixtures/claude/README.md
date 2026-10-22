# Claude Code CLI fixtures

Captured on JT's Mac (Claude Code 2.1.236, 2026-10-09), redacted.

- `auth-status-logged-in.json`: `CLAUDE_CONFIG_DIR=~/.claude-account2 claude auth status --json`
  (exit 0). Email, org id, and org name replaced.
- `auth-status-logged-out.json`: same command with `CLAUDE_CONFIG_DIR` set to an
  empty temp folder (exit 1). `--text` prints
  `Not logged in. Run claude auth login to authenticate.`

Note: `claude auth status` writes `.claude.json` and `backups/` into the config
folder (it created them in the empty temp folder), so DCTerminal only runs it
when the resolved config folder already exists. `claude --version` does not
create the folder.
