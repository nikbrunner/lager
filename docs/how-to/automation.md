# Automate Lager

Use explicit arguments and flags in scripts; commands never require a terminal when their non-interactive choices are supplied.

```sh
lager add github.com/my-org/project --no-register
lager remove github.com/my-org/project --unregister --yes --force
lager ensure --include-archived
lager list --remote --json > repositories.json
```

`register` and `unregister` accept repeated repository arguments. `add` requires exactly `--register` or `--no-register` outside a TTY; `--post-clone` implies registration and conflicts with `--no-register`. `remove` similarly requires exactly one of `--unregister`/`--keep-registered` plus `--yes` outside a TTY. Invalid choices fail before disk or config mutation.

Use `list --json` as the automation boundary. It emits one JSON document on stdout; warnings and provider errors use stderr. Git and hook subprocesses inherit their normal streams. A batch continues after independent failures and returns aggregate status. See [exit codes](../reference/exit-codes.md).
