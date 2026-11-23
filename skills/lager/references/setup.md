# Install lager

Help the user install lager. Run an install command only after the user agrees to it.

## Check what exists

```sh
command -v lager && lager --version
```

A version line means lager is installed. When `lager skill` still fails, the installed version predates the guide: update lager with the route it came from, then run `lager skill` again.

## Install

Pick the first route that fits the user's machine. Lager runs on Linux and macOS, on x86_64 and ARM64, and needs Git.

| Route | Command |
| --- | --- |
| Mise | `mise use --global github:nikbrunner/lager` |
| Cargo | `cargo install --git https://github.com/nikbrunner/lager --locked` |
| Release archive | Download the archive from https://github.com/nikbrunner/lager/releases and unpack it on `PATH`. |

`fzf` powers interactive repository selection. GitHub discovery uses an authenticated `gh` CLI. Both are optional.

## Continue with the guide

```sh
lager --version
lager skill
```

`lager skill` prints the usage guide of the installed version, including first-time configuration. Follow it from there.
