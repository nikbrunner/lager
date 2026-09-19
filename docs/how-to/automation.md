# Automate Lager

Use explicit arguments and flags in scripts; commands never require a terminal when their non-interactive choices are supplied.

```sh
lager add github.com/my-org/project --no-register
lager remove github.com/my-org/project --unregister --yes --force
lager ensure --include-archived
lager list --remote --json > repositories.json
```

`register` and `unregister` accept repeated repository arguments. `add` requires exactly `--register` or `--no-register` outside a TTY; `--post-clone` implies registration and conflicts with `--no-register`. `remove` similarly requires exactly one of `--unregister`/`--keep-registered` plus `--yes` outside a TTY. Invalid choices fail before disk or config mutation.

Use `list --json` as the automation boundary. It emits one JSON document on stdout;
warnings and provider errors use stderr. Git and hook subprocesses inherit stdin,
stdout, and stderr.

Human field values are presentation-escaped (`\n`, `\r`, `\t`, `\xNN`,
and `\\`) without changing renderer-owned separators. Do not decode those rows
into operational paths: JSON preserves the original serialized values.
Native Git and hook output is deliberately not filtered and may contain terminal
controls. Only run trusted hooks.

An `add`, `ensure`, or `hook` batch continues after ordinary independent failures
and returns aggregate status `1`. A native child reported as terminated by SIGINT
or exiting `130` instead stops later targets and returns `130`. Earlier successful disk and configuration
changes remain. An interrupted clone does not get registered or run a post-clone
hook; an interrupted hook can leave its completed clone and registration intact.
Do not use `exit 130` in a hook to report an ordinary error—use another nonzero
status if later independent targets should still run.
Lager does not intercept or forward signals sent to its own PID. Native shell
and process behavior remains in charge; external termination may bypass cleanup.
See [exit codes](../reference/exit-codes.md).
