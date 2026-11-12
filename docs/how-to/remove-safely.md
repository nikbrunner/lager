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

With no repository argument, Lager lists every checkout under the root that has an `origin`, without descending into checkouts. The guards run only on the checkout you select, so a blocked checkout still appears in the list and fails with its reason. Lager passes the selected canonical path through deletion and revalidates that exact path and its origin immediately before deleting it.

If the selected checkout vanishes, removal fails and leaves registration
unchanged. Lager does not fall back to another clone of the same origin, including
the configured destination. Picker labels and confirmations show visible escapes
for control characters and literal backslashes; the actual selected path is
unchanged.

Declining a warning or confirmation leaves the checkout and declaration unchanged. Failed or partial disk removal also leaves configuration unchanged: configuration cleanup happens only after disk removal succeeds.

## Remove from the inventory

In `lager inventory`, `r` removes the current row's checkout and keeps its registration; the menu's Repository actions… also offers Remove + unregister. Both act on the marked rows when any are marked, and both run the guards above on each exact found path.

The confirmation lists every target before anything is deleted: clean targets, targets with named local-state losses, and blocked targets with the guard that forbids them. Cancel is the default. Targets with losses are removed only after you tick the acknowledgment row, which covers the listed losses at the listed paths and nothing else. It never unblocks a guard.

Keys typed before the confirmation is on screen are discarded, so it cannot be answered unseen.

Each target is checked again immediately before deletion. A changed origin, a path that moved or vanished, another checkout put at the same path, a changed registration, or a loss that was not on the list blocks that target as stale and leaves it and its registration alone; the other targets go on. Remove + unregister needs a complete scan and unregisters an identity only after every selected checkout of it is gone, and a declared repository whose destination is missing is unregistered only if that destination is still absent.

A running batch stops before its next target when you cancel it from the menu, the batch popup or the quit prompt. Completed deletions are not rolled back.

See the [CLI reference](../reference/cli.md) for exit status and non-interactive rules.
