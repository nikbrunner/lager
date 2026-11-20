<h1 align="center">lager</h1>

<p align="center">A declarative local Git repository manager.</p>

<p align="center">
  <a href="https://github.com/nikbrunner/lager/actions/workflows/ci.yml"><img src="https://github.com/nikbrunner/lager/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/nikbrunner/lager/releases/latest"><img src="https://img.shields.io/github/v/release/nikbrunner/lager?display_name=tag&amp;sort=semver" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/nikbrunner/lager" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/Rust-2024-dea584?logo=rust" alt="Rust 2024">
</p>

<p align="center">
  <img src="assets/lager-banner.png" alt="A sunlit warehouse with the lager name on a storage pillar">
</p>

`lager` keeps a declared set of Git repositories available in one local root. It runs on Linux and macOS, on x86_64 and ARM64.

## Install

### Release archive

Download the archive for your platform from [GitHub Releases](https://github.com/nikbrunner/lager/releases), unpack it somewhere on `PATH`, and run `lager --version`.

### Mise

Install the latest GitHub release globally:

```sh
mise use --global github:nikbrunner/lager
```

### Source

Build and install the current source with Cargo:

```sh
cargo install --git https://github.com/nikbrunner/lager --locked
```

### Local development

Install the current checkout as a release build:

```sh
cargo install --path . --force
```

Cargo places `lager` in `~/.cargo/bin`. An untagged Git checkout reports its commit in `lager --version`, for example `lager 0.1.1-dev.55d7d5b`. Run the command again after local changes.

If Mise manages Lager, its shim takes precedence over `~/.cargo/bin` and runs the downloaded release instead. Remove the Mise tool and regenerate its shims to use the Cargo binary:

```sh
mise use --global --remove github:nikbrunner/lager
mise uninstall --all github:nikbrunner/lager
mise reshim --force
```

Git is required for `add`, `remove`, `ensure`, `hook`, `status` (and `list`, `ls`), `worktree`, and `tui`. Interactive repository selection uses [`fzf`](https://github.com/junegunn/fzf). GitHub discovery uses an authenticated `gh` CLI; Bitbucket discovery uses the credentials named in the configuration.

## Quick start

```sh
lager init --root ~/repos --create-root --github
lager register github.com/my-org/project
lager add github.com/my-org/project --register
lager status
lager ensure
```

`register` declares the repository. `add` creates its checkout. `ensure` creates every declared checkout that is missing.

## Configuration

`init` writes `$HOME/.config/lager/config.toml` by default. A configuration declares the local root, optional discovery providers, and repositories to keep available:

```toml
root = "~/repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/my-org/project"
```

This example places the repository at `~/repos/my-org/project`. Lager chooses the config path in this order: `--config PATH`, `LAGER_CONFIG`, then `$HOME/.config/lager/config.toml`.

When `init` runs in a terminal without choices, its defaults are `repos` for the root, create the root, and configure GitHub. Outside a terminal, pass `--root`, one of `--create-root` or `--no-create-root`, and one of `--github` or `--no-github` explicitly.

See the [configuration reference](docs/reference/config.md) for every field, default, and validation rule.

## Commands

| Command | What it does | Common options |
| --- | --- | --- |
| `init` | Creates a configuration and selects a repository root. | `--root`, `--create-root`, `--github` |
| `register` | Declares repositories for management. | `--post-clone`, `--include-archived` |
| `unregister` | Removes repository declarations or wildcard members. | `--include-archived` |
| `add` | Creates local checkouts. | `--register`, `--post-clone`, `--include-archived` |
| `remove` | Permanently deletes guarded local checkouts. | `--unregister`, `--keep-registered`, `--yes`, `--force` |
| `ensure` | Creates missing declared checkouts. | `--include-archived` |
| `hook` | Runs a declared repository's post-clone hook again. | — |
| `status`, `list`, `ls` | Prints the rows `tui` shows: declarations, checkouts and their state. With `--remote`, `tui` adds unregistered and excluded candidates that `status` omits. | `--remote`, `--include-archived`, `--registered`, `--json` |
| `worktree list`, `add`, `remove` | Lists, creates and removes linked worktrees of one repository through native Git. | `--json`, `--branch`, `--new-branch`, `--start`, `--path`, `--yes`, `--force` |
| `session open`, `create`, `close`, `list` | Opens a checkout's terminal session in Rex, creates it without switching to it, closes it, and lists the checkouts that have one. Needs a `[session]` section in the configuration. | `--json` |
| `tui` | Opens the inventory in a terminal: it registers, unregisters, clones and removes repositories and browses what the providers list. | `--remote`, `--include-archived`; `r` removes a checkout after confirmation; `w` opens its worktrees; `m` opens the actions; `R` refreshes local state; `q` or Esc quits. |

`list` and `ls` are identical aliases of `status`, which is offline by default and needs no terminal. Use `status --json` for stable machine-readable output. The [CLI reference](docs/reference/cli.md) documents every option, default, and interactive behavior.

Lager escapes terminal control characters and literal backslashes in its own
human fields, diagnostics, and picker labels. Stored values, JSON, and native
Git/hook streams are unchanged. See [safe removal](docs/how-to/remove-safely.md)
before deleting checkouts; `--force` never bypasses ownership or dependent
worktree guards.

## Documentation

- [First repository tutorial](docs/tutorials/first-repository.md)
- [CLI reference](docs/reference/cli.md) · [configuration reference](docs/reference/config.md)
- [Provider setup](docs/how-to/providers.md) · [automation](docs/how-to/automation.md)
- [Safe removal](docs/how-to/remove-safely.md) · [wildcards](docs/how-to/wildcards.md) · [worktrees](docs/how-to/worktrees.md) · [sessions](docs/how-to/sessions.md)
- [status JSON](docs/reference/status-json.md) · [worktree JSON](docs/reference/worktree-json.md) · [session JSON](docs/reference/session-json.md) · [exit codes](docs/reference/exit-codes.md) · [performance](docs/reference/performance.md)
- [Declarations and local state](docs/explanation/declarations-and-local-state.md)

## Development

Install standalone [Lefthook](https://lefthook.dev/) before working on a clone,
then install the Git hooks from the repository root:

```sh
lefthook install
```

The installed hooks run the same checks by stage: `pre-commit` runs formatting,
and `pre-push` runs Clippy and tests.

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

Use the sandboxed [human acceptance checklist](HOW_TO_TEST.md) to verify the
repository-management journeys. Current milestone evidence is recorded in
[PRD §3](docs/prd/1.0.0.md#3-repository-management); local macOS results are not
a substitute for the outstanding Linux validation.

## License

MIT
