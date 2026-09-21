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
| `inventory` | No | Default bindings | Mode-aware inventory key maps. |

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

Accepted explicit SCP, SSH, HTTP(S), and file clone references retain their text
in TOML, `clone_url` JSON, and Git clone arguments regardless of a `.git` suffix.
Identity comparison and destination mapping remain transport-independent; an
equivalent registration does not replace the existing stored URL.
Shorthand and canonical IDs still materialize default SSH URLs. The sole browser
convenience is an HTTP(S) URL with exact `/projects/PROJECT/repos/REPO/browse`
path (optionally one trailing slash), converted to SSH user `git` on port `7999`.
`/archive`, `/browse/src`, and other neighboring paths retain literal transport.

Explicit URL schemes are limited to `http`, `https`, `ssh`, and `file`.
Repository references may not contain HTTP(S) userinfo, including username-only
tokens, passwords in any scheme, file-URL userinfo, or queries/fragments.
Ordinary SSH/SCP usernames (including `alice+ci`) remain supported; usernames
cannot contain whitespace, password/userinfo separators, path separators, or
percent escapes. SCP hosts must be nonempty and contain no whitespace or path
separators, and the clone path must be nonempty. All declarations are checked before
diagnostics can quote them or operations begin. Providers apply the same policy
to the selected clone link after archive filtering; unused links and excluded
archived repositories are not displayed, persisted, or validated as clone inputs.
Existing affected configuration fails with a generic diagnostic and is never
automatically rewritten. TOML syntax/type errors report line and column when
available, without source values that could expose credentials. Trusted
`post_clone` commands remain executable configuration, not a secret-scanning surface.

A canonical ID for a custom host requires a matching provider entry. Full clone URLs remain valid without a provider. GitHub and Bitbucket Cloud map `host/path` to `root/prefix/path`; custom hosts include the host in the destination. Data Center removes a leading `~` only from a personal-project path segment.

## Inventory key maps

Shortcuts live under `[inventory.keys.<mode>]`. The modes are `normal`, `menu`,
`search`, `input`, `confirmation` and `inspection`. Omitted actions inherit their
defaults; an explicit list replaces every default key for that action.

```toml
[inventory.keys.normal]
down = ["n", "Down"]
refresh = ["x"]

[inventory.keys.search]
accept = ["Ctrl+a"]
cancel = ["Ctrl+e"]

[inventory.keys.inspection]
page_down = ["x", "PageDown"]
```

Normal actions are `up`, `down`, `refresh`, `search`, `clear_search`, `inspect`,
`menu`, `help` and `quit`. Menu uses navigation, paging, accept, cancel, help,
quit, Search, Inspect, Refresh and Clear search. It inherits those last four
normal shortcuts unless a menu override or a menu-local binding takes precedence.
Inspection uses navigation, paging, accept, cancel, help and quit. Help popups use inspection
bindings to scroll and close while listing the invoking mode's bindings.
Menu, Inspect and Help default to Esc/q for cancel and Ctrl-Q for quit. Normal
mode uses q/Esc for quit. To bind popup quit to q explicitly, also set
`cancel = ["Esc"]` in that mode so the actions do not collide.

Search uses accept, cancel, help and quit, with Enter, Esc, F1 and Ctrl-Q as
defaults. Its letters remain text. Input and confirmation settings are reserved
for future dialogs. Known future normal actions can be configured but remain
unavailable until implemented.

Keys include single characters, Enter, Esc, arrows, Home, End, PageUp, PageDown,
Tab, Shift+Tab, Backspace, Delete, Space, F1 and Ctrl combinations. Ctrl-letter
case is equivalent. Terminal aliases are normalized before collision checks:
`Ctrl+i` is Tab, `Ctrl+m` is Enter, `Ctrl+[` / `Ctrl+3` is Esc, and `Ctrl+?` /
`Ctrl+8` is Backspace. `Ctrl+@` / `Ctrl+2` is `Ctrl+Space`; `Ctrl+\`, `Ctrl+]`,
`Ctrl+^` and `Ctrl+_` / `Ctrl+/` share the terminal encodings of `Ctrl+4` through
`Ctrl+7`. Unsupported Ctrl
combinations are rejected. Ctrl-C and Ctrl-Z are native controls and cannot be
rebound.

Unknown modes/actions, collisions, missing required help/quit/accept/cancel
routes, and bindings that consume ordinary text-editing keys in typing modes
are rejected. Startup rejects invalid maps before raw mode. A valid local
refresh swaps configuration and bindings together; an invalid refresh keeps
the previous map, shows the failure and contributes to session exit status 1.
Repository configuration mutations preserve inventory settings.

## Reads and writes

Unknown TOML keys produce one stderr warning per key per command, remain preserved through mutations, and never contaminate JSON stdout. Recognized invalid values fail before external operations. Semantic CLI usage errors are reported before reading configuration.

Warnings visibly escape control characters and literal backslashes in key names;
the stored keys and values are not rewritten for display safety.

A configuration may be a direct, relative, multi-hop, or dangling symlink. Mutations write the resolved target atomically while preserving the target permissions and the logical link. Lager serializes mutations with a lock under `LAGER_CACHE_DIR`, or `$HOME/.cache/lager` by default.

`init` refuses any existing logical configuration path, including a dangling
symlink, without creating its target or the requested root. Mutations require a
readable existing target; they do not repair dangling links.

CLI registration validates all inputs and the loaded configuration before asking
for hook choices outside the lock, then reads the latest
configuration under the lock, applies requests in order, validates the final
document, and writes once. A failed batch leaves original bytes, symlinks, and
target permissions unchanged; a no-op batch does not rewrite the file.

For Rust callers, `AtomicRegistrationStore` and `registry::register_batch` expose
this all-or-nothing capability. The original `ConfigStore::register` remains
available for uniform-hook requests, and `registry::register_many` retains its
legacy incremental behavior: earlier writes survive a later failure or cancellation.
Credential/query/fragment-sensitive reference failures abort an entire target
batch before effects, including unsupported-scheme userinfo. Ordinary malformed
`add`, `remove`, and `hook` targets fail independently while other targets continue.
The legacy incremental registration helper keeps earlier writes on an ordinary
malformed later reference; atomic CLI registration still validates every input
before any writes.

See [provider setup](../how-to/providers.md), [wildcards](../how-to/wildcards.md), and [automation](../how-to/automation.md) for operational use.
