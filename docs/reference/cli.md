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
| `add` | `add [REPOSITORY]... [--register\|--no-register] [--post-clone CMD] [--include-archived]` | Creates local checkouts. `--post-clone` implies `--register`. |
| `remove` | `remove [REPOSITORY]... [--unregister\|--keep-registered] [--yes] [--force]` | Permanently deletes validated local checkouts. `--force` accepts local-state warnings only. |
| `ensure` | `ensure [--include-archived]` | Creates missing effective declarations sequentially. |
| `hook` | `hook [REPOSITORY]...` | Runs hooks for explicit declarations. Without repositories, it selects explicit declarations. |
| `status`, `list`, `ls` | `status [--remote] [--include-archived] [--registered] [--json]` | Prints the rows `tui` shows, once its scan has finished: a table, or one JSON document with `--json`. `--remote` expands wildcards through providers; `tui` with remote discovery on shows those rows and adds the providers' unregistered and excluded repositories as candidates. `--registered` keeps declared repositories and wildcard patterns. |
| `tui` | `tui [--remote] [--include-archived]` | Opens the inventory with its repository actions; requires TTY stdin and stdout. `--remote` starts with remote discovery on. |
| `worktree list` | `worktree list [REPOSITORY] [--json]` | Lists the primary checkout and every linked worktree of one repository. |
| `worktree add` | `worktree add <REPOSITORY> <NAME> [--branch EXISTING \| --new-branch NEW] [--start REF] [--path PATH]` | Creates a linked worktree through `git worktree add`. |
| `worktree remove` | `worktree remove <PATH> [--yes] [--force]` | Removes one linked worktree through `git worktree remove` and keeps its branch. |

`list` and `ls` are visible aliases of `status`. All three names accept the same arguments, options, and configuration selection, and produce identical output and exit statuses.

`status` needs no terminal. It scans the root the way `tui` does, through Git with optional locks off, and writes nothing to the configuration or to any repository. The table has the columns of the wide inventory grid (repository, path, registration, checkout, origin, branch, changes) with the same cell text, aligned with two spaces, never truncated. A repository the provider reports as archived reads `NAME [archived]` in the repository column. An empty inventory prints the header only. A root that does not exist yet, or a symlink to nothing, is an empty, complete scan: the declared rows read `missing`, stderr gets one note such as `lager: ~/repos: root does not exist yet`, and the exit status is `0`. A root that is a plain file, or a symlink to one, is a problem. Each problem (an unreadable path, a root that is not a directory, a failed Git observation, a failed provider) is one line on stderr, and the rows are still printed. See [status JSON](status-json.md) for the document.

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
| `register`, `unregister`, `add`, or `hook` without repositories | Uses `fzf --multi` to select candidates. |
| `remove` without repositories | Uses `fzf` to select local checkouts that have an `origin`; guards run after selection. |
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

Two inspections of the same checkout give the same warnings, word for word. The inventory lists them as the losses of a target.

The proof covers what the remote-tracking refs held at the last fetch; Lager does not contact the remote, so a branch deleted there after the last fetch still counts as pushed. Commits that only a reflog reaches, the names of local branches and tags whose commits a remote holds, local configuration, and the files inside `info/`, `logs/`, `hooks/` and `lfs/` (custom hooks, notes in `info/`, LFS objects never uploaded) go with the checkout unnamed: of the Git directory only the top-level entries are read.

## Offline inventory

`tui` shows declarations and local
Git roots beneath the configured root without provider requests or configuration
writes. Registration and checkout state are independent; duplicate origins keep
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
failures are also listed in Session problems. An inventory with no rows and
nothing declared says so in the table, names the root, and offers `a` to add a
repository; its status line reads `Scan complete · 0 repositories · 0
declarations` and its footer shows only the keys that act without a row. A root
that does not exist yet is shown the same way, with `~/repos does not exist
yet` in the table, or as a status line when declarations give the table rows;
it is not a session problem.

Press `R` to reload configuration and refresh local observations. Earlier
observations remain marked stale until refreshed. Press `q` or Esc to cancel
background work and quit, restoring the terminal. A normal quit returns `1` if
an operational failure occurred during the session, even after a successful
refresh; otherwise it returns `0`. Informational exclusions and deliberate
cancellation do not count as failures.

Use `j`/`k` or arrow keys to select a row, `/` for live fuzzy search, Space to
mark the row, `a` to add a repository by reference, `r` to remove the checkout and keep its registration, `w` for the worktrees of the selected repository, Enter to inspect the selected repository, `m` for the action menu,
and `?` for effective bindings. Search matches full attributes, paths and origins; Enter/Esc
retain the filter and normal-mode Backspace clears it. Ordinary search letters
remain text; F1 opens search help. Help describes the mode it was opened from.
Marks survive filtering and refresh while their exact target remains; the search
row shows the marked and hidden-marked counts, and the status line reports marks
removed with a vanished entry. Wildcard pattern rows cannot be marked.
The menu groups actions for this row (Repository actions…, Inspect, Copy value…,
Open origin, Worktrees…), the selection (mark this row, mark visible concrete
rows, clear all marks) and the session (Add, Remote discovery…, Refresh, Session
problems…, batch cancellation, Help and Quit).
`w` or Worktrees… opens the worktrees of the current row's checkout; marks are
not used. The popup lists the primary checkout and every linked worktree Git
knows, wherever it sits, with its path, branch and state: `primary`, `locked`
and `prunable` with Git's reason, and `clean` or the counts of modified and
untracked files. `a` asks for one name with the form of `NAME` in
[`worktree add`](#worktree-add). The popup and the command treat a name whose
local branch exists differently. The popup checks that branch out, which is
`worktree add REPOSITORY NAME --branch NAME`, and its plan marks the branch
`existing`. The command without `--branch` refuses with
`branch already exists`. For any other name both create a new branch of that
name at the primary checkout's HEAD. `--new-branch`, `--start` and `--path`
belong to the command only. Enter shows the planned path and branch; nothing is
created before "Create worktree" is chosen there. `r` runs the guards of [`worktree remove`](#worktree-remove) on
the selected worktree. A blocker is shown with its hint and no confirmation.
Otherwise a summary of losses, path and kept branch asks with Cancel as the
default; choosing removal accepts exactly the listed losses, and a worktree that
changed since the summary is left alone. The choices stay at the bottom of the
popup. In a terminal too small to show the losses, the full path, the kept
branch and both choices, or the plan and both choices, the popup says so and the changing choice does nothing until
the terminal is enlarged. The blocker view scrolls. Keys typed before a plan or summary is
on screen do not answer it. `r` on the primary checkout reports that it is
removed from the table. `c` copies the selected path. While Git runs, the
inventory hands it the terminal as for a clone; Ctrl-C there exits `130`, and
after a failure Enter returns to the popup. When Git fails a removal partway,
`remaining:` lines name what is left: the directory, the worktree metadata, or
nothing. A row without a checkout (missing,
remote only, wildcard pattern, unreadable) reports `Worktrees unavailable: …`
and opens nothing.
Repository actions… → Register and Unregister act on the marked rows, hidden
ones included, or on the current row when nothing is marked. They change the
configuration only, through the same registry services as `register` and
`unregister`: Register keeps a checkout's own clone URL text, otherwise the
identity, and removes the member's exclusions from every matching wildcard;
Unregister removes the explicit declaration and excludes the member from every
matching wildcard. On marked rows a preview lists every target with its exact
path and identity, eligible, no-op or blocked with the reason; the list scrolls
while Cancel and the action stay in view, Cancel is the default and changes
nothing. The current row alone acts directly. Targets are fixed when
the action starts, and a refresh never changes them. They run one after another,
each with its own configuration write; targets of one identity share one change
and one reported failure. The batch popup lists every target with its outcome
(succeeded, no-op, failed, blocked, cancelled or unattempted) and separate disk
and config cells, empty when unchanged. Path-identified rows (no origin, an
unsupported origin, or an origin that is pending, stale or failed) and wildcard
patterns are blocked; Inspect and the menu show why. Immediately before its
write, each target is checked again. A target found as a checkout must still be
at its exact path with the same origin identity, and the configuration must
still declare the identity as it did when the action started, so a target shown
as a no-op is never written and a change that was already made is not reported
as done. Otherwise the target is blocked as stale (`stale: origin changed`,
`stale: checkout is gone or unreadable` or `stale: registration changed`,
followed by `after preview`, or `since the action started` for the current row
alone), nothing is written for it, and the other targets go on. Blocked and
failed targets are session problems; a cancelled preview is not.
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
with the guard that forbids it and `force cannot waive this`. A checkout found
at the configured destination of another identity's declaration is removed at
that found path, and the confirmation names the declaration, which stays.
Cancel is the default and changes nothing. A confirmation, like the batch
previews of the other actions and the Add preview, is answered only by keys
typed once it is on screen: input typed before it is drawn is discarded. Targets with losses stay unless the acknowledgment
row is ticked (Space, or Enter on the row); it covers exactly the listed losses
at the listed paths. Confirming runs the eligible targets one after another;
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
not a session problem. Quitting while a batch runs, with `q` in the batch popup
or a normal-mode quit key, pauses it and asks: Return
to inventory, the default, resumes it; Cancel remaining work and quit cancels
as above and exits with the session status. Ctrl-C exits `130` at once.
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
Session problems… lists every failure that makes the session exit `1`; the header
shows their count.
Popup content scrolls independently of its title and effective action hints.
In Menu, Inspect, Help, the batch preview, the removal confirmation and the Add preview, q or Esc closes
the current popup (the Add preview returns to its reference);
Ctrl-Q quits the app. Normal-mode q and Esc quit.
Configure shortcuts through [inventory key maps](config.md#inventory-key-maps).
Register, Unregister, Clone only, Clone + register, Remove + unregister, Copy value,
Open origin and the three remote actions have no default key; a configured key runs the same entry as the
menu. Hints and Help show the effective keys, and Help names the actions without
one. A refresh with an invalid key map keeps the previous bindings and records a
session problem.

Register, Unregister, Clone only and Clone + register, from the menu or from Add,
and Remove only and Remove + unregister are the repository mutations.
`--include-archived` without `--remote` exits `2` before raw mode.
Non-TTY use exits `2`; fatal startup/configuration errors exit `1`.

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
and a session problem, so the session exits `1`. When only the catalog or one
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
| Disable remote | Drops the running requests, hides the rows the providers added and removes their marks, naming them in the status line. The answers of this session and the archive preference stay for the next enable. |

An answer to a request that was dropped by disabling, a refresh or the archive
preference is ignored. `R` reloads the configuration and rescans the disk; it
merges the cached answers into the new rows and asks no provider. A provider
whose settings or wildcards changed reads `N cached (stale) · config changed`
until remote discovery fetches again. A configuration saved during a scan is
reloaded once per saved revision as soon as no dialog, menu or input is open
(`Config changed on disk · reloaded`). A save that does not load keeps the
previous configuration with `Config changed on disk but did not load: <reason> ·
previous configuration stays · R retries`, and the next save is tried again.

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

`worktree list` without `REPOSITORY` uses the repository of the working directory, from any directory inside it. Outside a repository it exits `2`. `worktree add` and `worktree remove` always take their target as an argument.

### `worktree list`

The human listing has one line per working tree with three columns, two spaces apart: the path, the branch, and the state. A detached HEAD reads `detached` followed by the short commit ID. The state column holds `primary`, `locked` and `prunable`, the last two with Git's reason when there is one. Worktrees placed outside the default location and worktrees created by other tools are listed too. With `--json` the command prints one document; see [worktree JSON](worktree-json.md).

A failure after the repository is found is one line on stderr and exit `1`, with the readable rows still printed.

### `worktree add`

| Options | Branch | Starts at |
| --- | --- | --- |
| none | A new branch named `NAME` | The primary checkout's HEAD, also when the command runs in a linked worktree |
| `--new-branch NEW` | A new branch named `NEW` | The primary checkout's HEAD |
| `--branch EXISTING` | The existing local branch | Where the branch is |
| `--start REF` with a new branch | The new branch | The commit `REF` names in the local repository |

`--start` applies only to a new branch, and `--branch` conflicts with `--new-branch`; both mistakes exit `2`. A new branch whose name exists, an `--branch` that names no local branch, and a `--start` that names no local commit exit `1`. Lager never fetches, never moves an existing branch, and leaves a branch that is checked out elsewhere to Git, which refuses it.

`NAME` is one path component: ASCII letters, digits, `.`, `_` and `-`, starting with a letter or digit. Git checks the branch name separately.

The destination is the first of these that applies:

1. `--path PATH`, absolute or relative to the working directory. `..` is resolved by name. The path can lie outside the repository root and is never written to the configuration.
2. With [`worktree_root`](config.md#top-level-fields) set: `<worktree_root>/<primary relative to root>.worktrees/<NAME>`. A primary checkout outside `root` needs `--path`.
3. `<primary>.worktrees/<NAME>`, beside the primary checkout.

Every destination has to be a path where nothing exists, an empty directory included. Lager refuses a destination below a symlink, inside a checkout, inside a Git directory, or still registered as a worktree whose directory is gone. On macOS `/tmp` and `/var` are symlinks, so an explicit path there is written as `/private/tmp/...`. Missing parent directories are created after every check has passed.

Git's own output appears on its own streams, followed by one line from Lager:

```text
$ lager worktree add org/project hotfix
Preparing worktree (new branch 'hotfix')
HEAD is now at 4be1d02 Release 2.3.1
Created worktree /home/dev/repos/org/project.worktrees/hotfix on new branch hotfix
```

A new worktree holds what Git checks out. Lager copies no environment files, dependencies or ignored files into it and runs none of its own hooks for it. Git runs the repository's `post-checkout` hook as it does for any `git worktree add`.

When Git fails, Lager rolls nothing back. It exits `1` and names what is left, one `remaining:` line each for a directory, a new branch and worktree metadata, or `remaining: nothing`.

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

The worktree commands print `error:` and `hint:` lines; the other commands prefix diagnostics with `lager:`.

## Non-interactive use

Outside a terminal, Lager never prompts:

- `init` requires `--root`, one root-creation choice, and one GitHub choice.
- `add` requires exactly one of `--register` or `--no-register`, unless `--post-clone` supplies registration.
- `remove` requires exactly one of `--unregister` or `--keep-registered`, plus `--yes`, and `--force` as well when the checkout has warnings.
- `tui` requires TTY stdin and stdout; use `status --json` for automation.
- `worktree remove` requires `--yes`, and `--force` as well when the worktree has local losses.

`--post-clone` conflicts with `--no-register`. `status --include-archived` and `tui --include-archived` require `--remote`. Explicit repository arguments bypass the picker.

## Output and exit status

Flag-driven and JSON output contain no interactive styling. `status --json` writes one stable JSON document to stdout; warnings and problems use stderr. Git and hook processes retain their native streams. A selected repository without a post-clone hook reports that on stderr.

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

A picker or repository interaction cancellation exits 130. Inventory's `q` and Esc use the session exit status described above. Invalid usage exits 2. Configuration, provider, Git, hook, filesystem, and aggregate failures exit 1. Success, no-op, empty selections, and explicitly skipped removals exit 0.

See [status JSON](status-json.md), [worktree JSON](worktree-json.md), [exit codes](exit-codes.md), [automation](../how-to/automation.md), and [safe removal](../how-to/remove-safely.md) for the detailed contracts.
