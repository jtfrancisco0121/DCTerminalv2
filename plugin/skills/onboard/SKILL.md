---
name: onboard
description: Standalone Onboard - create, rebuild or refresh the codebase memory (index, topics, contracts, workspace) for the repos in the current folder, without changing code. Usage - /dcv2:onboard [rebuild | stale | <repo name>].
---
# Onboard

Request: `$ARGUMENTS`

This is a standalone session: you talk to the user directly in this tab. There is no run folder and no status file.

1. First, before anything else, Read `${CLAUDE_PLUGIN_ROOT}/roles/onboard.md` and follow it: it is your role (`<plugin>` there is `${CLAUDE_PLUGIN_ROOT}`). The rules below add to it for standalone use.
2. An empty request means: create the memory where it is missing and refresh stale topics; `rebuild` rewrites everything (keeping pinned lines); `stale` refreshes only stale topics; a repo name limits it to that repo.
3. Work in the current folder. It may be one git repository or a folder of repositories (each child folder a repo).
4. Write memory only with `bash ${CLAUDE_PLUGIN_ROOT}/scripts/memory.sh write <id> <file> <<'EOF'` … `EOF`; read what exists with `bash ${CLAUDE_PLUGIN_ROOT}/scripts/memory.sh status .` and the Read tool. Never Edit or Write in the memory folder, never change the repos.
5. Never print secrets (tokens, keys, `.env` values), and never store them.
6. End with the role's final report, and the result of `memory.sh status .` in one line per repo (present, topics, stale).
