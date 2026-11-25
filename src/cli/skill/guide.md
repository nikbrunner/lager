# Lager guide for agents

Lager keeps a declared set of Git repositories available in one local root, and manages their checkouts, worktrees and terminal sessions. A declaration in the configuration is the desired state; a checkout on disk is local state, and the two are independent.

This guide describes the installed version. Run `lager --help` and `lager <command> --help` for every option and example.

## Working rules

- Read with `--json`. A JSON document goes to stdout and diagnostics go to stderr. Schemas: `docs/reference/status-json.md`, `worktree-json.md` and `session-json.md` in https://github.com/nikbrunner/lager.
- Pass every repository as an argument. Without one, a command opens a picker and exits `2` outside a terminal.
- Run non-interactive commands only. `lager tui` needs a terminal; leave it to the user.
- Read exit codes: `0` success, `1` failure, `2` invalid usage, `130` cancelled. `status` exits `1` on any problem and still prints its rows.
- Name a repository as `org/project` (GitHub), `github.com/org/project`, a clone URL, or a checkout path such as `.`.
- Ask the user before anything destructive or remote: `remove` deletes a checkout, `remove --remote`, `rename`, `archive` and `visibility` change GitHub, and `--post-clone` runs a shell command. `--force` waits for the user to accept the warnings `remove` lists.

## First-time setup

Lager reads `--config PATH`, then `LAGER_CONFIG`, then `$HOME/.config/lager/config.toml`. When `lager status --json` fails because no configuration exists, create one. The user decides the repository root and the providers, so ask before choosing them. Pass every choice as a flag, so the command never prompts:

```sh
lager init --root ~/repos --create-root --github
```

`--no-create-root` and `--no-github` are the other choices. `init` never overwrites an existing configuration. Bitbucket providers, `worktree_path` and the `[session]` section are optional; see `docs/reference/config.md` in https://github.com/nikbrunner/lager.

Declare and clone a first repository with `lager add org/project --register`, then confirm with `lager status --json`.

## Workflows

| Goal | Command |
| --- | --- |
| See every repository, its checkout, branch and changes | `lager status --json` (offline; `--remote` also expands wildcards through providers) |
| Declare a repository | `lager register org/project` |
| Clone a repository | `lager add org/project --register` (`--no-register` keeps it undeclared) |
| Clone every declared repository that is missing | `lager ensure` |
| Delete a checkout | `lager remove org/project --unregister --yes` (`--keep-registered` keeps the declaration) |
| List the worktrees of a repository | `lager worktree list [REPOSITORY] --json` |
| Start a branch in a worktree | `lager worktree add org/project NAME [--branch EXISTING \| --new-branch NEW] --json` |
| Check out an existing local or remote branch in a worktree | `lager worktree add org/project --branch origin/feat/x --json` (`NAME` defaults to the last segment of the branch; a remote branch becomes a local branch that tracks it; add `--fetch` to fetch every remote first) |
| Remove a worktree, keeping its branch | `lager worktree remove PATH --yes` |
| List the checkouts that have a session | `lager session list --json` |
| Create a session without switching to it | `lager session create org/project [NAME] --json` |
| Create a worktree and its session in one call | `lager worktree add org/project NAME --create-session --json` |

`status --json` rows carry `kind`, `identity`, `registration`, `checkout`, `found_path`, `branch` and `changes`. Use `found_path` as the checkout's location.

Sessions need a `[session]` section in the configuration and a running terminal host (Rex). `session open` switches the user's terminal to the session; prefer `session create` when the user should stay where they are. The `--create-session` output lists the host's id of every pane by name, so work can be sent to a pane.

The configuration is TOML at `$HOME/.config/lager/config.toml`, or the path from `--config` or `LAGER_CONFIG`. Fields: `docs/reference/config.md` in https://github.com/nikbrunner/lager.
