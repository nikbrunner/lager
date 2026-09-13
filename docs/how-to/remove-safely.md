# Remove a repository safely

`remove` permanently deletes a local repository after validating that it is a standalone, non-bare Git checkout inside the configured root and that its normalized `origin` matches the requested repository.

```sh
lager remove github.com/my-org/project --keep-registered --yes
lager remove github.com/my-org/project --unregister --yes
```

`--unregister` removes the declaration after successful disk removal; `--keep-registered` leaves it. A missing destination skips disk confirmation and still honors `--unregister`.

Symlinks, paths outside the root, the root itself, bare repositories, linked worktrees, plain directories, unreadable paths, and mismatched origins are rejected. Tracked or untracked changes, ignored files, stashes, dirty submodules, detached state, and unpushed commits are warnings. `--force` bypasses those warnings only; it never bypasses identity or boundary checks.

A non-TTY invocation requires one registration choice and `--yes`. With no repository argument, discovered clones may live at custom locations: the picker displays and retains each exact canonical path, and that same path is revalidated immediately before deletion. In a TTY, existing targets are shown and confirmed. Declining a warning or confirmation leaves the target and declaration unchanged. Failed or partial disk removal also leaves configuration unchanged.
