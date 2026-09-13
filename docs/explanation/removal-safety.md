# Why removal is guarded

Permanent deletion combines filesystem and Git identity risks. For explicit arguments Lager resolves a destination under the configured root. For no-argument discovery it instead retains the exact canonical discovered path as an opaque selection value and displays that path; it never recomputes a conventional destination after selection. Lager initially rejects symlinks and non-standalone repositories, compares normalized `origin`, inspects local-state warnings, and asks for confirmation. After confirmation it revalidates the selected exact path and matching origin immediately before deletion. `--force` is intentionally limited to warnings; it cannot authorize an escape, an origin mismatch, or an unsafe repository shape.

Configuration cleanup happens after successful disk removal. This ordering keeps a failed or partial filesystem operation visible and recoverable instead of silently dropping the declaration.
