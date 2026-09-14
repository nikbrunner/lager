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

`lager` keeps a declared set of Git repositories available in one local root. It is a stable-Rust CLI for Linux and macOS, on x86_64 and ARM64.

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

Interactive repository selection requires [`fzf`](https://github.com/junegunn/fzf). Git is required for `add`, `remove`, `ensure`, and `hook`. GitHub discovery uses the authenticated `gh` CLI (`gh auth login`); Bitbucket discovery uses its configured API credentials.

## Quick start

```sh
lager init --root ~/repos --create-root --github
lager register github.com/my-org/project
lager add github.com/my-org/project --register
lager list
lager ensure
```

Use `--config PATH` or `LAGER_CONFIG` to select a config file. The default is `$HOME/.config/lager/config.toml`. `list --json` is the stable machine-readable interface.

## Commands

| Command | What it does | Common options |
| --- | --- | --- |
| `init` | Creates the configuration and chooses a repository root. | `--root`, `--create-root`, `--github` |
| `register` | Declares repositories that lager should manage. | `--post-clone`, `--include-archived` |
| `unregister` | Removes repository declarations or wildcard members. | `--include-archived` |
| `add` | Clones repositories into the configured root. | `--register`, `--post-clone`, `--include-archived` |
| `remove` | Removes local repositories safely. | `--unregister`, `--keep-registered`, `--yes`, `--force` |
| `ensure` | Reconciles every declared repository with the local root. | `--include-archived` |
| `hook` | Runs a declared repository's post-clone hook again. | — |
| `list` | Shows local repositories or discovers remote repository state. | `--remote`, `--include-archived`, `--json` |

Without repository arguments, `register`, `unregister`, and `add` use `fzf --multi`; `remove` selects standalone local repositories. In a terminal, omitted `init` values are prompted. Outside a terminal, `init`, `add`, and `remove` require their explicit choice flags. See the [CLI reference](docs/reference/cli.md) for every option and interactive behavior.

## Documentation

- [First repository tutorial](docs/tutorials/first-repository.md)
- [CLI reference](docs/reference/cli.md) · [config reference](docs/reference/config.md)
- [Provider setup](docs/how-to/providers.md) · [automation](docs/how-to/automation.md)
- [Safe removal](docs/how-to/remove-safely.md) · [wildcards](docs/how-to/wildcards.md)
- [JSON output](docs/reference/list-json.md) · [exit codes](docs/reference/exit-codes.md)
- [Managed repositories](docs/explanation/managed-repositories.md) · [v1 limits](docs/explanation/v1-scope.md)

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

## Release bootstrap

The first live release-please pull request must propose exactly `v0.1.0`; do not merge a bootstrap PR for any other version.

## License

MIT
