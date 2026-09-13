# Wildcards and exclusions

A wildcard is a declarative discovery rule, not a local filesystem glob. `register` accepts only a trailing `/*` declaration and wildcard declarations cannot store hooks; `add` never clones a wildcard reference directly. Providers expand it into concrete identities, then Lager applies exclusions, explicit declarations, and deduplication. This makes declaration order meaningful and lets a concrete entry attach a hook without being hidden by a broad pattern.

Offline listing preserves the rule itself because expansion would require provider access. Remote listing and reconciliation expand only when requested, so scripts can choose deterministic offline behavior or current provider state explicitly.
