# Lager

Lager is a stable Rust CLI and library for declarative local Git repositories.

- Keep domain and application services independent of terminal UI and subprocess details.
- Keep CLI and future TUI adapters on the same application services.
- Use native Git and shell processes where the product contract requires native output.
- Persist portable TOML through the symlink-aware config store; never persist machine-specific absolute paths.
- Keep Git, filesystem, providers and config storage behind explicit boundaries. Add no generic plugin or workflow framework, and keep the public Rust library contract.
- Configure one terminal host at a time and keep its adapter small enough that another host needs no change elsewhere.
- Pass paths and references as process arguments, and quote intentional shell substitutions. Hooks are trusted executable configuration; discovery never runs repository-supplied code. Escape external text against terminal control sequences and store no credentials.
- Keep tests at the binary boundary for user-visible behavior. Drive the TUI through a real PTY, and test worktrees and removal on real temporary Git repositories. Use isolated HOME and config roots, state-based waits and retained failure diagnostics.
- A performance claim needs a measured comparison from `scripts/inventory-performance.py`, and a change of target needs review.
- A user-visible change updates the help text and the matching `docs/reference` page in the same change.

## Agent skills

### Issue tracker

Track issues and specs in GitHub Issues. Before issue operations, read `docs/agents/issue-tracker.md`.

### Triage labels

Use the five canonical triage labels. Before triaging, read `docs/agents/triage-labels.md`.

### Domain docs

Use a single-context layout. Before exploring the codebase, read `docs/agents/domain.md`.

### Design canvas

The Design canvas "Lager" is the working space for all design work: https://claude.ai/artifact/VqhtkQHMxKMFd6T2hN8X9p. Before drawing or changing a screen, a layout or UI wording, read `docs/agents/design-canvas.md`.
