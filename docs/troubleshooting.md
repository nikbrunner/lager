# Troubleshooting

**`fzf` is missing.** Supply repository arguments to avoid interactive selection, or install `fzf` and retry the no-argument command.

**GitHub discovery fails.** Install `gh`, run `gh auth login`, and verify the configured host is `github.com`.

**A wildcard cannot expand.** Check its provider entry, API URL, credentials, and `--include-archived` choice. Explicit repository commands do not require provider access.

**A destination is a conflict.** `list` reports its state. Lager will not overwrite a non-matching directory or Git origin; inspect it and choose a different declaration/root.

**Removal is skipped.** Review the warning. Use `--force` only when the state warning is understood; it does not bypass root, symlink, repository-shape, or origin checks.

**A script gets unexpected stdout.** Use `list --json` for machine output. Human summaries and native Git/hook streams are intentionally separate from the JSON contract.
