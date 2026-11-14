# Exit codes

- `0`: success, no-op, empty candidate set, or an explicitly skipped removal.
- `1`: configuration, provider, Git, hook, filesystem, or aggregate operation failure.
- `2`: invalid usage, missing non-TTY confirmation/registration choice, or unsupported command/flag.
- `130`: picker or repository interaction cancellation (Esc or Ctrl-C), or a completed
  native Git/hook child reported as interrupted by SIGINT or exiting with
  status `130`.

For `tui`, normal-mode `q`/Esc or popup Ctrl-Q perform a normal
quit: `1` if discovery, observation or another operational failure occurred in
the session, including
failures followed by successful refresh; otherwise `0`. Informational exclusions,
deliberate background-work cancellation and a configuration changed on disk that
fails to load automatically do not count as failures. A
cancelled confirmation and a cancelled batch do not count either, including
cancelling from the quit prompt of a running batch; blocked and failed batch
targets do. Non-TTY
use and invalid flags exit `2` before raw mode;
fatal startup/configuration errors exit `1`. In Unix binary inventory sessions,
Ctrl-C or SIGINT exits `130`, SIGTERM `143` and SIGHUP `129` (128 plus the
signal number). The terminal is restored first; the local scan and the provider
requests then share one 2-second budget to end, and work still running after it,
such as a scan held in a blocking filesystem call, is left behind. Every exit
route does the same. SIGQUIT and SIGTSTP keep their default actions.
While a clone or hook owns the terminal, Ctrl-C interrupts it and the batch, and
a child that reports an interruption or exits `130` does the same: the session
exits `130` without returning to the inventory, and completed targets keep their
checkouts and registrations. SIGTERM or SIGHUP during that time takes effect when
the child returns; the session then exits `143` or `129`.

For `worktree`, `1` covers a repository that cannot be found, a refused
destination or branch, a blocked removal, local losses without `--force`, a
worktree that changed after its summary was shown, a failed Git command and any
entry in the `errors` of `worktree list --json`. `2` covers invalid `NAME` or
branch names, `--start` without a new branch, `worktree list` outside a
repository without a target, `worktree_root` placement of a primary checkout
outside `root` without `--path`, and `worktree remove` without `--yes` outside a
terminal. Declining the removal question exits `0`; Ctrl-C at it exits `130`.

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

`status` (and its aliases `list` and `ls`) exits `0` when the scan finished
and nothing failed. It exits `1` when a path could not be read, a Git
observation failed, or a provider failed with `--remote`: the failures that
make a `tui` session exit `1`. A root that does not exist yet is an empty
scan with a note on stderr and exits `0`; a root that is not a directory
exits `1`. The rows are printed in every case, and each failure is one line on
stderr. Invalid flags exit `2` and invalid configuration exits `1`, both before
any output. With `--json`, only the versioned document is written to stdout;
its `problems` list holds the same failures.
