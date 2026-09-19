# Remove a repository safely

`remove` permanently deletes a local repository only after it validates a standalone, non-bare Git checkout inside the configured root and matches its normalized `origin` to the requested repository.

```sh
lager remove github.com/my-org/project --keep-registered --yes
lager remove github.com/my-org/project --unregister --yes
```

`--unregister` removes the declaration after successful disk removal. `--keep-registered` leaves it. A missing destination skips the disk confirmation and still honors `--unregister`.

## Understand the guards

Lager rejects symlinks, paths outside the root, the root itself, bare repositories, linked worktrees, plain directories, unreadable paths, and mismatched origins.

Lager also rejects a primary checkout whose Git common directory contains dependent worktree metadata. Every entry in `worktrees` blocks removal, including live, external, locked, stale (missing checkout), or malformed entries. A symlink, unexpected file type, or failure to read that metadata also blocks removal. An absent or empty `worktrees` directory is allowed, subject to the other guards.

These checks run during initial inspection and again before deletion. Lager does not prune, repair, or remove dependent worktrees for you. Resolve their state with Git before retrying; `--unregister` leaves the declaration unchanged when removal is blocked. Other eligible repositories in the same batch can still be removed.

Tracked or untracked changes, ignored files, stashes, dirty submodules, detached state, and unpushed commits are warnings. `--force` accepts these warnings only. It never bypasses the root boundary, symlink rejection, repository-shape checks, dependent worktree checks, or origin matching.

A removal confirmation defaults to no. After a successful removal, the unregister prompt defaults to yes. Outside a terminal, pass `--yes` and exactly one of `--unregister` or `--keep-registered`.

## Remove from the picker

With no repository argument, Lager lists standalone local checkouts and passes the selected canonical path through deletion. It revalidates that exact path and its origin immediately before deleting it.

If the selected checkout vanishes, removal fails and leaves registration
unchanged. Lager does not fall back to another clone of the same origin, including
the configured destination. Picker labels and confirmations show visible escapes
for control characters and literal backslashes; the actual selected path is
unchanged.

Declining a warning or confirmation leaves the checkout and declaration unchanged. Failed or partial disk removal also leaves configuration unchanged: configuration cleanup happens only after disk removal succeeds.

See the [CLI reference](../reference/cli.md) for exit status and non-interactive rules.
