# Open checkouts as terminal sessions

Lager opens a checkout or a worktree as a session in [Rex](https://www.superlogical.com/) with your tabs and panes, each pane starting in the checkout. When the session exists, lager takes you back to it. The [CLI reference](../reference/cli.md#sessions) lists every outcome.

## Set up the section

Add a `[session]` section to the configuration with one layout called `default`:

```toml
[session]
host = "rex"

[session.layouts.default]
focus = "Agents"

[session.layouts.default.tabs.Agents]
order = 1
root = { pane = "Agent I" }
splits = [{ source = "Agent I", pane = "Agent II", direction = "right" }]

[session.layouts.default.tabs.Shells]
order = 2
root = { pane = "Shell I" }
splits = [{ source = "Shell I", pane = "Shell II", direction = "right" }]
```

Each tab starts as its `root` pane, and each split divides a pane in two. `focus` names the tab a new session shows first. The [configuration reference](../reference/config.md#session) has every field and rule.

Check the section with any command, such as `lager status`. An invalid section stops every lager command with a message that names the setting at fault, so fix it before anything else.

## Open a session

```sh
lager session open org/project
lager session open .
lager session open org/project hotfix
```

The first form opens the repository's primary checkout. The second opens the checkout or worktree of the working directory. The third opens the linked worktree whose directory is named `hotfix`.

The first `open` creates the session and switches Rex to it: `Opened session project`. The next `open` of the same checkout switches to the same session.

## Open a session from the inventory

Run `lager tui`, select a repository and press `s`. The inventory closes once Rex shows the session and prints `Opened session project`. When the session exists, it prints `Switched to session project`. Open session in the `m` menu does the same.

A `●` between the mark and the name shows a repository whose primary checkout has a session. Worktree sessions show in `lager worktree list`.

When Rex is missing or not running, the inventory says `Sessions unavailable: …` once, and `s` repeats the reason. Everything else in the inventory keeps working. The [CLI reference](../reference/cli.md#sessions-in-the-inventory) lists every outcome.

## Create a session for an agent

`create` makes the session and leaves Rex showing what it shows:

```sh
lager session create org/project hotfix --json
```

The answer names the session and maps every pane name to Rex's pane id, so an agent can start work in the pane it wants. Without `--json` it prints `Created session project · Hotfix`, or `Session project · Hotfix already exists`. See [session JSON](../reference/session-json.md) for the document.

For a session that already existed, the pane ids come from the record lager wrote when it created the session. A pane closed in Rex since then keeps its old id there, so treat the ids of an existing session as possibly stale.

## Start and finish a worktree with its session

The worktree commands open and close the session in the same step:

```sh
lager worktree add org/project hotfix --open-session
lager worktree remove ~/repos/org/project.worktrees/hotfix --close-session
```

`--create-session` on `add` creates the session without switching Rex to it. The [worktrees how-to](worktrees.md#start-a-topic-in-its-own-session) has the details.

## Close a session

```sh
lager session close org/project hotfix
```

This prints `Closed session project · Hotfix`, or `No session for PATH` when the checkout has none. Closing a session ends every program in its panes.

You can close the session you are working in. The pane closes with the session, so the `Closed session` line never shows, but the session is closed and lager forgets it.

## See which sessions exist

```sh
lager session list
lager session list --json
```

The listing shows the label, path and session id of every checkout with a live session.

## Know the limits

- Lager knows only the sessions it created. A session you made by hand in Rex, or one another tool made, is not found. `open` creates a session of its own beside it.
- The record of sessions lives on this machine, in `sessions.json` in lager's cache directory, `LAGER_CACHE_DIR` or `~/.cache/lager`. Deleting that file makes lager forget its sessions while they keep running in Rex. The next `open` creates a new session. Close the old ones in Rex.
- A session closed in Rex is gone for lager too. The next `open` or `create` makes a new one.
- Remove a worktree with `lager worktree remove PATH --close-session` to close its session in the same step. Once Git no longer lists a worktree, lager can no longer name it, so a session left behind by a plain removal stays in `lager session list`. Close that session in Rex. A worktree directory deleted without Git is still listed by Git, and `lager session close REPOSITORY NAME` closes its session.

## What `open` needs from Rex

- A running Rex server. Lager never starts it. With no server, every session command prints `error: session host unavailable: …` and exits `1`.
- The `rex` command. Lager looks for it on `PATH`, then at `/opt/homebrew/bin/rex`, `/usr/local/bin/rex`, in `REX_BIN_DIR`, and in the Rex Server app under `~/Library/Application Support/com.superlogical.rex`. A shell with a short `PATH` still finds the bundled one.
- A connected Rex app. `open` asks the app to select the session. Without one it prints `error: session host failed: no Rex app connected`.
- Remote Control turned on in Rex. Rex can refuse the select while this setting is off, and it is off by default. When `open` does not switch Rex to the session, check this setting first.

`create`, `close` and `list` need only the server.
