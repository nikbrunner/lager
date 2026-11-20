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

`--start` takes a local commit, tag or branch. Lager never fetches, so fetch first when the branch or tag lives on the remote only.

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

For every worktree, set `worktree_root` in the configuration:

```toml
root = "~/repos"
worktree_root = "~/worktrees"
```

`lager worktree add org/project hotfix` then creates `~/worktrees/org/project.worktrees/hotfix`. A primary checkout outside `root` needs `--path` while `worktree_root` is set.

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

- `a` asks for a name and shows the planned path and branch before anything is created. A name whose local branch exists checks that branch out; any other name creates a new branch.
- `r` inspects the selected worktree and shows blockers, or the losses with Cancel as the default.
- `c` copies the selected path.

The primary checkout is removed from the table with `r`, not from the popup. `--new-branch`, `--start` and `--path` are available on the command only.
