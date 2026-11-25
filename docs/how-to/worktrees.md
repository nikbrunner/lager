# Work in several branches at once

A worktree is a second working directory of the same repository, checked out at another branch. Lager creates, lists and removes them through native Git, one repository at a time. Worktrees are not declarations: the configuration does not name them and `ensure` does not recreate them.

The commands need Git 2.36 or later. The [CLI reference](../reference/cli.md#worktrees) lists every option and blocker.

## Start a branch in a new worktree

```sh
lager worktree add org/project hotfix
```

This creates the branch `hotfix` at the primary checkout's HEAD and checks it out in `~/repos/org/project.worktrees/hotfix`, beside the primary checkout. The repository can be a reference, as here, or a checkout path such as `.` from inside it.

Check out a branch that already exists, or name the new branch apart from the directory:

```sh
lager worktree add . review --branch feature/login
lager worktree add . spike --new-branch dev/spike --start v2.3.0
```

`--start` takes a local commit, tag or branch. `--start` never fetches, so fetch first when the branch or tag lives on the remote only.

With `--branch`, the name is optional. Lager takes the last segment of the branch name, and a remote branch becomes a new local branch that tracks it:

```sh
lager worktree add . --branch origin/feat/session-tabs
```

This creates `~/repos/org/project.worktrees/session-tabs` on a new branch `feat/session-tabs`. For an existing local branch the output says how far it is from its remote branch, such as `2 behind origin/feat/session-tabs`; Lager never moves the branch. A branch that is checked out in another worktree is refused with the path of that worktree.

A branch pushed since the last fetch is unknown to the repository. Add `--fetch` to fetch every remote first:

```sh
lager worktree add . --branch origin/feat/session-tabs --fetch
```

A remote that fails to fetch prints a warning, and the command goes on as long as the branch is found.

## Start a topic in its own session

With a [`[session]` section](sessions.md#set-up-the-section) in the configuration, one command creates the worktree and opens it as a session in Rex:

```sh
lager worktree add org/project hotfix --open-session
```

After Git's output and the `Created worktree` line, lager prints `Opened session project · Hotfix` and Rex shows the new session, every pane in the worktree. `--create-session` makes the same session and leaves Rex where it is. When the session cannot be opened, the worktree stays and the command names what went wrong.

## Put worktrees somewhere else

For one worktree, pass the destination:

```sh
lager worktree add . spike --path ../spike
```

For every worktree, set `worktree_path` in the configuration:

```toml
root = "~/repos"
worktree_path = "~/worktrees/{relative}.worktrees/{name}"
```

`lager worktree add org/project hotfix` then creates `~/worktrees/org/project.worktrees/hotfix`. A primary checkout outside `root` needs `--path` while the template uses `{relative}`. [Worktree path](../reference/config.md#worktree-path) lists the other placeholders, such as `{parent}/{repo}-{name}` for a sibling folder.

## See what exists

```sh
lager worktree list org/project
lager worktree list --json
```

Without an argument the command lists the repository of the working directory. It shows the primary checkout and every linked worktree Git knows, also those created by other tools or placed elsewhere, with branch and `locked` or `prunable` state. With a `[session]` section, a last column names each worktree's session.

## Remove a worktree

```sh
lager worktree remove ~/repos/org/project.worktrees/hotfix
```

Lager shows the path, the branch it keeps and the local files that would be lost, then asks; the answer defaults to no. The branch always stays, so a removed worktree can be added again with `--branch`. With modified, untracked or ignored files the command refuses unless you add `--force`, which accepts exactly those losses. A locked worktree, a detached HEAD that no branch or tag holds, or a populated submodule blocks the removal even with `--force`; the error names the Git command that resolves it.

In scripts, pass `--yes`, and `--force` as well when losses are expected.

To end the worktree's session with it, add `--close-session`:

```sh
lager worktree remove ~/repos/org/project.worktrees/hotfix --close-session
```

Lager closes the session only once Git has removed the worktree, and prints `Closed session project · Hotfix`. When the removal is refused or you decline it, the session keeps running.

## From the inventory

In `lager tui`, select a repository and press `w`. The popup lists the same working trees as `worktree list`:

- `a` asks for a name and shows the planned path and branch before anything is created. A name whose local branch exists checks that branch out, unless another worktree has it; any other name creates a new branch.
- `r` inspects the selected worktree and shows blockers, or the losses with Cancel as the default.
- `c` copies the selected path.

With a [`[session]` section](sessions.md#set-up-the-section), a `●` marks each worktree that has a session, and `s` opens the selected one. The plan opens the new worktree's session after Git created it, and the removal summary closes the worktree's session after Git removed it. Each shows as a ticked line, and `s` clears the tick for that worktree.

The primary checkout is removed from the table with `r`, not from the popup. `--new-branch`, `--start`, `--path` and `--fetch` are available on the command only.
