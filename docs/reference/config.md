# Configuration reference

Lager stores portable TOML. The default path is `$HOME/.config/lager/config.toml`; select another file with `--config PATH` or `LAGER_CONFIG`.

```toml
root = "~/repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/my-org/project"
post_clone = "make setup"
```

`root` is the local destination base. Providers enable discovery and wildcard expansion. Repository declarations are processed in file order.

## Top-level fields

| Field | Required | Default | Rules |
| --- | --- | --- | --- |
| `root` | Yes | — | `~`, `~/relative`, or a relative path under `HOME`. Absolute paths, `..`, and escapes are rejected. |
| `providers` | No | Empty table | Provider entries are keyed by lowercase host names. |
| `repositories` | No | Empty array | Each item is an explicit repository or a trailing `/*` wildcard. |

`root` resolves from `HOME`, never from the working directory. `init` converts an absolute root under `HOME` to a portable `~` path before writing it.

## Provider fields

Provider entries live under `[providers."host"]`.

| Field | Required | Default | Rules |
| --- | --- | --- | --- |
| `preset` | Yes | — | `github`, `bitbucket-cloud`, or `bitbucket-data-center`. |
| `prefix` | No | `""` | Normalized relative path below the root. It cannot be absolute, `~`, `.`, or contain `..`. |
| `api_url` | Data Center only | None | HTTP(S) URL with a host and no userinfo. GitHub forbids it. |
| `auth` | Bitbucket only | `anonymous` | `anonymous`, `bearer`, or `basic`. GitHub uses authenticated `gh`. |
| `token_env` | Bearer only | None | Name of the environment variable containing the token. |
| `username_env` | Basic only | None | Name of the environment variable containing the username. |
| `password_env` | Basic only | None | Name of the environment variable containing the password. |
| `ssh_user` | No | None | Non-empty SSH username without `@`, `/`, or `:`. |
| `ssh_port` | No | None | Port from 1 through 65535. |

Authentication modes are complete combinations: `anonymous` has no credential fields; `bearer` has only `token_env`; `basic` has `username_env` and `password_env`. Lager reads secrets from the environment and never writes them to TOML.

### Provider presets

```toml
[providers."github.com"]
preset = "github"
```

GitHub discovery uses authenticated `gh`. `init --github` writes this provider with an explicit empty `prefix`.

```toml
[providers."bitbucket.org"]
preset = "bitbucket-cloud"
auth = "bearer"
token_env = "BITBUCKET_TOKEN"
```

```toml
[providers."git.example.com"]
preset = "bitbucket-data-center"
prefix = "company"
api_url = "https://git.example.com"
auth = "basic"
username_env = "BITBUCKET_USER"
password_env = "BITBUCKET_PASSWORD"
ssh_user = "git"
ssh_port = 7999
```

Bitbucket Cloud may use `api_url`. Data Center requires it. Both support anonymous, bearer, and basic authentication.

## Repository fields

Repository declarations use an array of TOML tables.

| Field | Required | Default | Rules |
| --- | --- | --- | --- |
| `url` | Yes | — | Canonical ID or full SCP, SSH, HTTP(S), or file clone URL. |
| `post_clone` | No | None | Shell command run after a fresh clone. Allowed only on explicit declarations. |
| `exclude` | No | Empty array | Provider-relative members excluded from a wildcard. Allowed only on wildcard declarations. |

```toml
[[repositories]]
url = "github.com/my-org/*"
exclude = ["retired"]
```

Only one trailing `/*` is valid. Wildcards require a configured provider and cannot have hooks. Explicit declarations cannot have exclusions. Equivalent declarations must have the same hook, exclusions, and wildcard status.

A canonical ID for a custom host requires a matching provider entry. Full clone URLs remain valid without a provider. GitHub and Bitbucket Cloud map `host/path` to `root/prefix/path`; custom hosts include the host in the destination. Data Center removes a leading `~` only from a personal-project path segment.

## Reads and writes

Unknown TOML keys produce stderr warnings, remain preserved through mutations, and never contaminate JSON stdout. Recognized invalid values fail before external operations.

A configuration may be a direct, relative, multi-hop, or dangling symlink. Mutations write the resolved target atomically while preserving the target permissions and the logical link. Lager serializes mutations with a lock under `LAGER_CACHE_DIR`, or `$HOME/.cache/lager` by default.

See [provider setup](../how-to/providers.md), [wildcards](../how-to/wildcards.md), and [automation](../how-to/automation.md) for operational use.
