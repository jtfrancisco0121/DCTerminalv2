# Memory updater

Keep the codebase memory true after a change, cheaply. You read only the diff of the change and patch only the memory topics whose covered paths it touched. You change facts that changed and nothing else: no diary entries, no history, no new opinions (git and the run report keep history).

## Rules

- Read only `git diff <from>..<to>` (and the touched topic files). Do not explore the codebase.
- Write memory **only** with `bash <plugin>/scripts/memory.sh write <repo id> <file> <<'EOF'` … `EOF` (`<plugin>` is the folder that holds this `roles/` folder). It refuses over-cap files, secrets, a changed or removed `<!-- pinned -->` line, and a topic without its `covers:` / `checked-at:` header; fix what it says and write again (condense when over the cap).
- Never change the repos: no edits, commits, branch switches. Read-only git only.
- Never store secrets. Keep every `<!-- pinned -->` line exactly as it is.
- Memory is a hint, the code is the truth: when the diff shows a remembered fact is wrong, fix it.
- Do not create new topics (that is `/dcv2:onboard`'s job); list important uncovered changes instead.

## Process (per repo given)

1. `bash <plugin>/scripts/memory.sh touched <repo path> <from> <to>`: the topics whose covered paths changed, and the changed paths no topic covers. `memory.sh status`'s repo has `"present": false` (no `index.md`) → nothing to update for that repo: say "no memory for <repo> (run /dcv2:onboard)".
2. "no topic touched" → nothing to write for that repo, unless the change obviously breaks a fact in `index.md` (a renamed check command, a moved top-level folder): then patch just that line.
3. For each touched topic: read it, read `git -C <repo path> diff <from>..<to> -- <its covered paths>`, and rewrite the topic with only the changed facts patched. Keep its title and `covers:`; set `checked-at:` to `git -C <repo path> rev-parse <to>`. If nothing in the topic became wrong, still write it with the new `checked-at` (it was checked).
4. `index.md`: patch only when a command, the map, or the topic list changed.

## Output

A short summary per repo: topics updated (name, old → new `checked-at`), lines changed in `index.md`, writes memory.sh rejected, important uncovered paths. Or "no topic touched".
