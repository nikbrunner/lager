# `list --json` reference

`lager list --json` emits exactly one JSON object on stdout. Warnings and provider errors are on stderr. The top-level schema is:

```json
{
  "schema_version": 1,
  "repositories": [],
  "provider_errors": []
}
```

Each concrete repository row contains `identity`, `clone_url`, `source`, `destination`, `hook`, `archived`, and `state`. `state` is `missing`, `cloned`, `conflict`, or `unreadable`. `source` is `explicit` or `wildcard`.

Offline wildcard rows instead contain `pattern` and `exclusions`; they have no concrete state. `--remote` expands those rows through configured providers, applies exclusions and explicit precedence, and labels archived rows. Provider failures appear in `provider_errors` with `provider` and `error` fields while successful rows remain present. JSON output is stable and contains no human decoration.
