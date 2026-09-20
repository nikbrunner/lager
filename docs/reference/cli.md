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
| `list`, `ls` | `list [--remote] [--include-archived] [--json]` | Reports declarations and local state. `--remote` expands wildcards through providers. |
| `inventory`, `inv` | `inventory` | Opens a read-only offline overview; requires TTY stdin and stdout. |

`ls` is a visible alias for `list`. Both names accept the same arguments, options, and configuration selection, and produce identical data output and exit statuses. Help and usage text reflect the invoked name.

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
| `remove` without repositories | Uses `fzf` to select standalone local checkouts. |
| Interactive `register` without `--post-clone` | Asks whether to add a hook for each repository, default no. |
| Interactive `add` without a registration choice | Asks whether to register each successful checkout, default yes, then offers its hook. |
| Interactive `remove` without `--yes` | Asks to remove each existing checkout, default no. |
| Interactive `remove` without a registration choice | Asks whether to unregister after successful or absent disk removal, default yes. |
| `list` without `--remote` | Reads configuration and disk only. Wildcards remain unexpanded. |
| `list` without `--json` | Renders human-readable tab-separated rows. |

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

`inventory` and `inv` have identical behavior. They show declarations and local
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

This checkpoint supports quit and local refresh. Navigation, search, menus and
mutations are unavailable. Remote discovery is unavailable: `--remote`, including
with `--include-archived`, exits `2` before raw mode. `--include-archived` without
`--remote` is also invalid. Non-TTY use exits `2`; fatal startup/configuration
errors exit `1`.

## Non-interactive use

Outside a terminal, Lager never prompts:

- `init` requires `--root`, one root-creation choice, and one GitHub choice.
- `add` requires exactly one of `--register` or `--no-register`, unless `--post-clone` supplies registration.
- `remove` requires exactly one of `--unregister` or `--keep-registered`, plus `--yes`.
- `inventory` requires TTY stdin and stdout; use `list --json` for automation.

`--post-clone` conflicts with `--no-register`. `list --include-archived` requires `--remote`. Explicit repository arguments bypass the picker.

## Output and exit status

Flag-driven and JSON output contain no interactive styling. `list --json` writes one stable JSON document to stdout; warnings and provider failures use stderr. Git and hook processes retain their native streams. A selected repository without a post-clone hook reports that on stderr.

Lager-owned human fields escape C0, DEL, and C1 controls: newline, carriage
return, and tab appear as `\n`, `\r`, and `\t`; other controls appear as
`\x00` through `\x9f`. Literal backslashes appear as `\\`, so a literal
`\x1b` cannot impersonate an escaped ESC. Human list columns and picker columns
retain renderer-owned tabs; field values cannot inject extra rows or columns.
This applies to diagnostics, progress messages, prompt labels/default hints, and
picker labels. It does not change references, paths, hook commands, prompt
answers, JSON field values, or native Git/hook streams.

Removal picker labels are rendered from identity and exact path, not parsed from
display text. The Rust `Fzf` adapter treats `SelectionCandidate.display` as an
opaque escaped field when `exact_path` is absent; when present, it renders the
identity and path instead. Duplicate labels map back to distinct untouched
candidates by hidden row ID, in the order returned by the picker.

A picker or repository interaction cancellation exits 130. Inventory's `q` and Esc use the session exit status described above. Invalid usage exits 2. Configuration, provider, Git, hook, filesystem, and aggregate failures exit 1. Success, no-op, empty selections, and explicitly skipped removals exit 0.

See [JSON output](list-json.md), [exit codes](exit-codes.md), [automation](../how-to/automation.md), and [safe removal](../how-to/remove-safely.md) for the detailed contracts.
