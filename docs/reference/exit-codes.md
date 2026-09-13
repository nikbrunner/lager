# Exit codes

- `0`: success, no-op, empty candidate set, or an explicitly skipped removal.
- `1`: configuration, provider, Git, hook, filesystem, or aggregate operation failure.
- `2`: invalid usage, missing non-TTY confirmation/registration choice, or unsupported command/flag.
- `130`: picker or interaction cancellation (Esc or Ctrl-C).

Native Git and hook diagnostics remain on their inherited stdout/stderr streams. For `list --json`, only the versioned document is written to stdout; warnings and provider errors remain on stderr.
