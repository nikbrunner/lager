# Use repository wildcards

A wildcard is a provider-backed declaration, not a filesystem glob. It ends in one trailing `/*`:

```toml
[[repositories]]
url = "github.com/my-org/*"
exclude = ["retired"]
```

The provider expands this pattern for `ensure`, `list --remote`, and no-argument picker commands. `add` never clones a wildcard directly.

## Exclude or restore one member

Remove a concrete member from a wildcard without deleting the wildcard:

```sh
lager unregister github.com/my-org/project
```

Lager adds `project` to the matching wildcard's `exclude` array. Register the repository again to remove that exclusion:

```sh
lager register github.com/my-org/project
```

`exclude` values are provider-relative names. They cannot be empty, absolute, `~`, `..`, or contain another wildcard.

## Set the precedence you want

Explicit declarations take precedence over wildcard members. Wildcards cannot have `post_clone`; add an explicit declaration for a member that needs a hook.

Lager expands members in declaration order, applies exclusions and explicit precedence, then deduplicates them. Offline `list` preserves the wildcard rule and exclusions without contacting a provider. Use `--include-archived` to include archived members; with `list`, that requires `--remote`.

See the [configuration reference](../reference/config.md) for the complete wildcard validation rules.
