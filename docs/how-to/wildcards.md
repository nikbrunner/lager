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

Lager adds `project` to every matching wildcard's `exclude` array. For overlapping
patterns, each exclusion is relative to that pattern: `org/*` excludes `sub/project`,
while `org/sub/*` excludes `project`. Register the repository again to remove all
matching exclusions:

```sh
lager register github.com/my-org/project
```

`exclude` values are provider-relative names. They cannot be empty, absolute, `~`, `..`, or contain another wildcard.

## Set the precedence you want

Explicit declarations take precedence over wildcard members. Wildcards cannot have `post_clone`; add an explicit declaration for a member that needs a hook.

`register REPOSITORY --post-clone CMD` restores the member in all matching
wildcards and creates or updates its explicit hook declaration in the same
transaction. `unregister REPOSITORY` removes that explicit declaration and
excludes the member from every matching wildcard. Repeating either operation
without changing the hook is a no-op.

Lager expands members in declaration order, applies exclusions and explicit precedence, then deduplicates them. Offline `list` preserves the wildcard rule and exclusions without contacting a provider. Use `--include-archived` to include archived members; with `list`, that requires `--remote`.

See the [configuration reference](../reference/config.md) for the complete wildcard validation rules.

Human rows and provider diagnostics use visible escapes for terminal controls;
this does not alter matching, exclusions, or stored patterns. For automation,
read `list --remote --json` rather than parsing escaped human rows.
