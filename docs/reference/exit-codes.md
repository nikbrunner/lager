# Exit codes

- `0`: success, no-op, empty candidate set, or an explicitly skipped removal.
- `1`: configuration, provider, Git, hook, filesystem, or aggregate operation failure.
- `2`: invalid usage, missing non-TTY confirmation/registration choice, or unsupported command/flag.
- `130`: picker or interaction cancellation (Esc or Ctrl-C), or a completed
  native Git/hook child reported as interrupted by SIGINT or exiting with
  status `130`.

Native Git and hooks inherit stdin, stdout, and stderr. Ordinary child failures
return aggregate status `1` and allow independent later targets to run.
An observed child cancellation stops the batch: later targets do not run, and a cancelled clone is
not registered and does not run its hook. Completed clones and configuration
changes remain; cancellation is not a rollback. A cancelled hook can therefore
leave its clone and declaration intact.

Lager leaves signal handling, foreground process groups, and job control to the
OS, shell, and native programs. It does not forward signals sent only to Lager
or install a custom signal policy for library callers. An external signal can
terminate Lager before it observes a child result or performs cleanup.
Universal signal-driven terminal restoration and process supervision are not
guaranteed by this delivery.

For `list --json`, only the versioned document is written to stdout; warnings
and provider errors remain on stderr.
