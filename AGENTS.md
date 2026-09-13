# Lager

Lager is a stable Rust CLI and library for declarative local Git repositories.

- Keep domain and application services independent of terminal UI and subprocess details.
- Keep CLI and future TUI adapters on the same application services.
- Use native Git and shell processes where the product contract requires native output.
- Persist portable TOML through the symlink-aware config store; never persist machine-specific absolute paths.
- Keep tests at the binary boundary for user-visible behavior.
