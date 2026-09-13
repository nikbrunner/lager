# Configuration reference

Example:

```toml
root = "~/repos"

[providers."github.com"]
preset = "github"
prefix = ""

[[repositories]]
url = "git@github.com:my-org/project.git"
post_clone = "make setup"
```

The top-level `root` is required. It must be portable: `~`, `~/relative`, or a relative path under `$HOME`; absolute paths, `..`, and escapes are rejected. `root` resolves from `HOME`, not the current directory. `repositories` are processed in file order.

Provider fields are `preset`, `prefix`, `api_url`, `auth`, `token_env`, `username_env`, `password_env`, `ssh_user`, and `ssh_port`. Repository fields are `url`, `post_clone`, and `exclude`. Supported presets are `github`, `bitbucket-cloud`, and `bitbucket-data-center`.

Prefixes and exclusions are relative paths below their owner; empty exclusions, absolute paths, `~`, and `..` are rejected. Only trailing `/*` wildcards are valid. Wildcards require a configured provider, cannot have hooks, and are the only declarations that may have exclusions. Normalized duplicate declarations must have identical semantics. Canonical IDs for custom hosts require a matching provider entry, while full SCP, SSH, HTTP(S), and file clone URLs remain valid without one.

Provider authentication is validated as one complete combination: anonymous has no credential fields, bearer requires only `token_env`, and basic requires `username_env` plus `password_env`. Data Center requires an HTTP(S) `api_url`; API URLs cannot contain userinfo.

Unknown TOML keys produce stderr warnings, remain preserved through mutation, and do not corrupt JSON stdout. Recognized invalid values fail before external operations. Config symlinks and their target permissions are preserved during atomic writes; locks live under `LAGER_CACHE_DIR` when set.
