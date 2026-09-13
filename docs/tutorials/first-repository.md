# Manage your first repository

This tutorial sets up a local root, declares one repository, and reconciles it.

## Before you start

Install Git and `lager`. You need `fzf` only when a command has no repository arguments. This tutorial uses explicit arguments and does not need a provider or network access.

## Set up a root

Choose a root under your home directory. `init` writes a new config and never overwrites an existing one:

```sh
lager init --root ~/repos --create-root --no-github
```

Outside a TTY, `--root`, exactly one of `--create-root`/`--no-create-root`, and exactly one of `--github`/`--no-github` are required. The persisted root remains portable (`~/repos`).

## Declare and add a repository

```sh
lager register https://github.com/example/project.git
lager add https://github.com/example/project.git --register
```

`add` places the repository at the computed destination. `--register` records it in the config; `--no-register` leaves declarations unchanged. A post-clone command implies registration:

```sh
lager add example/project --post-clone 'make setup'
```

Hooks run in the repository directory through `/bin/sh -c`, with native output on the process streams.

## Check and reconcile

```sh
lager list
lager ensure
```

`list` is offline by default. `ensure` adds missing effective declarations in config order. Running it again is safe and skips matching existing repositories.

For machine consumers use `lager list --json`; see the [JSON reference](../reference/list-json.md).
