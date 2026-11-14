# Troubleshooting

**`fzf` is missing.** Supply repository arguments to avoid interactive selection, or install `fzf` and retry the no-argument command.

**GitHub discovery fails.** Install `gh`, run `gh auth login`, and verify the configured host is `github.com`.

**A wildcard cannot expand.** Check its provider entry, API URL, credentials, and `--include-archived` choice. Explicit repository commands do not require provider access.

**A destination is a conflict or unknown.** `status` reports `conflict` when another repository's checkout sits at a declared destination and `unknown` when something else does. Lager will not overwrite a non-matching directory or Git origin; inspect it and choose a different declaration/root.

**Removal is refused with `unsafe local state`.** Lager removes a checkout without `--force` only when it can show that nothing is lost, and it names each part it cannot show. `worktree remove` applies the same rule and lists what it found under `losses`. Push or delete what a `commits not on any remote` warning lists, finish or drop a named operation with a command in its brackets, and look at the entries an `unknown entries in` warning names before deciding; the [CLI reference](reference/cli.md#what-remove-proves) explains every warning. `--force` accepts exactly the named warnings. It does not bypass root, symlink, repository-shape, or origin checks.

**A script gets unexpected stdout.** Use `status --json` for machine output. Human summaries and native Git/hook streams are intentionally separate from the JSON contract.

**The inventory is empty and says the root does not exist yet.** The configured `root` names a directory that is not there. Check it for a typo, or create it; `status` and `tui` report it as a note and exit `0` until then.
