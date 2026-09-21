# Exit codes

- `0`: success, no-op, empty candidate set, or an explicitly skipped removal.
- `1`: configuration, provider, Git, hook, filesystem, or aggregate operation failure.
- `2`: invalid usage, missing non-TTY confirmation/registration choice, or unsupported command/flag.
- `130`: picker or repository interaction cancellation (Esc or Ctrl-C), or a completed
  native Git/hook child reported as interrupted by SIGINT or exiting with
  status `130`.

For `inventory` / `inv`, `q` and Esc perform a normal quit: `1` if discovery,
observation or another operational failure occurred in the session, including
failures followed by successful refresh; otherwise `0`. Informational exclusions
and deliberate background-work cancellation do not count as failures. Non-TTY
use, invalid flags and unavailable remote discovery exit `2` before raw mode;
fatal startup/configuration errors exit `1`. In Unix binary inventory sessions,
Ctrl-C or SIGINT exits `130`; SIGTERM exits `143`. Workers are cancelled and the
terminal is restored before returning.

Native Git and hooks inherit stdin, stdout, and stderr. Ordinary child failures
return aggregate status `1` and allow independent later targets to run.
An observed child cancellation stops the batch: later targets do not run, and a cancelled clone is
not registered and does not run its hook. Completed clones and configuration
changes remain; cancellation is not a rollback. A cancelled hook can therefore
leave its clone and declaration intact.

Repository commands leave signal handling, foreground process groups and job
control to the OS, shell and native programs. They do not forward signals sent
only to Lager; an external signal can terminate Lager before it observes a child
result or performs cleanup. `lager::cli::run()` installs no signal handlers for
library callers. `lager::cli::run_binary()` scopes its interruption policy to
inventory and restores previous signal actions afterward.

For `list --json`, only the versioned document is written to stdout; warnings
and provider errors remain on stderr.
