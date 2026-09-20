# Lager

Lager is a stable Rust CLI and library for declarative local Git repositories.

- Keep domain and application services independent of terminal UI and subprocess details.
- Keep CLI and future TUI adapters on the same application services.
- Use native Git and shell processes where the product contract requires native output.
- Persist portable TOML through the symlink-aware config store; never persist machine-specific absolute paths.
- Keep tests at the binary boundary for user-visible behavior.

## Agent skills

### Issue tracker

Track issues and specs in GitHub Issues. Before issue operations, read `docs/agents/issue-tracker.md`.

### Triage labels

Use the five canonical triage labels. Before triaging, read `docs/agents/triage-labels.md`.

### Domain docs

Use a single-context layout. Before exploring the codebase, read `docs/agents/domain.md`.
