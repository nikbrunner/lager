# CLI reference

```text
lager [--config PATH] <COMMAND>
```

Lager reads one configuration for each command. It selects the path in this order:

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
| `hook` | `hook REPOSITORY...` | Runs hooks for explicit declarations. At least one repository is required. |
| `list` | `list [--remote] [--include-archived] [--json]` | Reports declarations and local state. `--remote` expands wildcards through providers. |

Repository arguments accept canonical IDs such as `github.com/org/project`, GitHub shorthand such as `org/project`, and full SCP, SSH, HTTP(S), or file clone URLs. GitHub shorthand becomes an SSH clone URL.

## Defaults and interactive behavior

| Situation | Default behavior |
| --- | --- |
| `init` in a terminal without `--root` | Prompts with `repos`. |
| `init` without a root-creation choice | Prompts to create the root, default yes. |
| `init` without a GitHub choice | Prompts to configure GitHub, default yes. |
| `register`, `unregister`, or `add` without repositories | Uses `fzf --multi` to select candidates. |
| `remove` without repositories | Uses `fzf` to select standalone local checkouts. |
| Interactive `register` without `--post-clone` | Asks whether to add a hook for each repository, default no. |
| Interactive `add` without a registration choice | Asks whether to register each successful checkout, default yes, then offers its hook. |
| Interactive `remove` without `--yes` | Asks to remove each existing checkout, default no. |
| Interactive `remove` without a registration choice | Asks whether to unregister after successful or absent disk removal, default yes. |
| `list` without `--remote` | Reads configuration and disk only. Wildcards remain unexpanded. |
| `list` without `--json` | Renders human-readable tab-separated rows. |

`init` still writes the configuration when Git, `fzf`, or authenticated `gh` is missing. It reports those missing tools on stderr.

## Non-interactive use

Outside a terminal, Lager never prompts:

- `init` requires `--root`, one root-creation choice, and one GitHub choice.
- `add` requires exactly one of `--register` or `--no-register`, unless `--post-clone` supplies registration.
- `remove` requires exactly one of `--unregister` or `--keep-registered`, plus `--yes`.

`--post-clone` conflicts with `--no-register`. `list --include-archived` requires `--remote`. Explicit repository arguments bypass the picker.

## Output and exit status

Flag-driven and JSON output contain no interactive styling. `list --json` writes one stable JSON document to stdout; warnings and provider failures use stderr. Git and hook processes retain their native streams.

A picker or interaction cancellation exits 130. Invalid usage exits 2. Configuration, provider, Git, hook, filesystem, and aggregate failures exit 1. Success, no-op, empty selections, and explicitly skipped removals exit 0.

See [JSON output](list-json.md), [exit codes](exit-codes.md), [automation](../how-to/automation.md), and [safe removal](../how-to/remove-safely.md) for the detailed contracts.
