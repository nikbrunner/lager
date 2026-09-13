# Configure Lager

Set `root` and optional providers in TOML. Pass `--config PATH` for a one-off file, set `LAGER_CONFIG` for a session, or use the default `$HOME/.config/lager/config.toml`. See the [configuration reference](../reference/config.md) for every field and portability rule.

A config may be linked into place. Lager follows direct, relative, multi-hop, and dangling symlinks safely; successful mutations preserve the logical link and target permissions. Set `LAGER_CACHE_DIR` to isolate lock files in tests or parallel disposable environments.
