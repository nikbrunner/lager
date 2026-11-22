# Declarations and local state

Lager separates the repositories you want from the checkouts that happen to exist.

TOML declarations describe the desired set. `add` creates one local checkout and can declare it. `ensure` reconciles every effective declaration. `status` compares the declarations with disk without changing either.

A repository identity is normalized from its host and path for duplicate, destination, and origin checks. Lager preserves the stored clone URL text. It creates destinations below the configured root without following symlinks, then asks native Git to clone into that destination. Git retains responsibility for credentials, prompts, progress, and errors.

Registration, checkout availability and worktree management are independent concerns. Registering a repository does not create files, removing a worktree does not delete its branch, and no command prescribes a start, resume or finish lifecycle. The session flags of `worktree add` and `worktree remove` add one optional session step to a single worktree command.

This distinction lets a configuration travel between machines. A fresh machine runs `ensure`; a machine with existing checkouts sees their current state through `status`.
