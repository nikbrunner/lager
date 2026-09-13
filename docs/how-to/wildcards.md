# Use repository wildcards

A declaration may end in one trailing `/*`:

```toml
[[repositories]]
url = "github.com/my-org/*"
exclude = ["retired"]
```

The provider expands the pattern only for `ensure`, `list --remote`, and picker commands. Members are deduplicated in declaration order. An explicit declaration takes precedence over a wildcard. `exclude` values are provider-relative names and are applied after expansion.

Wildcards cannot have `post_clone`; register a concrete member when it needs a hook. `unregister github.com/my-org/project` adds `project` to matching exclusions, while registering it again removes that exclusion. Offline `list` shows the pattern and exclusions without contacting a provider.

Use `--include-archived` to include archived members. It requires `--remote` for `list` because offline listing deliberately performs no provider access.
