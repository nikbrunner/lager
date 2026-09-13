# lager

`lager` keeps a declared set of Git repositories available in one local root. It is a stable-Rust CLI for Linux and macOS (x86_64 and ARM64).

## Install

From a released archive, unpack `lager` somewhere on `PATH`. The archive also contains `LICENSE` and this README. To install from source:

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
