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
| `status`, `list`, `ls` | `status [--remote] [--include-archived] [--registered] [--json]` | Prints the rows `tui` shows, once its scan has finished: a table, or one JSON document with `--json`. `--remote` expands wildcards through providers; `--registered` keeps declared repositories and wildcard patterns. |
| `tui` | `tui` | Opens the offline overview with registration actions; requires TTY stdin and stdout. |

`list` and `ls` are visible aliases of `status`. All three names accept the same arguments, options, and configuration selection, and produce identical output and exit statuses.

`status` needs no terminal. It scans the root the way `tui` does, through Git with optional locks off, and writes nothing to the configuration or to any repository. The table has the columns of the wide inventory grid (repository, path, registration, checkout, origin, branch, changes) with the same cell text, aligned with two spaces, never truncated. A repository the provider reports as archived reads `NAME [archived]` in the repository column. An empty inventory prints the header only. Each problem (an unreadable path, a failed Git observation, a failed provider) is one line on stderr, and the rows are still printed. See [status JSON](status-json.md) for the document.

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

## Offline inventory

`tui` shows declarations and local
Git roots beneath the configured root without provider requests or configuration
writes. Registration and checkout state are independent; duplicate origins keep
separate rows for their exact paths. Originless checkouts display `No origin`.

Discovery recurses through directories, prunes found Git roots, and excludes
directory symlinks, linked worktrees, submodules and bare repositories. Unreadable
paths produce partial-scan diagnostics. Pending, failed and stale observations
remain distinct from clean status.

Press `R` to reload configuration and refresh local observations. Earlier
observations remain marked stale until refreshed. Press `q` or Esc to cancel
background work and quit, restoring the terminal. A normal quit returns `1` if
an operational failure occurred during the session, even after a successful
refresh; otherwise it returns `0`. Informational exclusions and deliberate
cancellation do not count as failures.

Use `j`/`k` or arrow keys to select a row, `/` for live fuzzy search, Space to
mark the row, `a` to add a repository by reference, `r` to remove the checkout and keep its registration, Enter to inspect the selected repository, `m` for the action menu,
and `?` for effective bindings. Search matches full attributes, paths and origins; Enter/Esc
retain the filter and normal-mode Backspace clears it. Ordinary search letters
remain text; F1 opens search help. Help describes the mode it was opened from.
Marks survive filtering and refresh while their exact target remains; the search
row shows the marked and hidden-marked counts, and the status line reports marks
removed with a vanished entry. Wildcard pattern rows cannot be marked.
The menu groups actions for this row (Repository actions…, Inspect, Copy value…,
Open origin, Worktrees…), the selection (mark this row, mark visible concrete
rows, clear all marks) and the session (Add, Remote discovery…, Refresh, Session
problems…, batch cancellation, Help and Quit). Entries for actions this build
does not provide report that they are unavailable.
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
Register, Unregister, Clone only, Clone + register, Remove + unregister, Copy value
and Open origin have no default key; a configured key runs the same entry as the
menu. Hints and Help show the effective keys, and Help names the actions without
one. A refresh with an invalid key map keeps the previous bindings and records a
session problem.

Register, Unregister, Clone only and Clone + register, from the menu or from Add,
and Remove only and Remove + unregister are the repository mutations; remote
discovery is unavailable. `--remote`, including with `--include-archived`, exits
`2` before raw mode. `--include-archived` without `--remote` is also invalid.
Non-TTY use exits `2`; fatal startup/configuration errors exit `1`.

On Unix, Ctrl-C or SIGINT interrupts binary inventory with exit `130`; SIGTERM
exits `143`. Inventory cancels its workers and restores the terminal before
returning. This signal policy is scoped to the binary inventory invocation;
`lager::cli::run()` retains the embedding process's signal policy.

Use the [isolated inventory walkthrough](../how-to/verify-inventory.md) for
acceptance testing without your normal repositories or configuration.

## Non-interactive use

Outside a terminal, Lager never prompts:

- `init` requires `--root`, one root-creation choice, and one GitHub choice.
- `add` requires exactly one of `--register` or `--no-register`, unless `--post-clone` supplies registration.
- `remove` requires exactly one of `--unregister` or `--keep-registered`, plus `--yes`.
- `tui` requires TTY stdin and stdout; use `status --json` for automation.

`--post-clone` conflicts with `--no-register`. `status --include-archived` requires `--remote`. Explicit repository arguments bypass the picker.

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

See [status JSON](status-json.md), [exit codes](exit-codes.md), [automation](../how-to/automation.md), and [safe removal](../how-to/remove-safely.md) for the detailed contracts.
