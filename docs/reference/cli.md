# CLI reference

```text
lager [--config PATH] <COMMAND>
```

Lager selects the configuration path in this order:

1. `--config PATH`
2. `LAGER_CONFIG`
3. `$HOME/.config/lager/config.toml`

`--config` and `LAGER_CONFIG` accept any path. The default path is derived from `HOME`, or from the current directory when `HOME` is unavailable.

## Commands

| Command | Syntax | Behavior |
| --- | --- | --- |
| `init` | `init --root ROOT [--create-root\|--no-create-root] [--github\|--no-github]` | Creates a configuration once. It optionally creates the root and adds the GitHub provider. |
| `register` | `register [REPOSITORY]... [--post-clone CMD] [--include-archived]` | Declares repositories or wildcards. A hook belongs to an explicit declaration. |
| `unregister` | `unregister [REPOSITORY]... [--include-archived]` | Removes explicit declarations, or excludes concrete members from matching wildcards. |
| `add` | `add [REPOSITORY]... [--register\|--no-register] [--post-clone CMD] [--include-archived]` | Creates local checkouts. A checkout already at the destination is a no-op. `--post-clone` implies `--register`. |
| `remove` | `remove [REPOSITORY]... [--unregister\|--keep-registered] [--yes] [--force]` | Permanently deletes validated local checkouts. `--force` accepts local-state warnings only. |
| `ensure` | `ensure [--include-archived]` | Creates missing effective declarations sequentially. It never pulls or resets an existing checkout. |
| `hook` | `hook [REPOSITORY]...` | Runs hooks for explicit declarations. Without repositories, it selects explicit declarations. |
| `status`, `list`, `ls` | `status [--remote] [--include-archived] [--registered] [--json]` | Prints the rows `tui` shows, once its scan has finished: a table, or one JSON document with `--json`. `--remote` expands wildcards through providers; `tui` with remote discovery on shows those rows and adds the providers' unregistered and excluded repositories as candidates. `--registered` keeps declared repositories and wildcard patterns. |
| `tui` | `tui [--remote] [--include-archived]` | Opens the inventory with its repository actions; requires TTY stdin and stdout. `--remote` starts with remote discovery on. |
| `worktree list` | `worktree list [REPOSITORY] [--json]` | Lists the primary checkout and every linked worktree of one repository. |
| `worktree branches` | `worktree branches [REPOSITORY] [--remote] [--json]` | Lists the local branches of one repository, or with `--remote` its remote-tracking branches, newest commit first. |
| `worktree add` | `worktree add <REPOSITORY> [NAME] [--branch EXISTING \| --new-branch NEW] [--start REF] [--path PATH] [--fetch] [--open-session \| --create-session] [--json]` | Creates a linked worktree through `git worktree add`. |
| `worktree remove` | `worktree remove <PATH> [--yes] [--force] [--close-session]` | Removes one linked worktree through `git worktree remove` and keeps its branch. |
| `session open` | `session open [REPOSITORY] [NAME]` | Shows the checkout's terminal session in Rex, creating it first when there is none. |
| `session create` | `session create <REPOSITORY> [NAME] [--json]` | Creates the checkout's session without showing it. |
| `session close` | `session close [REPOSITORY] [NAME]` | Closes the checkout's session. |
| `session list` | `session list [--json]` | Lists the checkouts that have a session. |
| `skill` | `skill [install]` | Prints the usage guide for agents. `skill install` writes the agent skill to `$HOME/.agents/skills/lager`. |

`list` and `ls` are visible aliases of `status`. All three names accept the same arguments, options, and configuration selection, and produce identical output and exit statuses.

`status` needs no terminal. It scans the root the way `tui` does, through Git with optional locks off, and writes nothing to the configuration or to any repository. The table has the columns of the full inventory grid (repository, owner, local, branch, status, registration) with the same cell text, ordered by owner and then repository name as the grid is, aligned with two spaces, never truncated. A repository the provider reports as archived reads `NAME [archived]` in the repository column. An empty inventory prints the header only. A root that does not exist yet, or a symlink to nothing, is an empty, complete scan: the declared rows read `missing`, stderr gets one note such as `lager: ~/repos: root does not exist yet`, and the exit status is `0`. A root that is a plain file, or a symlink to one, is a problem. Each problem (an unreadable path, a root that is not a directory, a failed Git observation, a failed provider) is one line on stderr, and the rows are still printed. See [status JSON](status-json.md) for the document.

Repository arguments accept canonical IDs such as `github.com/org/project`, GitHub shorthand such as `org/project`, and full SCP, SSH, HTTP(S), or file clone URLs. GitHub shorthand becomes an SSH clone URL.

Explicit clone references retain their transport and text, with or without a
`.git` suffix. SCP accepts ordinary `USER@HOST:PATH` usernames (not only `git`);
SSH URLs retain custom usernames and ports. Canonical IDs retain default SSH.
Only HTTP(S) `/projects/PROJECT/repos/REPO/browse` URLs, optionally with one
trailing slash, are browser conveniences: these become
`ssh://git@HOST:7999/PROJECT/REPO.git`. Neighboring `/archive` and `/browse/src`
paths remain literal clone URLs.

HTTP(S) userinfo (including username-only tokens), passwords in any scheme,
all file-URL userinfo, and all queries/fragments are rejected. A rejected
reference aborts argument preflight before prompts, writes, or subprocesses,
without echoing its value. Use credential helpers or SSH authentication instead
of embedding credentials. See [configuration](config.md) for loaded-config behavior.

## Defaults and interactive behavior

| Situation | Default behavior |
| --- | --- |
| `init` in a terminal without `--root` | Prompts with `repos`. |
| `init` without a root-creation choice | Prompts to create the root, default yes. |
| `init` without a GitHub choice | Prompts to configure GitHub, default yes. |
| `register`, `unregister`, `add`, or `hook` without repositories | Uses `fzf --multi` to select candidates. Outside a TTY it exits `2` with `lager: COMMAND requires a repository outside a TTY` and starts no `fzf`. |
| `session open` or `session close` without `REPOSITORY` | Uses `fzf` to pick one checkout; see [Picking the checkout](#picking-the-checkout). |
| `remove` without repositories | Uses `fzf` to select local checkouts that have an `origin`; guards run after selection. Outside a TTY, after the flag check, it exits `2` with `lager: remove requires a repository outside a TTY` and starts no `fzf`. |
| Interactive `register` without `--post-clone` | Asks whether to add a hook for each repository, default no. |
| Interactive `add` without a registration choice | Asks whether to register each successful checkout, default yes, then offers its hook. |
| Interactive `remove` without `--yes` | Asks to remove each existing checkout, default no. |
| Interactive `remove` without a registration choice | Asks whether to unregister after successful or absent disk removal, default yes. |
| `status` without `--remote` | Reads configuration and disk only. Wildcard members appear only where a checkout exists. |
| `status` without `--json` | Prints an aligned table with a header row. |

`init` still writes the configuration when Git, `fzf`, or authenticated `gh` is missing. It reports those missing tools on stderr.

### Registration batches

`register` collects all per-repository hook choices before committing one atomic
configuration update, whether references come from arguments or the picker.
Invalid references, conflicting hooks, or cancelled prompts leave the original
configuration unchanged. Cancellation exits 130.

Successful batches report `added` or `already managed` for each input in order.
Equivalent repeated references are no-ops after the first change. Retrying the
same batch is safe. `add` and `ensure` retain per-target behavior; they are not
all-or-nothing clone batches.

## What `remove` proves

`remove` deletes a checkout without `--force` only when it can show that nothing is lost. Each part it cannot show is a warning with a name. In a terminal Lager lists the warnings one per line and asks whether to force the removal; outside a terminal it exits `1` with `lager: REPOSITORY: unsafe local state: WARNING; WARNING; rerun with --force to accept these losses`. With `--force` it prints `lager: REPOSITORY: removing with --force; this is lost: WARNING; WARNING` on stderr before it deletes, without the commands in brackets.

| Warning | What Lager found |
| --- | --- |
| `staged changes`, `unstaged changes`, `untracked files`, `ignored files` | `git status` lists them. A submodule checked out at another commit than the recorded one counts whatever `submodule.NAME.ignore` or `diff.ignoreSubmodules` say. |
| `tracked files hidden from git status (skip-worktree or assume-unchanged)` | A tracked file with one of these marks is present. Git's status skips it, edited or not. |
| `nested repository PATH` | A directory in the checkout holds a `.git` entry of its own. |
| `stashes` | The stash holds an entry. |
| `detached HEAD`, `unsafe branch tip`, `no upstream` | HEAD names no branch, no commit, or a branch without an upstream. |
| `local commits absent from the remote-tracking ref` | The branch is ahead of its upstream, and the ref warning does not list it already. |
| `commits not on any remote: branch NAME, tag NAME, REF` | The commit a local branch, tag or other ref names is reachable from no remote-tracking ref as of the last fetch. Refs under `refs/prefetch/` are copies of remote refs and are not listed. A branch whose upstream is another local branch is listed here. So is a tag fetched from the remote whose commit no remote branch holds. A warning with more than eight refs ends with the number of the rest. |
| `submodule PATH: WARNING` | An initialized submodule fails the same proof as the checkout, nested submodules included. In a submodule the commit HEAD names counts as a ref, listed as `HEAD`, because Git checks submodules out detached. A shallow submodule whose commit is below the tip it fetched reads as `HEAD` not on any remote, since its history does not reach a remote-tracking ref; `--force` accepts it. |
| `submodule repositories in .git/modules: NAME` | Git keeps a submodule's repository there that no initialized submodule uses, as after `git submodule deinit`. |
| `submodule state is unknown: REASON` | Git could not answer, or an unpopulated submodule directory holds files that Git does not report. |
| `stopped rebase (git rebase --continue or git rebase --abort)`, `stopped git am (git am --continue or git am --abort)`, `merge in progress (...)`, `cherry-pick in progress (...)`, `revert in progress (...)`, `unfinished sequence of picks or reverts (...)`, `bisect in progress (git bisect reset)` | The Git directory holds the state of that operation. The commands in brackets finish it or drop it. |
| `autostash of a stopped rebase`, `autostash of a stopped merge`, `autostash of an interrupted checkout` | Git set local changes aside in the Git directory and has not put them back. |
| `unknown entries in .git: NAME, NAME` | The Git directory has a top-level entry that Git does not keep there in normal use and that holds something, such as the state of another tool. An empty file or directory is not listed, nor are `CLAUDE_BASE` and `opencode`, which hold only an id a coding agent keeps. |

The commits part reads the real objects, without replacement refs or grafts, so neither can make a commit look pushed. The Git-directory part reads top-level entries only. A checkout's Git directory holds without a warning `HEAD`, `config`, `config.worktree`, `description`, `index`, `objects`, `refs`, `reftable`, `packed-refs`, `info`, `hooks`, `logs`, `FETCH_HEAD`, `ORIG_HEAD`, `COMMIT_EDITMSG`, `REBASE_HEAD`, `AUTO_MERGE`, `gc.log`, `shallow`, `lfs`, `fsmonitor--daemon`, any `.ipc` socket, `CLAUDE_BASE`, `opencode` and `modules`. A symlink in `modules` counts as an unused submodule repository. The Git directory of a linked worktree has its own set, listed under [`worktree remove`](#worktree-remove).

Two inspections of the same checkout give the same warnings, word for word. The inventory lists them as the losses of a target.

The proof covers what the remote-tracking refs held at the last fetch; Lager does not contact the remote, so a branch deleted there after the last fetch still counts as pushed. Commits that only a reflog reaches, the names of local branches and tags whose commits a remote holds, local configuration, and the files inside `info/`, `logs/`, `hooks/` and `lfs/` (custom hooks, notes in `info/`, LFS objects never uploaded) go with the checkout unnamed: of the Git directory only the top-level entries are read.

## Inventory

The screen reads top to bottom: a header bar with the mode, the visible row count and, once one has occurred, the problem count; the source line; the search row; the unbordered table with a mark column; the status line, which shows the current state only; and the hint footer. The grid has the columns repository, owner, local, branch, status and registration. The owner is the first folder of the row's path below the root, so `~/repos/imfusion/cp/repo` shows `imfusion`; a path outside the root, or a repository directly in the root, shows `—`. Rows are ordered by owner, then by the repository name shown, both ignoring case, with rows that have no owner last; the order holds through refreshes and search narrows the ordered list. `status --json` keeps its row-key order. The grid shows the widest of three column sets that fits the terminal: all six columns from 113 columns, without registration from 99 columns, and below that repository, branch and status. A hidden column is not drawn, and Inspect still shows its value. The table shows short repository names and `~`-relative paths; Inspect shows the full values, the origin among them. A row is selected whole, without shifting its text, and there is no permanent detail pane or cell focus. Query, cursor, marks and cached provider answers last for one run.

`tui` starts from declarations and local
Git roots beneath the configured root. It makes no provider request until remote
discovery is on and writes no configuration until an action is chosen. Registration and checkout state are independent; duplicate origins keep
separate rows for their exact paths. Originless checkouts display `No origin`.

Discovery recurses through directories, prunes found Git roots, and excludes
directory symlinks, linked worktrees, submodules and bare repositories. Unreadable
paths produce partial-scan diagnostics. Pending, failed and stale observations
remain distinct from clean status.

The source line under the header says how far the local sources are:
`Local + declarations · remote off` once the scan has read everything and every
Git status is in, otherwise `Local: scanning`, `Local: incomplete, still
scanning`, `Local: incomplete`, or `Local: ready` for a finished scan while Git
status is pending or remote discovery is on, followed by `status: N pending` while Git
observations are outstanding. A scan that could not read a path shows one
warning line above the state, `! Scan incomplete:` with each unreadable path and
its reason and each ignored repository (`PATH ignored: bare repository`); the
failures are also listed in Problems. An inventory with no rows and
nothing declared says so in the table, names the root, and offers `a` to add a
repository; its status line reads `Scan complete · 0 repositories · 0
declarations` and its footer shows only the keys that act without a row. A root
that does not exist yet is shown the same way, with `~/repos does not exist
yet` in the table, or as a status line when declarations give the table rows;
it is not a problem.

Press `R` to reload configuration and refresh local observations. Earlier
observations remain marked stale until refreshed. Press `q` or Esc to cancel
background work and quit, restoring the terminal. A normal quit returns `1` if
an operational failure occurred during the run, even after a successful
refresh; otherwise it returns `0`. Informational exclusions and deliberate
cancellation do not count as failures.

Use `j`/`k` or arrow keys to select a row, `/` for live ranked search, Space to
mark the row, `a` to add a repository by reference, `r` to remove the checkout and keep its registration, `w` for the worktrees of the selected repository, `g` for lazygit in its checkout, `f` to filter the rows by property, `s` for its session, Enter to inspect the selected repository, `m` for the action menu,
and `?` for effective bindings. Search ranks the rows by repository name and owner, best
match first, in fzf syntax (space, `'`, `^`, `$`, `!`, smart case); a query with a `/`
matches the local path as the grid shows it. Typing moves the cursor to the best match. Enter/Esc
retain the search and normal-mode Backspace clears it. Ordinary search letters
remain text; F1 opens search help. Help describes the mode it was opened from.
Marks survive search, filter and refresh while their exact target remains; the search
row shows the marked and hidden-marked counts, and the status line reports marks
removed with a vanished entry. Wildcard pattern rows cannot be marked.
The menu groups actions for this row (Repository actions…, Inspect, Copy value…,
Open origin, Worktrees…, Open lazygit, and Open session with a `[session]` section), the
selection (mark this row, mark visible concrete rows, clear all marks) and the
inventory (Add, Filter…, Remote discovery…, Refresh, Problems…, batch cancellation, Help
and Quit).
`f` or Filter… opens a popup that narrows the rows by property. Changes offers
`dirty` (the row has changes) and `clean` (observed clean); Owner offers every
owner of the inventory in the grid's order, and `—` for rows without one;
Registration offers `explicit`, `wildcard`, `unregistered` and `excluded`,
each only while a row has it. With a `[session]` section and once the session
list has answered, Session offers `open` (the row's primary checkout has a
live session, the `●` of the grid) and `none`. Each value shows the number of
rows that have it in the whole inventory, before search. Values within a group
combine with "or", groups combine with "and", and a group with no value ticked
does not filter. A row without a changes observation (missing, remote only,
unknown, error) matches neither `dirty` nor `clean`, so a ticked Changes value
hides it. A row whose changes are stale after a refresh keeps its last known
value until it is observed again.
`j`/`k` move over the values, Space ticks one and the table behind the
popup narrows at once, `c` clears every value, and Enter, Esc or `q` closes the
popup with the filter in place. While a filter is active the source line ends with
`filter:` and the ticked values (a session value reads `session open`), and the
header counts `N of M rows`. Search applies on top of the filter, and marks on
rows the filter hides behave as under search: the search row counts them as
hidden and a batch names them as hidden. The filter reads row data, so it
applies in every column set. It lasts for the run and survives a refresh; a
value that disappears stops matching and drops out of the popup, and a ticked
session value keeps applying while the session list is refreshed.
`Ctrl+o` ticks or clears the Session value `open` of that same filter without
the popup, so the popup, the source line and the header show it. Without a
`[session]` section, or before the session list has answered, it changes
nothing and the status line gives the reason.
`Ctrl+d` ticks or clears the Changes value `dirty` the same way.
On a row whose host has a `github` provider, Repository actions… adds an
`On <host>` group with Rename…, Archive, Delete everywhere… and the visibility
entries. Their forms and confirmations, and the form of New repository on
GitHub…, change nothing on GitHub until their choice is made; Esc closes them
with `<Action> cancelled · nothing changed`. After a rename, GitHub redirects
the old URL to the new name.
`w` or Worktrees… opens the worktrees of the current row's checkout; marks are
not used. The popup lists the primary checkout and every linked worktree Git
knows, wherever it sits, with its path, branch and state: `primary`, `locked`
and `prunable` with Git's reason, and `clean` or the counts of modified and
untracked files. `a` opens Add worktree with three tabs, New, Local and Remote;
`[` and `]` switch them, and the selected tab shows in brackets, `[Local]`. New
asks for one name with the form of `NAME` in [`worktree add`](#worktree-add) and
creates a new branch of that name at the primary checkout's HEAD, as the command
does without `--branch`. A name whose local branch exists is refused with
`branch NAME exists; pick it on Local`. Local lists the local branches and
Remote the remote-tracking branches, the newest commit first, each with its
commit and its age. Local shows where a branch is checked out, `primary` for the
primary checkout; Remote shows the local branch of the same name and how far
apart the two are, such as `feat/x · 2 behind` or `main · same commit`, and where
that local branch is checked out, such as `feat/x · 2 behind · in core.worktrees/x`
or `main · same commit · primary`. A remote's `HEAD` is left out. `/` types into
a filter by branch name; while typing, Enter keeps the filter and Esc clears it.
Outside the filter, Esc closes Add. Enter on a branch picks it as `--branch` does:
a local branch is checked out as it is, and a remote-tracking branch gets a new
local branch that tracks it. The worktree is named after the branch's last `/`
segment. A branch that is checked out in another worktree is refused on the
status line with `! BRANCH is checked out in PATH`. On Remote, `f` fetches each
remote on its own while the list stays usable; the status line reads
`Fetching NAME: origin, upstream… N s`, and afterwards `Fetched origin, upstream`, or
`! fetch REMOTE failed: REASON` for each remote that failed, and the list is read
again with the selection kept on its branch. A list that cannot be read again stays
as it was, and the status line says so. Git shows no prompt on the terminal during
that fetch. The fetch goes on when the popup closes; until it ends, `f` reports
`! a fetch is already running` and its hint is hidden. The tab row shows
`last fetch HH:MM`, or `never fetched`. A repository without remotes shows
`No remote branches`, and `f` reports `! NAME has no remote to fetch`.
`--new-branch`, `--start` and `--path` belong to the command only. Enter shows
the planned path and branch; a picked branch also shows the worktree's `name`,
with "Change name" to edit it and "Back to branches". Nothing is created before
"Create worktree" is chosen there. `r` runs the guards of [`worktree remove`](#worktree-remove) on
the selected worktree. A blocker is shown with its hint and no confirmation.
Otherwise a summary of losses, path and kept branch asks with Cancel as the
default; choosing removal accepts exactly the listed losses, and a worktree that
changed since the summary is left alone. The choices stay at the bottom of the
popup. In a terminal too small to show the losses, the full path, the kept
branch and both choices, or the plan and its choices, the popup says so and the changing choice does nothing until
the terminal is enlarged. The blocker view scrolls. Keys typed before a plan or summary is
on screen do not answer it. `r` on the primary checkout reports that it is
removed from the table. `c` copies the selected path. While Git runs, the
inventory hands it the terminal as for a clone; Ctrl-C there exits `130`, and
after a failure Enter returns to the popup. When Git fails a removal partway,
`remaining:` lines name what is left: the directory, the worktree metadata, or
nothing. A row without a checkout (missing,
remote only, wildcard pattern, unreadable) reports `Worktrees unavailable: …`
and opens nothing.
`g` or Open lazygit in the menu hands the terminal to `lazygit` in the current
row's checkout. The program is `lazygit` from `PATH`; marks are not used. When
lazygit exits, whatever its status, the inventory takes the terminal back and
observes that row's branch and changes again, without scanning the other rows;
the cursor stays on the row. A row without a checkout (missing, remote only,
wildcard pattern, unreadable) reports `Open lazygit unavailable: …`, a running
batch reports `a batch is running`, and without `lazygit` on `PATH` the status
line reads `lazygit not found on PATH`. None of these starts a process.
Repository actions… → Register and Unregister act on the marked rows, hidden
ones included, or on the current row when nothing is marked. They change the
configuration only, through the same registry services as `register` and
`unregister`: Register keeps a checkout's own clone URL text, otherwise the
identity, and removes the member's exclusions from every matching wildcard;
Unregister removes the explicit declaration and excludes the member from every
matching wildcard. On marked rows a preview lists every target with its exact
path and identity, eligible, no-op or blocked with the reason; the list scrolls
while Cancel and the action stay in view, Cancel is the default and changes
nothing. A target that the search or the filter hides carries `(hidden)` in the list and
in the outcomes, and the action choice adds `K hidden`. The current row alone acts directly. Targets are fixed when
the action starts, and a refresh never changes them. They run one after another,
each with its own configuration write; targets of one identity share one change
and one reported failure. The batch popup lists every target with its outcome
(succeeded, no-op, failed, blocked, cancelled or unattempted) and separate disk
and config cells, empty when unchanged. Path-identified rows (no origin, an
unsupported origin, or an origin that is pending, stale or failed) and wildcard
patterns are blocked; the menu shows why. Immediately before its
write, each target is checked again. A target found as a checkout must still be
at its exact path with the same origin identity, and the configuration must
still declare the identity as it did when the action started, so a target shown
as a no-op is never written and a change that was already made is not reported
as done. Otherwise the target is blocked as stale (`stale: origin changed`,
`stale: checkout is gone or unreadable` or `stale: registration changed`,
followed by `after preview`, or `since the action started` for the current row
alone), nothing is written for it, and the other targets go on. Blocked and
failed targets are problems; a cancelled preview is not.
Repository actions… → Clone only and Clone + register run through the same
service as `add`, with the same targets, preview and outcomes as above. A clone
lands at the identity's configured destination. A matching checkout there is a
no-op; a conflicting, unreadable or unobserved destination blocks the target,
as do originless rows and patterns. A row whose checkout was found at another
path clones at the configured destination and leaves the found checkout alone;
the menu hint, Inspect and the preview name both paths, and such a row is
previewed even when nothing is marked. Clone only never writes configuration.
Clone + register registers after a successful or no-op clone; a failed
configuration write reports `write failed` in the config cell and keeps the
checkout. A post-clone hook declared for the identity runs after a fresh clone
and before the registration write; when it fails the target fails, the checkout
stays and Clone + register does not register it. A checkout found at the
destination never starts Git. Immediately before a
clone starts, the configured layout must still lead to the previewed
destination and a destination previewed as missing must still be missing;
otherwise the target is blocked (`stale: configured destination changed` or
`stale: destination is no longer missing`) and nothing is cloned over.
Remove only (`r`, or Repository actions… → Remove only) and Repository actions…
→ Remove + unregister delete exact checkouts through the same guards as
`remove`. They act on the marked rows, hidden ones included, or on the current
row, and always open a confirmation first. Opening it runs the guards once on
every target's exact found path, never on the configured destination of a
checkout found elsewhere, and fixes the targets, their identities and what the
guards answered. The confirmation lists each target as `ELIGIBLE` (`clean`),
`LOSS ACKNOWLEDGMENT REQUIRED` with its named local-state losses, or `BLOCKED`
with the guard that forbids it, which force cannot waive. A checkout found
at the configured destination of another identity's declaration is removed at
that found path, and the confirmation names the declaration, which stays.
Cancel is the default and changes nothing. A confirmation, like the batch
previews of the other actions and the Add preview, is answered only by keys
typed once it is on screen: input typed before it is drawn is discarded. A target's losses come one per line before its identity. When the
list does not fit, its last row reads `… N more lines` and names the key that
pages on; a page is the visible list. The hint row puts the keys that act before
the scroll keys, so a narrow popup cuts the scroll keys first. Targets with
losses stay unless the acknowledgment row is ticked (Space, or Enter on the
row); it covers exactly the listed losses at the listed paths. Until the list
has been paged to its end the acknowledgment does nothing and the status line
reads `Scroll through every loss before accepting`. Confirming runs the eligible targets one after another;
blocked targets and unacknowledged losses are reported as `BLOCKED` and the
others go on. Immediately before a deletion the target is checked again: the
configuration must still declare the identity as it did, the checkout must
still be at its exact path with the same origin, and every guard must pass
against the current configured root with exactly the losses that were listed.
The confirmation binds to the checkout that was inspected: its canonical path
and the device and inode of its directory and Git directory, read before and
after the Git guards and equal both times, must be the same,
so another checkout put at the path since, or reached through an ancestor that
became a symlink, is never deleted.
Any difference, including a loss that appeared after the box was ticked, blocks
that target as stale (`stale: registration changed`, `stale: origin changed`,
`stale: checkout is gone or unreadable`, `stale: selected clone path changed`,
`stale: checkout was replaced`, `stale: local state changed` or `stale:` with
the failing guard, followed by `after preview`), deletes nothing
and leaves its registration alone. Remove only never writes configuration.
Remove + unregister unregisters an identity once, with its last selected
target, and only when every selected checkout of that identity is gone from
disk; a selected checkout that was blocked, failed, stale or left unattempted
keeps the registration. A removed target whose identity is settled by a later
one reads `unregister waits for name` until that target has run, then
`unregistered with name` or `registration kept: …`; the last target of an
identity that could not unregister reads `registration kept: name was not
removed`. Remove + unregister needs a complete scan, because unregistering
changes every checkout of an identity: while a scan runs or is partial its
menu entry is unavailable with `the scan is not complete` and no confirmation
opens. Remove only is unaffected. The
confirmation names unselected checkouts of the same identity whose registration
the unregister changes; their files stay. A declared row whose destination is
missing can only be unregistered this way: its destination must still be the
configured one and still absent when the target runs (`stale: destination is no
longer missing` otherwise); Remove only is unavailable for it, as both actions
are for originless rows, patterns, and conflicting, unreadable or unobserved
destinations. A deletion whose configuration write then fails reports
`removed` in the disk cell and `write failed` in the config cell; the checkout
is not restored.
While a batch runs, the status line shows `Batch: n / m processed`, and
browsing, search and the menu stay available between two targets. A target is
never interrupted midway: keys typed while it runs are kept and handled in
order once it has finished, before the next target starts. Esc followed by a
character, Enter, Tab or Backspace while a target runs arrives from the
terminal as one Alt+key; for keys typed during a target the inventory reads it
as Esc and then that key. Two Esc in a row arrive as one. At any other time an
Alt+key matches no binding and does nothing.
Cancel active batch in the menu, or Cancel remaining work in the batch popup,
stops the batch before its next target: completed targets keep their outcomes,
the rest stay `UNATTEMPTED`, the inventory refreshes, and the cancellation is
not a problem. Quitting while a batch runs, with `q` in the batch popup
or a normal-mode quit key, pauses it and asks: Return
to inventory, the default, resumes it; Cancel remaining work and quit cancels
as above and exits with the run's status. Ctrl-C exits `130` at once.
While Git or a hook runs, the inventory leaves the alternate screen, restores
cooked mode and the cursor, and reads no input: Git and the hook keep the
terminal's streams for output, prompts and authentication, without a timeout.
Above Git's output Lager prints the target (`lager: clone 2 of 3 · identity →
destination`) and that Git owns the terminal. Keys typed before a child starts
are discarded and never answer its prompts; that holds for Git and for a hook.
The terminal stays handed over across consecutive clones. When no target in the
handoff failed the inventory returns at once. Otherwise the output stays on the
normal screen under one `lager: failed · name · reason` line per failure
(`registration failed` for a configuration write), and Enter returns to
the inventory; Ctrl-C exits `130` there too. On return the inventory discards
keys typed at the child and shows the batch outcomes; Git's own output stays in
the shell's scrollback. Ctrl-C during the handover interrupts the child and the
batch and exits `130` without returning to the inventory; targets already
completed keep their checkouts and registrations. Copy value puts the full
original Path, Origin, Identity or Branch on the clipboard through `pbcopy`
(macOS), `clip` (Windows) or `wl-copy`, `xclip` or `xsel`, trying the next one
when a utility fails, and gives up after 2 seconds in total; in Inspect, `c` opens the
same choices. Open origin hands an HTTPS address derived from the repository
identity to `open` (macOS) or `xdg-open` without waiting for it, never the
origin's own scheme; Windows has no opener. A missing or failing utility is
explained in the status line and is not a failure.
Add (`a`, or Add repository in the menu) takes one concrete reference through
the same parser as the CLI: shorthand, a canonical identity or a supported clone
URL. Its letters are text; Enter previews and Esc closes the form.
References with credentials, a query or a fragment are rejected without showing
the value again: the field is emptied and only the reason remains. Any `*`
rejects the reference as a wildcard; a wildcard or an unparsable reference keeps
its text in the field for editing. The preview
shows the identity, the configured destination, the checkout state observed there,
the registration and any declared post-clone hook, then offers Register only,
Clone only, Clone + register and Back to reference. Only a destination that does
not exist reads `missing`; a file, a symlink or a path that cannot be read reads
`unreadable`; a directory reads `cloned` or `conflict` once the scan has observed
a checkout there and `unknown` otherwise, either not observed yet or holding no
observed checkout after the scan. An unknown, unreadable or conflicting
destination disables both clone choices with the reason.
Each choice runs as a one-target batch with the same re-checks, terminal
handover and outcomes as the matching entry under Repository actions…, keeping
the typed reference text. The preview observes the destination once, when it
opens; a destination that appeared since then blocks the clone as stale.
Add never targets or changes marks, and closing it changes no files or
configuration and keeps the query, cursor and marks.
Problems… lists every failure that makes the inventory exit `1`; the header
shows their count.
Popup content scrolls independently of its title and effective action hints.
A popup shows its hints inside its frame, and the hint bar at the bottom of the
screen stays empty while it is open.
In Menu, Inspect, Help, the batch preview, the removal confirmation and the Add preview, q or Esc closes
the current popup (the Add preview returns to its reference);
Ctrl-Q quits the app. Normal-mode q and Esc quit.
Configure shortcuts through [inventory key maps](config.md#inventory-key-maps).
Register, Unregister, Clone only, Clone + register, Remove + unregister, Copy value,
Open origin and the three remote actions have no default key; a configured key runs the same entry as the
menu. Hints and Help show the effective keys, and Help names the actions without
one. A refresh with an invalid key map keeps the previous bindings and records a
problem.

Register, Unregister, Clone only and Clone + register, from the menu or from Add,
and Remove only and Remove + unregister are the repository mutations.
`--include-archived` without `--remote` exits `2` before raw mode.
Non-TTY use exits `2`; fatal startup/configuration errors exit `1`.

#### Sessions in the inventory

With a [`[session]` section](config.md#session) the inventory opens the session of a repository. A `●` after the mark column shows that the row's primary checkout has a live session. Lager asks Rex for its sessions in the background when the inventory starts, on every refresh and after an open that failed. Marks and the `s session` hint appear once Rex has answered.

`s` or Open session in the menu opens the session of the selected row, with the status `Opening session LABEL…`. The table keeps answering keys while Rex works. When the session is open, the inventory closes with the exit status of a normal quit and prints one line after the terminal is restored:

| Outcome | Line |
| --- | --- |
| A new session | `Opened session LABEL` |
| The session existed | `Switched to session LABEL` |

The inventory stays open in these cases:

| Case | Status line | Problem |
| --- | --- | --- |
| The row has no checkout, or Git cannot name its repository | `Open session unavailable: REASON`, such as `no checkout at the destination` | No |
| Another session is opening | `Open session unavailable: a session is opening` | No |
| A batch or a GitHub action is running | `Open session unavailable: a batch is running` or `a GitHub action is running` | No |
| Rex is missing or stopped | `Open session unavailable: REASON` | No |
| A call fails on a running Rex, or the session record cannot be read or written | `! Open session failed: REASON` | Yes |

When Rex is missing or stopped, its first answer turns the status line into `Sessions unavailable: REASON` until the next refresh, and the marks and the `s session` hint stay hidden. A missing Rex does not change the exit status. A list that fails on a running Rex, or a record file that cannot be read, is a problem as well.

An inventory that quits while a session opens restores the terminal, prints `lager: waiting for session LABEL to open…` on stderr and waits for Rex, so lager records the session. An open makes up to four Rex calls, each cut off after 10 seconds, and keys or signals do not end the wait. Then it prints the session line, or the status line of a failed open after `lager: `, and a failure on a running Rex makes it exit `1`. Without the section the inventory shows nothing about sessions and never calls Rex.

#### Sessions in the Worktrees popup

With a `[session]` section the Worktrees popup works with the session of each worktree. A `●` before the path marks a worktree with a live session, and the hint row adds `s session`. Both appear once Rex has answered, as in the table.

`s` opens the session of the selected worktree. Its status line, its exit lines and the cases that keep the inventory open are those of `s` in the table. A worktree whose directory is gone answers `Open session unavailable: no checkout at the destination`.

The plan of a new worktree shows its session label in a `session` field and the line `[x] Open session`. After Git has created the worktree, lager opens its session with the status `Opening session LABEL…`. The inventory then closes and prints two lines:

```text
Created worktree ~/repos/org/core.worktrees/fix on new branch fix
Opened session core · Fix
```

When a session for that path is still running, the second line reads `Switched to session LABEL`.

When that open cannot run, the worktree stays created and the inventory stays open. The status line starts with the worktree line, which reads `on branch BRANCH` for a branch that existed:

| Case | Status line | Problem |
| --- | --- | --- |
| Another session is opening | `Created worktree PATH on new branch BRANCH · Open session unavailable: a session is opening` | No |
| Rex is missing or stopped | `Created worktree PATH on new branch BRANCH · Open session unavailable: REASON` | No |
| A call fails on a running Rex, or the session record cannot be read or written | `! Created worktree PATH on new branch BRANCH · Open session failed: REASON` | Yes |

For a worktree with a live session, the removal summary shows `session  LABEL · open` and the line `[x] Close session`, noted `ends its programs`. Once Git has removed the worktree, lager closes the session while Git still holds the terminal. Back in the popup, the status line reads `Removed worktree PATH · branch BRANCH kept · Closed session LABEL`. A worktree without a live session gets neither line.

`s` in the plan or the summary switches the line to `[ ] Open session` or `[ ] Close session` for this one worktree, and lager leaves the session alone. While Rex is missing or stopped, the plan shows no session line.

When the close fails, the worktree stays removed:

| Case | Status line | Problem |
| --- | --- | --- |
| Rex is missing or stopped | `Removed worktree PATH · branch BRANCH kept · Close session unavailable: REASON` | No |
| A call fails on a running Rex, or the session record cannot be read or written | `! Removed worktree PATH · branch BRANCH kept · Close session failed: REASON` | Yes, listed as `Close session failed: REASON` |

A blocked removal keeps the session open. Its blocker view shows `session  LABEL · open` beside the blockers. A removal refused after the summary, because the worktree changed or would lose more than listed, closes nothing either, and its status line ends in `· the session stays open` when the close was ticked.

Closing the session the inventory runs in ends the inventory too. Lager saves its record before it asks Rex to close the session, so the record stays correct.

#### Remote discovery

Remote discovery adds what the configured providers list to the table. It reads
from providers and changes nothing: no registration, no clone, no hook.
`tui --remote` starts with it on, and "Remote discovery…" in the menu turns it
on and off. Opening Add turns it on when a provider is configured: the providers
load while the reference is typed, and "Browse remote candidates" closes the
form to show them. It stays on until it is disabled; cancelling Add leaves it
on.

Each provider is asked for its whole catalog and for the members of the wildcard
declarations on its host. The rows are merged as `status --remote` merges them:
one row per identity, every local checkout of an identity kept, exclusions and
explicit declarations first, archived repositories only with the archive
preference. Once every provider answered, each row of `status --remote` is on
the screen with the same cells. The screen adds the catalog's other repositories
as candidates: `unregistered`, or `excluded` where a wildcard excludes them, with
the checkout `remote only` while nothing is at their destination. A candidate
that shares its short name with another row lengthens both names, as any two
rows do.

Providers load in the background, one request each. Local rows, navigation,
search and marks work meanwhile, and the rows of a provider appear when it
answered. The source line under the header names every provider with its state:
`loading… N s`, `ready, N candidates`, `failed`, or `refreshing, N cached
(stale)` while rows of an earlier answer are still shown. A failed provider
leaves the other providers' rows in place. Its error is the status line's state
and a problem, so the inventory exits `1`. When only the catalog or one
wildcard of a provider fails, the provider reads `failed` and keeps the rows the
rest of its answer returned. A request without an answer after 30 seconds is
ended and fails, also while Git or a hook holds the terminal; the failure shows
once the inventory is back. Quitting waits at most 2 seconds for running
requests to end. Neither bound applies to Git or hooks that run on the terminal.

The Remote discovery menu lists the providers and these entries:

| Entry | Effect |
| --- | --- |
| Enable remote | Fetches every configured provider. |
| Retry failed providers | Fetches the failed providers again and no other. |
| Refresh all enabled providers | Fetches every provider again; the cached rows stay until the answers arrive. |
| Include archived | Switches the archive preference. While remote discovery is on, the running requests are dropped and every provider is fetched again. |
| Disable remote | Drops the running requests, hides the rows the providers added and removes their marks, naming them in the status line. The answers of this run and the archive preference stay for the next enable. |

An answer to a request that was dropped by disabling, a refresh or the archive
preference is ignored. `R` reloads the configuration and rescans the disk; it
merges the cached answers into the new rows and asks no provider. A provider
whose settings or wildcards changed reads `N cached (stale), config changed`
until remote discovery fetches again. A configuration saved during a scan is
reloaded once per saved revision as soon as no dialog, menu or input is open
(`Config changed on disk · reloaded`). A save that does not load keeps the
previous configuration with `R retries · config changed on disk but did not load:
<reason> · previous configuration stays`; a scan in progress completes with the
previous configuration, and the next save is tried again.

On Unix, Ctrl-C or SIGINT interrupts binary inventory with exit `130`, SIGTERM
with `143` and SIGHUP with `129`. Inventory restores the terminal first, then
gives its background work one shared 2-second budget to end; when that wait
starts, stderr shows `lager: waiting up to 2 s for background work to stop…`. This signal policy is scoped to the binary inventory invocation;
`lager::cli::run()` retains the embedding process's signal policy.

Use the [isolated inventory walkthrough](../how-to/verify-inventory.md) for
acceptance testing without your normal repositories or configuration.

## Worktrees

`lager worktree` works on one repository at a time through native Git. The commands need Git 2.36 or later. With an older Git they exit with Git's own error and create or remove nothing. Worktrees are not declarations: the configuration file does not name them and `ensure` does not create them. The commands run without a configuration file as long as the repository is given as a path.

### Naming the repository

`REPOSITORY` is a checkout path or a repository reference.

- An argument that is absolute, or that is `.` or `..` or starts with `./` or `../`, is a path. It has to be the root of a non-bare checkout.
- Anything else is a repository reference, such as `org/project`, and resolves to the checkout the configuration assigns to it.
- A linked worktree resolves to its primary checkout.

`worktree list` and `worktree branches` without `REPOSITORY` use the repository of the working directory, from any directory inside it. Outside a repository they exit `2`. `worktree add` and `worktree remove` always take their target as an argument.

### `worktree list`

The human listing has one line per working tree with three columns, two spaces apart: the path, the branch, and the state. A detached HEAD reads `detached` followed by the short commit ID. The state column holds `primary`, `locked` and `prunable`, the last two with Git's reason when there is one. Worktrees placed outside the default location and worktrees created by other tools are listed too. With `--json` the command prints one document; see [worktree JSON](worktree-json.md).

With a [`[session]` section](config.md#session), a fourth column shows the label of each working tree's live session and stays empty for one without a session. The command asks Rex once. When Rex cannot answer, the column reads `unknown` on every row, one line on stderr gives the reason, and the exit status stays what the listing makes it:

```text
$ lager worktree list org/project
/home/dev/repos/org/project                   main    primary  project
/home/dev/repos/org/project.worktrees/hotfix  hotfix           project · Hotfix
```

The stderr line reads `lager: warning: REASON`, where `REASON` is one of the session errors in [Outcomes](#outcomes) without its `error:`, such as `session host unavailable: …`.

A failure after the repository is found is one line on stderr and exit `1`, with the readable rows still printed.

### `worktree branches`

The command prints what the Local and Remote tabs of the Worktrees popup show, newest commit first. It reads the branches Git already has and never touches the network; [`worktree add --fetch`](#worktree-add) and the popup's `f` bring remote branches up to date.

Without `--remote` it lists the local branches in four columns under a heading, two spaces apart. They are the branch, the short commit ID, the age of the commit, and where the branch is checked out. The last column holds `primary` for the primary checkout, the path of a linked worktree, or nothing for a branch no worktree has.

With `--remote` it lists the remote-tracking branches, such as `origin/feat/x`, in the same first three columns. The fourth column, headed `local`, names the local branch of the same name and its distance from the remote branch. The distance reads `same commit`, `N ahead`, `N behind` or `N ahead, M behind`, counting commits only on the local branch first. It reads `distance unknown` when Git could not count them. The column ends with `primary` or `in PATH` when a worktree has that local branch checked out. It stays empty for a remote branch without a local one. A remote's `HEAD` is not listed. Without remotes the list is empty and the exit status is `0`.

```text
$ lager worktree branches org/project
branch    commit   age  checked out
main      9f2c1ab  now  primary
feat/new  4d0e7a1  3d
feat/mid  b81c2f0  2w   /home/dev/repos/org/project.worktrees/mid

$ lager worktree branches org/project --remote
branch           commit   age  local
origin/main      9f2c1ab  now  main · same commit · primary
origin/feat/new  71aa0c3  3d   feat/new · 1 ahead
origin/feat/mid  e5d9b02  2w   feat/mid · 1 behind · in /home/dev/repos/org/project.worktrees/mid
```

A list with no rows prints nothing. With `--json` the command prints one document; see [worktree JSON](worktree-json.md#worktree-branches---json). A failed Git call after the repository is found is one line on stderr and exit `1`; the document then holds the failure in `errors`.

### `worktree add`

| Options | Branch | Starts at |
| --- | --- | --- |
| none | A new branch named `NAME` | The primary checkout's HEAD, also when the command runs in a linked worktree |
| `--new-branch NEW` | A new branch named `NEW` | The primary checkout's HEAD |
| `--branch EXISTING`, a local branch | The existing local branch | Where the branch is |
| `--branch REMOTE/EXISTING`, a remote-tracking branch without a local branch of that name | A new local branch `EXISTING` that tracks `REMOTE/EXISTING` | `REMOTE/EXISTING` |
| `--start REF` with a new branch | The new branch | The commit `REF` names in the local repository |

`--branch` looks for the local branch of that name first, then for the remote-tracking branch, such as `origin/feat/x`, the way `git worktree add` guesses. With `--branch`, `NAME` is optional and defaults to the last `/` segment of the local branch name: `--branch origin/feat/session-tabs` creates the worktree `session-tabs`. A `NAME` that is given always wins. When the last segment is not a valid `NAME`, the command exits `2` with `branch BRANCH gives no valid NAME; pass NAME`.

`--start` applies only to a new branch, and `--branch` conflicts with `--new-branch`; both mistakes exit `2`. `NAME` is required without `--branch`. A new branch whose name exists, a `--branch` that names no local or remote-tracking branch, a branch that is checked out in another worktree, and a `--start` that names no local commit exit `1`. The checked-out refusal reads `BRANCH is checked out in PATH`, names the primary checkout or the linked worktree that has the branch, and comes before anything is created. Lager never moves an existing branch.

`worktree add` never touches the network unless `--fetch` is given, so without it a remote branch has to be known to the repository already. With `--fetch`, Lager runs `git fetch REMOTE` once for each remote, in the order `git remote` lists them, before it looks for the branch. It passes no `--prune`, so Git's `fetch.prune` decides. A remote that fails prints one line on stderr and the others still run:

```text
lager: warning: fetch REMOTE failed: REASON
```

`REASON` is the first line of Git's message. The command goes on when the branch is found afterwards, and otherwise exits `1` with `no local or remote branch named BRANCH`. A repository without remotes fetches nothing. `--fetch` without `--branch` fetches and then creates the worktree as usual. Git's fetch output goes to stderr, also under `--json`.

`NAME` is one path component: ASCII letters, digits, `.`, `_` and `-`, starting with a letter or digit. Git checks the branch name separately.

The destination is the first of these that applies:

1. `--path PATH`, absolute or relative to the working directory. `..` is resolved by name. The path can lie outside the repository root and is never written to the configuration.
2. With [`worktree_path`](config.md#worktree-path) set: the template with its placeholders expanded. A template with `{relative}` needs `--path` for a primary checkout outside `root`.
3. `<primary>.worktrees/<NAME>`, beside the primary checkout.

Every destination has to be a path where nothing exists, an empty directory included. Lager refuses a destination below a symlink, inside a checkout, inside a Git directory, or still registered as a worktree whose directory is gone. On macOS `/tmp` and `/var` are symlinks, so an explicit path there is written as `/private/tmp/...`. Missing parent directories are created after every check has passed.

Git's own output appears on its own streams, followed by one line from Lager:

```text
$ lager worktree add org/project hotfix
Preparing worktree (new branch 'hotfix')
HEAD is now at 4be1d02 Release 2.3.1
Created worktree /home/dev/repos/org/project.worktrees/hotfix on new branch hotfix
```

The line says how the branch relates to its remote-tracking branch, when there is one:

| Branch | Line ends with |
| --- | --- |
| A new local branch that tracks a remote branch | `on new branch feat/x, tracking origin/feat/x` |
| An existing branch that is behind | `on branch feat/x, 2 behind origin/feat/x` |
| An existing branch that is ahead | `on branch feat/x, 1 ahead of origin/feat/x` |
| An existing branch with both | `on branch feat/x, 1 ahead, 2 behind origin/feat/x` |
| An existing branch at the same commit | `on branch feat/x, up to date with origin/feat/x` |
| A new branch, or an existing branch without a remote-tracking branch | `on new branch hotfix`, `on branch feat/x` |

The remote-tracking branch of an existing local branch is the upstream it is configured with, or else the branch of the same name on the first remote that has one. Lager reads the remote-tracking refs as they are.

A new worktree holds what Git checks out. Lager copies no environment files, dependencies or ignored files into it and runs none of its own hooks for it. Git runs the repository's `post-checkout` hook as it does for any `git worktree add`.

When Git fails, Lager rolls nothing back. It exits `1` and names what is left, one `remaining:` line each for a directory, a new branch and worktree metadata, or `remaining: nothing`.

`--json` prints one document about the new worktree on stdout and sends Git's output to stderr; see [worktree JSON](worktree-json.md#worktree-add---json).

#### Sessions on `add`

`--open-session` opens the new worktree's [session](#sessions) and shows it in Rex. `--create-session` creates the session and leaves Rex where it is, which suits agents and scripts. The two flags conflict. Without either one, `add` calls no Rex.

The session belongs to the worktree's canonical path and carries its label, such as `project · Hotfix`. A ticket key at the start of the worktree's name moves to the end: `CP-244-sign-in-page` gives `project · Sign in page · CP-244`. A key is capital letters or digits that start with a letter, a `-` and digits, followed by `-`, `_` or the end of the name. After the `Created worktree` line comes one more:

| Flag and outcome | Line |
| --- | --- |
| `--open-session` | `Opened session LABEL` |
| `--create-session`, a new session | `Created session LABEL` |
| `--create-session`, the session exists | `Session LABEL already exists` |

A session exists already when an earlier worktree at the same path left its session running. With `--json` the lines are left out and the document's `session` holds the session.

A session flag without a `[session]` section exits `1` with `error: sessions are not configured; add [session] to CONFIG` before lager creates anything. When the worktree is created and the session fails, the worktree stays, the command prints `error: worktree created, session not opened: REASON` and exits `1`. `REASON` is the session error without its `error:`, such as `session host unavailable: …`; [Outcomes](#outcomes) lists them. With `--json`, stdout stays empty then.

### `worktree remove`

`PATH` is the exact path of a linked worktree, absolute or relative to the working directory. A branch name does not select a worktree. Removal goes through `git worktree remove`; Lager deletes no directory itself and never prunes or repairs. The branch always stays.

Lager prints what it is about to remove, then asks:

```text
$ lager worktree remove /home/dev/repos/org/project.worktrees/hotfix
Remove worktree
  path      /home/dev/repos/org/project.worktrees/hotfix
  branch    hotfix · kept · 2 commits not pushed
  losses    none
```

The confirmation defaults to no. Declining prints `Skipped PATH` and exits `0`; Ctrl-C exits `130`. `--yes` skips the question. Commits on the branch that no remote holds are counted in the summary and never block the removal.

`losses` lists modified files, untracked files and ignored files on one line, then each named state of the worktree on a line of its own. A tracked file marked skip-worktree or assume-unchanged counts as modified whether or not it was edited, because Git's status hides it. A repository deeper inside an ignored directory is part of the ignored files. The Git directory of a linked worktree is read like the one of a checkout in [What `remove` proves](#what-remove-proves): a stopped rebase, `git am`, merge, cherry-pick, revert or bisect is named with the commands that finish or drop it, as `git -C PATH ... --continue or --abort` (the popup, which shows the path, reads `in the worktree: git ...`), and an entry Git does not keep there in normal use is named as `unknown entries in GITDIR: NAME`, with the absolute path of `.git/worktrees/NAME`. Each ref only this worktree sees (`refs/worktree/*`, `refs/bisect/*`, `refs/rewritten/*`) is named as `per-worktree ref REF`, since it goes with the worktree. With any of them the command exits `1` unless `--force` is given, with `error: local changes would be lost; rerun with --force to accept these losses` when the losses are files only and `error: the listed state would be lost; ...` otherwise, and the question reads "Accept these losses and remove this worktree?". Git receives `--force` only in that case.

The Git directory of a linked worktree holds without a warning `HEAD`, `index`, `logs`, `refs`, `reftable`, `info`, `config.worktree`, `commondir`, `gitdir`, `locked`, `FETCH_HEAD`, `ORIG_HEAD`, `COMMIT_EDITMSG`, `REBASE_HEAD`, `AUTO_MERGE`, `gc.log`, `fsmonitor--daemon`, any `.ipc` socket, `CLAUDE_BASE`, `opencode` and `modules`, and an entry that is an empty file or directory is not named either.

These block the removal with or without `--force`. Each prints `error: blocked: ...` and, where Git has a command for it, a `hint:` line:

| Blocker | Hint |
| --- | --- |
| The path is the primary checkout | `lager remove IDENTITY` |
| The path is not the root of a Git worktree, or is a symlink | none |
| The path and the Git directory its `.git` file names do not name each other | none; the message names that Git directory |
| The worktree is locked | `git worktree unlock` |
| The path is absent | `git worktree prune` |
| HEAD is detached, or names a ref outside `refs/heads/`, at a commit that no branch, tag or remote ref holds | `git branch keep-NAME` in the worktree |
| A submodule is populated | `git submodule deinit` |
| Git keeps submodule repositories for the worktree in its Git directory (`modules`) | none |
| A stopped rebase or merge, or an interrupted checkout, holds an autostash in the worktree's Git directory (`rebase-merge/autostash`, `rebase-apply/autostash`, `MERGE_AUTOSTASH` or `CHECKOUT_AUTOSTASH_HEAD`) | `git rebase --abort` or `git merge --abort` in the worktree, which puts the stashed changes back; for a checkout, `git stash apply` with the commit the file names, then deleting the file |
| An untracked directory, or an ignored directory itself, is a Git repository of its own, or a directory with tracked or untracked files holds a `.git` entry | none; the message names the directory |
| Submodule state, local state or HEAD cannot be read | none |

A detached commit counts as held when it is an ancestor of a commit under `refs/heads/`, `refs/tags/` (peeled to a commit) or `refs/remotes/`. HEAD, reflogs, the stash, per-worktree refs and replacement refs do not count. Only a ref under `refs/heads/` is a branch that stays: a HEAD that names any other ref is checked like a detached one, and the summary reads `detached` with the short commit ID. A submodule that `.gitmodules` declares and that holds no repository in the worktree does not block while its directory is empty. Git reports nothing inside a submodule path, so any entry in that directory blocks as unreadable submodule state; the message names the path.

For an absent path Lager names the owning repository when the command runs inside it. No remote origin is needed for any of this.

Lager checks again immediately before it calls Git. If the worktree differs from what the summary showed (its losses, its HEAD, its lock, its ownership), the command exits `1` and removes nothing.

#### Sessions on `remove`

`--close-session` closes the worktree's [session](#sessions) once Git has removed the worktree. A refused, blocked or declined removal leaves the session running. After the `Removed worktree` line comes `Closed session LABEL`, or `No session for PATH` when the worktree has no live session. Closing a session ends every program in its panes.

A `--close-session` without a `[session]` section exits `1` with `error: sessions are not configured; add [session] to CONFIG` before lager inspects or removes anything. When the worktree is removed and closing fails, the command prints `error: worktree removed, session not closed: REASON` and exits `1`; the session keeps running and stays in `lager session list`. `lager session close PATH` closes it by the worktree's path.

The worktree commands print `error:` and `hint:` lines, the session commands print `error:` lines, and the other commands prefix diagnostics with `lager:`.

## Sessions

`lager session` opens a checkout as a terminal session in Rex with the tabs and panes of the configured layout. Every pane starts in the checkout. The commands need a [`[session]` section](config.md#session) in the configuration; without one they exit `1` with `error: sessions are not configured; add [session] to CONFIG` and call no Rex. Only `session open`, `--open-session`, `s` in the inventory or the Worktrees popup, and a worktree created from the popup with `[x] Open session` change what Rex shows. Every other call, `session create` and `--create-session` included, leaves Rex's view alone. The [sessions how-to](../how-to/sessions.md) walks through the setup.

Lager keeps one session per checkout and finds it again through a record on this machine, never by its label. A session made by hand, or by another tool, is unknown to lager. Every command first asks Rex which sessions are live and forgets the records of the others, so a session closed in Rex is created again on the next `open` or `create`.

### Naming the checkout

`REPOSITORY` is a checkout path or a repository reference:

- A path (absolute, `.`, `..`, or starting with `./` or `../`) names the working tree it lies in, from any directory inside it. Inside a linked worktree that is the worktree.
- A reference such as `org/project` names the primary checkout the configuration assigns to it.

`NAME` picks a linked worktree of that repository by its directory name. No match, or more than one, exits `1` and lists the linked worktrees, or the matches, one indented path per line, followed by `error: PATH: ERROR` for each worktree entry Git could not read:

```text
error: no linked worktree of /home/dev/repos/acme/core is named spike
  /home/dev/repos/acme/core.worktrees/review
```

```text
error: several linked worktrees are named spike
  /home/dev/one/spike
  /home/dev/two/spike
```

`open` and `create` need the checkout's directory and exit `1` with `error: not a Git checkout: PATH` without it; for a reference, `PATH` is the destination under the root. `close` also reaches a worktree whose directory is gone. While Git still lists the worktree, `REPOSITORY NAME` names it. A worktree removed with `git worktree remove`, or by `lager worktree remove` without `--close-session`, leaves its session running and in `lager session list`. Git does not list that worktree, so `REPOSITORY NAME` cannot name it; `lager session close PATH` closes the session recorded under its path. With no session recorded there, `close` exits `1` with `error: not a Git checkout: PATH`.

### Picking the checkout

`open` and `close` without `REPOSITORY` let you pick the checkout in `fzf`. Each row shows a label and a path. `open` offers every checkout under the root and every worktree of their repositories, and a trailing ` ●` marks a checkout with a live session. `close` offers only the checkouts with a live session. Lager then acts as if the picked checkout's path were given as `REPOSITORY`.

A pick acts on one checkout. Marking more than one exits `2` with `error: session open takes one checkout; N were picked`. Esc or Ctrl-C in `fzf` exits `130` and changes nothing. With no rows to offer, `fzf` does not start and the command exits `0`. Outside a terminal there is no picker: the command exits `2` with `error: session open requires a repository outside a terminal` and starts no `fzf`. For `close`, both lines say `session close`. Without a `[session]` section the command fails as above before any of this.

### Labels

A session's label is the primary checkout's directory name, such as `core`. A linked worktree's label adds its directory name in words: `-` and `_` become spaces and the first letter is upper-cased, so `dev_spike-one` gives `core · Dev spike one`. Control characters are dropped, the label is cut to at most 128 bytes on a character boundary, and surrounding whitespace is trimmed. For an existing session lager prints the label it recorded at creation, also when the label was edited in Rex since.

### Outcomes

| Command and outcome | Output | Exit |
| --- | --- | --- |
| `create`, a new session | `Created session LABEL` | `0` |
| `create`, the session exists | `Session LABEL already exists` | `0` |
| `open`, new or existing | `Opened session LABEL` | `0` |
| `close` | `Closed session LABEL` | `0` |
| `close PATH` of a removed worktree with a recorded session | `Closed session LABEL` | `0` |
| `close`, no live session | `No session for PATH` | `0` |
| No `[session]` section, or no configuration file | `error: sessions are not configured; add [session] to CONFIG` | `1` |
| Rex is not installed or not running | `error: session host unavailable: REASON` | `1` |
| A Rex call fails on a running Rex | `error: session host failed: REASON` | `1` |
| The checkout is missing | `error: not a Git checkout: PATH` | `1` |
| The record cannot be locked, read or written | `error: could not lock sessions: …`, `error: could not read sessions PATH: …` or `error: could not write sessions PATH: …` | `1` |
| `open` or `close` without `REPOSITORY` outside a terminal | `error: session open requires a repository outside a terminal` | `2` |
| More than one checkout picked | `error: session open takes one checkout; N were picked` | `2` |
| The picker is cancelled | nothing | `130` |
| `fzf` is missing or fails, or the `open` picker cannot resolve or walk the root | `error: REASON`, such as `error: fzf is required for interactive repository selection; install fzf and try again` | `1` |
| Invalid arguments | clap's usage error | `2` |

Success lines go to stdout, error lines to stderr. `PATH` is the canonical checkout path. `REASON` is Rex's own first line, such as `no Rex server is running at SOCKET, and autostart is disabled`, or one of lager's: `rex is not installed`, `no Rex app connected`, `rex did not answer within 10 s`, `rex listed no session ids`.

Every command that gets past its arguments asks Rex first, `list` and a `close` that finds nothing included, so with Rex unavailable each one exits `1`. An `open` or `close` without `REPOSITORY` outside a terminal fails before that and calls no Rex. Lager never starts the Rex server. `open` creates the session before it asks Rex to show it, so when showing fails the session stays and `list` shows it.

`close` forgets the session, then asks Rex to end it. When Rex fails, the record comes back. Closing a session ends every program in its panes. Run from inside the session it closes, the command ends with the session, so its `Closed session` line is not seen; the session is closed and forgotten all the same.

`create --json` prints one document with the session and its panes by name, and `list --json` prints every recorded session; see [session JSON](session-json.md).

### `session list`

The human listing has one line per checkout with a live session, sorted by path, with three columns two spaces apart: the label, the path and Rex's session id. It has no header, and with no sessions it prints nothing.

```text
$ lager session list
core              /home/dev/repos/acme/core                      session:01a1f8aa-76be-721e-8f28-6e3ba611fe57
core · Dev spike  /home/dev/repos/acme/core.worktrees/dev-spike  session:01a1f8ab-8ffc-76ae-836e-30d02892582e
```

## Agent skill

`lager skill` writes the usage guide for coding agents to stdout. The guide is embedded in the binary, so it describes the commands of the installed version. It reads no configuration and touches no file.

`lager skill install` writes `SKILL.md` and `references/setup.md` to `$HOME/.agents/skills/lager`, creating the directories and replacing existing files, then prints `Installed skill to PATH`. `SKILL.md` tells the agent to run `lager skill`; `references/setup.md` covers installing lager for the case that the command is missing; configuration is part of the guide. The command fails with exit `1` when `HOME` is unset or a file cannot be written.

`npx skills add nikbrunner/lager --skill lager` installs the same two files from GitHub.

## Non-interactive use

Outside a terminal, Lager never prompts:

- `init` requires `--root`, one root-creation choice, and one GitHub choice.
- `add` requires exactly one of `--register` or `--no-register`, unless `--post-clone` supplies registration.
- `remove` requires exactly one of `--unregister` or `--keep-registered`, plus `--yes`, and `--force` as well when the checkout has warnings.
- `tui` requires TTY stdin and stdout; use `status --json` for automation.
- `worktree remove` requires `--yes`, and `--force` as well when the worktree has local losses.

`--post-clone` conflicts with `--no-register`. `status --include-archived` and `tui --include-archived` require `--remote`. Explicit repository arguments bypass the picker.

## Output and exit status

Flag-driven and JSON output contain no interactive styling. `status --json` writes one stable JSON document to stdout; warnings and problems use stderr. Git and hook processes retain their native streams, except under `worktree add --json`, where their stdout goes to stderr. A selected repository without a post-clone hook reports that on stderr.

Lager-owned human fields escape C0, DEL, and C1 controls: newline, carriage
return, and tab appear as `\n`, `\r`, and `\t`; other controls appear as
`\x00` through `\x9f`. Literal backslashes appear as `\\`, so a literal
`\x1b` cannot impersonate an escaped ESC. Status table columns and picker columns
are separated by the renderer; field values cannot inject extra rows or columns.
This applies to diagnostics, progress messages, prompt labels/default hints, and
picker labels. It does not change references, paths, hook commands, prompt
answers, JSON field values, or native Git/hook streams.

Removal picker labels are rendered from identity and exact path, not parsed from
display text. The Rust `Fzf` adapter treats `SelectionCandidate.display` as an
opaque escaped field when `exact_path` is absent; when present, it renders the
identity and path instead. Duplicate labels map back to distinct untouched
candidates by hidden row ID, in the order returned by the picker.

A picker or repository interaction cancellation exits 130. Inventory's `q` and Esc use the exit status described above. Invalid usage exits 2. Configuration, provider, Git, hook, filesystem, and aggregate failures exit 1. Success, no-op, empty selections, and explicitly skipped removals exit 0.

See [status JSON](status-json.md), [worktree JSON](worktree-json.md), [session JSON](session-json.md), [exit codes](exit-codes.md), [automation](../how-to/automation.md), and [safe removal](../how-to/remove-safely.md) for the detailed contracts.
