# CLI reference

Global options:

- `--config PATH` selects a config file.
- `LAGER_CONFIG` selects the file when `--config` is absent.
- The default is `$HOME/.config/lager/config.toml`.

Commands:

- `init --root ROOT [--create-root|--no-create-root] [--github|--no-github]` creates a config once.
- `register [REPOSITORY]... [--post-clone CMD] [--include-archived]` declares repositories.
- `unregister [REPOSITORY]... [--include-archived]` removes declarations or wildcard members. It does not accept hook options.
- `add [REPOSITORY]... --register|--no-register [--post-clone CMD] [--include-archived]` adds local repositories.
- `remove [REPOSITORY]... --unregister|--keep-registered [--yes] [--force]` safely removes local repositories.
- `ensure [--include-archived]` reconciles all effective declarations.
- `hook REPOSITORY...` reruns hooks and requires explicit declarations.
- `list [--remote] [--include-archived] [--json]` reports local or remote state.

In a TTY, omitted `init` values are prompted with `repos`, create-root yes, and GitHub yes defaults. Initialization still creates the config when Git, `fzf`, or authenticated `gh` is unavailable, and reports each missing tool on stderr.

No-argument `register`, `unregister`, and `add` use `fzf --multi`; no-argument `remove` selects standalone local repositories. Explicit arguments bypass the picker. Interactive registration asks whether to configure a hook for each repository, defaulting to no. Interactive add asks whether to register each successful repository, defaulting to yes, then offers its hook. Interactive remove confirms deletion first and asks whether to unregister only after the checkout was removed or found absent, defaulting to yes. Esc or Ctrl-C cancellation exits 130 and stops the remaining batch.

Outside a TTY, `init`, `add`, and `remove` require the documented explicit choice flags. Flag-driven and JSON output never include interactive styling. Archived filtering requires remote listing.
