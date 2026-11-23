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
| `worktree_root` | No | None | Base directory for new worktrees: `~`, `~/relative`, or a relative path under `HOME`. Absolute paths and `..` are rejected. |
| `session` | No | None | The terminal host and the layout of new sessions. See [Session](#session). |

`root` resolves from `HOME`, never from the working directory. A root that does not exist yet is an empty inventory: `status` and `tui` say so in a note and exit `0`. `init` converts an absolute root under `HOME` to a portable `~` path before writing it.

Without `worktree_root`, `lager worktree add` places a worktree beside its primary checkout at `<primary>.worktrees/<NAME>`. With it, the worktree goes to `<worktree_root>/<primary relative to root>.worktrees/<NAME>`:

```toml
root = "~/repos"
worktree_root = "~/worktrees"
```

`lager worktree add org/project hotfix` then creates `~/worktrees/org/project.worktrees/hotfix`. A primary checkout outside `root` has no such path and needs `--path`. An invalid `worktree_root` fails every command when the configuration is loaded. You set the key by editing the file; configuration mutations keep it as written.

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
in TOML, the `origin` value of `status --json`, and Git clone arguments regardless of a `.git` suffix.
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

## Session

The `[session]` section turns on [`lager session`](cli.md#sessions). It names the terminal host and the layout every new session starts with. Without the section, lager calls no terminal host: the session commands and the session flags of `lager worktree` exit `1` and name the missing section, and every other command is unaffected. With it, `lager worktree list` adds a session column.

```toml
[session]
host = "rex"

[session.layouts.default]
focus = "Agents"

[session.layouts.default.tabs.Agents]
order = 1
root = { pane = "Agent I" }
splits = [{ source = "Agent I", pane = "Agent II", direction = "right" }]

[session.layouts.default.tabs.Shells]
order = 2
root = { pane = "Shell I" }
splits = [{ source = "Shell I", pane = "Shell II", direction = "down", ratio = 0.3 }]
```

| Field | Required | Default | Rules |
| --- | --- | --- | --- |
| `host` | Yes | — | `rex`. |
| `layouts.default` | Yes | — | The layout of every new session. |
| `layouts.default.focus` | No | None | The name of the tab a new session shows first. It has to name a tab. |
| `layouts.default.tabs.NAME` | At least one | — | A tab named `NAME`. |
| `…tabs.NAME.order` | No | `0` | Tabs open sorted by `order`, then by name. |
| `…tabs.NAME.root` | Yes | — | `{ pane = "PANE" }`: the pane the tab starts with. |
| `…tabs.NAME.splits` | No | Empty array | Splits applied in order, each `{ source, pane, direction, ratio }`. |
| `source` | Yes | — | The pane to split, the root or a pane an earlier split created. |
| `pane` | Yes | — | The name of the new pane, unique within its tab. |
| `direction` | Yes | — | Where the new pane goes: `right`, `left`, `down` (or `below`) or `up` (or `above`). |
| `ratio` | No | `0.5` | The new pane's share of the split pane's room, between 0 and 1, both excluded. |

Every pane starts a shell in the checkout and carries its name as its label. Lager uses the layout named `default`. The layout has the shape of Helm's layout configuration, so a Helm layout moves over once its tables sit under `session.`: Helm's `[layouts.default]` becomes `[session.layouts.default]`.

The section is validated whenever the configuration is loaded, by every lager command. An invalid `[session]` section is a configuration error for all of them, `status` and `tui` included, and they exit `1` at startup before doing anything. A running inventory that reloads it keeps the previous configuration, as for any configuration that fails to load. The layout errors name the setting at fault:

| Message | Cause |
| --- | --- |
| `session.layouts.default: is missing` | The section has no `default` layout. |
| `session.layouts.default: has no tabs` | The layout has no tab. |
| `session.layouts.default: focus names no tab "X"` | `focus` names a tab that does not exist. |
| `session.layouts.default: tab T: no pane "X"` | A split's `source` names no pane of tab `T` at that point. |
| `session.layouts.default: tab T: pane "X" is named twice` | A split's `pane` names a pane that tab `T` already has. |
| `session.layouts.default: ratio R is not between 0 and 1` | A `ratio` is 0, 1 or outside that range. |

`lager session` and `lager worktree` print these as `error: invalid config: MESSAGE`; `status` and the other repository commands print `lager: invalid config: MESSAGE`.

Unknown keys inside `[session]` are errors, unlike unknown keys at the top level, which only warn. A missing `host`, an unknown host, an unknown direction or an unknown key fails with the TOML parser's message, for example:

```text
error: could not parse config /home/dev/.config/lager/config.toml: session: unknown variant `tmux`, expected `rex`
error: could not parse config /home/dev/.config/lager/config.toml: session: unknown variant `diagonal`, expected one of `right`, `left`, `below`, `down`, `above`, `up`
```

Lager records which session it opened for which checkout in `sessions.json`, beside the configuration lock under `LAGER_CACHE_DIR` or `$HOME/.cache/lager`. The record belongs to this machine and is never written to the configuration.

## Inventory key maps

Shortcuts live under `[inventory.keys.<mode>]`. The modes are `normal`, `menu`,
`search`, `input`, `confirmation`, `inspection`, `worktrees` and `filter`. Omitted actions inherit their
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
`menu`, `mark`, `add`, `remove`, `worktrees` (default `w`), `lazygit` (default `g`), `filter` (default `f`), `session` (default `s`), `help` and `quit`. `lazygit` opens lazygit in the current row's checkout. `filter` opens the Filter popup. `open_sessions` (default `Ctrl+o`) ticks or clears the Session value `open` of the filter, and needs a [`[session]` section](#session). `dirty` (default `Ctrl+d`) ticks or clears the Changes value `dirty` of the filter. `session` opens the session of the current row and needs a [`[session]` section](#session); without one its key does nothing. `register`, `unregister`,
`clone`, `clone_register`, `remove_unregister`, `copy`, `open`, `remote`, `retry`
and `archives` have no default key. Bound to a key, each runs its menu entry on
the marked rows or the current row, with the same previews, eligibility and
outcomes; a disabled entry reports its reason instead. `copy` opens the Copy
value menu, `remote` the Remote discovery menu; `retry` fetches the failed
providers again and `archives` switches the archive preference. Menu uses navigation, paging, accept, cancel, help,
quit, Search, Inspect, Refresh and Clear search. It inherits those last four
normal shortcuts unless a menu override or a menu-local binding takes precedence.
Inspection uses navigation, paging, `copy` (default `c`), accept, cancel, help
and quit. Help popups use inspection
bindings to scroll and close while listing the invoking mode's bindings.
Menu, Inspect, Help and the batch preview default to Esc/q for cancel and Ctrl-Q
for quit. Normal
mode uses q/Esc for quit. To bind popup quit to q explicitly, also set
`cancel = ["Esc"]` in that mode so the actions do not collide.

Worktrees drives the list of the Worktrees popup: navigation, paging, `add`
(default `a`), `remove` (default `r`), `copy` (default `c`), `session` (default
`s`), accept and cancel, which both close it, help and quit, with the popup
defaults for those. `session` opens the session of the selected worktree and
needs a [`[session]` section](#session). Its name
form uses the input settings, its plan and its removal summary use the
confirmation settings, and its blocker view scrolls with the inspection
navigation keys and closes with the inspection accept and cancel keys.

Filter drives the Filter popup: navigation, paging, `toggle` (default Space),
which ticks or clears the value under the cursor, `clear_filter` (default `c`),
accept and cancel, which both close the popup and keep the filter, help and
quit, with the popup defaults for those.

Search uses accept, cancel, help and quit, with Enter, Esc, F1 and Ctrl-Q as
defaults. Its letters remain text. Confirmation settings drive the batch preview:
up and down move the cursor between Cancel and the action, page up and down
scroll the target list, accept chooses, cancel closes it, plus help and quit.
The removal confirmation adds `acknowledge` (default Space), which ticks the
loss acknowledgment, and the quit prompt of a running batch uses up, down,
accept and cancel. With a `[session]` section, `session` (default `s`) ticks or
clears the session line of a worktree's plan or removal summary.
Input settings drive the Add reference form: accept previews the reference,
cancel closes the form, plus help and quit, with Enter, Esc, F1 and Ctrl-Q as
defaults; its letters remain text. The Add preview uses the confirmation
settings: up and down move between its choices, accept chooses and cancel returns
to the reference.

Every mode is validated when the inventory starts and on every refresh. An
unknown mode or action name, a key bound to two actions in one mode, an explicit
empty list for accept, cancel, help or quit, and a typing-mode key that would
swallow ordinary text are errors. A collision names the mode, the key and both
actions:

```text
[inventory.keys.normal] binds x to remove and to clone
```

Fifteen defaults give way to an explicit key of the same mode instead of colliding:
`worktrees` (`w`), `lazygit` (`g`), `filter` (`f`), `open_sessions` (`Ctrl+o`), `dirty` (`Ctrl+d`) and `session` (`s`) in normal, `add` (`a`), `remove` (`r`),
`copy` (`c`) and `session` (`s`) in worktrees, `copy` (`c`) in inspection, and
`acknowledge` (Space), `session` (`s`), cancel's `q` and quit's Ctrl-Q in
confirmation. The
yielding action then has no key in that mode, and Help says so; Worktrees…, Filter… and
Open session in the menu still work. When Ctrl-Q goes to another confirmation action and quit has no
explicit list, quit moves to `q` and cancel keeps Esc; if `q` is taken as well,
the map is rejected because quit is unreachable.

At startup an invalid map stops the inventory before raw mode. On refresh (`R`)
the configuration and bindings are installed together or not at all: an invalid
map keeps the previous map active, the status line shows `! Config refresh
failed: … · previous bindings stay active`, and the failure is listed in
Problems and counted in the header.

Help lists the mode it was opened from with the effective keys, adds
`(config: action = [...])` next to each overridden action, shows one line per
other mode, and names the normal actions without a key. With a `[session]`
section, Help in normal mode adds a Sessions block that explains the `●` mark
and the `session` keys of the table and the confirmation, and the worktrees and
confirmation lines name their `session` key. It scrolls with the
inspection bindings and leaves the status line visible.

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
the previous map, shows the failure and makes the inventory exit 1.
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
