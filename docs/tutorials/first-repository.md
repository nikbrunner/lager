# Manage your first repository

This tutorial creates a local root, declares one repository, and keeps its checkout available.

## Before you start

Install Git and `lager`. This path clones a repository, so it needs network access and credentials accepted by the repository host. The GitHub shorthand used below resolves to an SSH URL, so make sure your SSH key has access.

You need `fzf` only when a command has no repository arguments. This tutorial uses explicit arguments.

## Create a root

Choose a root under your home directory and create a configuration:

```sh
lager init --root ~/repos --create-root --github
```

`init` writes the portable value `~/repos` to `$HOME/.config/lager/config.toml` and adds the GitHub discovery provider. It never overwrites an existing configuration.

Outside a terminal, `init` requires `--root`, exactly one of `--create-root` or `--no-create-root`, and exactly one of `--github` or `--no-github`.

## Declare and add a repository

Declare the repository you want Lager to manage, then create its checkout:

```sh
lager register github.com/my-org/project
lager add github.com/my-org/project --register
```

The declaration is the desired state. The checkout is local state. `add` places this repository at `~/repos/my-org/project`; `--register` keeps the declaration if it was not already present.

A post-clone command implies registration and runs only after a fresh clone:

```sh
lager add github.com/my-org/project --post-clone 'make setup'
```

Hooks run from the repository directory through `/bin/sh -c`, with their normal output streams.

## Check and reconcile

```sh
lager list
lager ensure
```

`list` compares declarations with disk without contacting providers. `ensure` creates missing effective declarations in configuration order. Running it again leaves matching checkouts alone.

Next, read the [configuration reference](../reference/config.md) to add providers, hooks, and wildcards. For machine consumers, use `lager list --json`; see the [JSON reference](../reference/list-json.md).
