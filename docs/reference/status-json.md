# `status --json` reference

`lager status --json` (also `list --json` and `ls --json`) writes exactly one JSON object and a newline to stdout. Warnings and problems go to stderr, one line each. The rows are the rows `lager tui` shows for the same configuration and disk, after its scan has finished. They stay in row-key order (declarations by identity, then checkouts by identity), not in the screen's owner order. With `--remote`, `tui` also lists the providers' unregistered and excluded repositories as candidates; `status` prints the rows without them, so its remote rows are a subset of the screen's.

```json
{
  "schema_version": 2,
  "complete": true,
  "rows": [],
  "problems": []
}
```

| Key              | Type    | Meaning                                                                                                   |
| ---------------- | ------- | --------------------------------------------------------------------------------------------------------- |
| `schema_version` | integer | `2`.                                                                                                      |
| `complete`       | boolean | `false` when a source did not finish: the scan could not read part of the root, or a provider failed with `--remote`. The rows then show only what was read. A failed Git observation leaves it `true`, so `problems` can be non-empty while `complete` is `true`. |
| `rows`           | array   | One object per inventory row. `--registered` keeps the rows with a registration.                          |
| `problems`       | array   | Every failure that makes the command exit `1`. Empty on success.                                          |
| `notes`          | array   | Facts worth seeing that are not failures, in the shape of `problems`. Present only when there is one.     |

Keys appear in the order listed here. Enumerations are lowercase snake_case strings. A value Lager does not know is `null`; every key is present on every row.

## Rows

| Key               | Type             | Meaning                                                                                                                                                                       |
| ----------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `kind`            | string           | `declaration` (a repository at its configured destination: an explicit declaration, or with `--remote` a wildcard member without a checkout, whose `declaration` is `null`), `checkout` (a found checkout with a supported origin that is not at a matching explicit declaration's destination), `path` (a found checkout identified by its path), `pattern` (a wildcard declaration). |
| `identity`        | string or null   | The full normalized identity, such as `github.com/org/repo`. A `pattern` row holds the pattern, such as `github.com/org/*`. `null` for `path` rows.                                |
| `declaration`     | string or null   | The `url` of the explicit declaration of this identity, as written in the configuration file. A `pattern` row holds its own entry. `null` when no declaration names the identity. |
| `hook`            | string or null   | The declaration's `post_clone` command. `null` when the row has no declaration or the declaration has no hook.                                                                |
| `registration`    | string           | `explicit`, `wildcard`, `excluded`, `unregistered` or `pattern`.                                                                                                              |
| `pattern`         | string or null   | For `wildcard` and `excluded`, the wildcard declaration that covers or excludes the identity.                                                                                 |
| `exclusions`      | array or null    | On a `pattern` row, the members its `exclude` list names, possibly empty. `null` on every other kind.                                                                          |
| `checkout`        | string           | `cloned`, `missing`, `unknown`, `conflict`, `unreadable`, `remote_only` or `pattern`.                                                                                          |
| `found_path`      | string or null   | The absolute path where the scan found the checkout.                                                                                                                          |
| `configured_path` | string or null   | The absolute path the configuration assigns to the identity. A `pattern` row holds the directory its members go to.                                                           |
| `origin`          | fact             | The origin URL. See below.                                                                                                                                                    |
| `branch`          | fact             | The checked-out branch. See below.                                                                                                                                            |
| `changes`         | fact             | The working tree state. See below.                                                                                                                                            |
| `archived`        | boolean or null  | Whether the provider reports the repository as archived. `null` unless `--remote` asked a provider about this identity.                                                       |
| `warnings`        | array of strings | The row's warnings: misplaced checkouts with both paths, covering and excluding wildcards, `excludes NAME` on a pattern row, and identities taken from a path.                |

`checkout` is `unknown` when something other than a checkout sits at a configured destination, `conflict` when another repository's checkout sits there, and `remote_only` for a wildcard member the provider lists that has no local checkout and nothing at its destination. Paths are converted to UTF-8; a byte that is not valid UTF-8 becomes U+FFFD.

### Facts

`origin`, `branch` and `changes` are objects with a `state` and a `value`.

| Field     | `state`          | `value`                                                     |
| --------- | ---------------- | ----------------------------------------------------------- |
| `origin`  | `supported`      | The origin URL text as Git reports it.                      |
| `origin`  | `unsupported`    | The origin URL text; Lager cannot derive an identity from it. |
| `origin`  | `absent`         | `null`; the checkout has no `origin` remote.                |
| `origin`  | `error`          | The error from reading the origin.                          |
| `origin`  | `declared`       | The clone URL the declaration resolves to. No checkout was observed for the row, so this comes from the configuration. |
| `origin`  | `not_applicable` | `null`; a `pattern` row has no single origin.               |
| `branch`  | `known`          | The branch name, or `detached HEAD` followed by the short commit. |
| `branch`  | `error`          | The Git error.                                              |
| `branch`  | `not_applicable` | `null`; the row has no checkout to observe.                 |
| `changes` | `clean`          | `null`.                                                     |
| `changes` | `dirty`          | The change summary, such as `3 modified`: the number of `git status --porcelain` lines, untracked files included. |
| `changes` | `error`          | The Git error.                                              |
| `changes` | `not_applicable` | `null`; the row has no checkout to observe.                 |

## Identifying a row

`identity` repeats: a declaration and a misplaced checkout of the same repository, or two checkouts of one origin, are separate rows with the same identity. A row is identified by `kind` together with:

| `kind`        | Fields                          |
| ------------- | ------------------------------- |
| `declaration` | `identity` and `configured_path` |
| `checkout`    | `identity` and `found_path`     |
| `path`        | `found_path`                    |
| `pattern`     | `identity`                      |

`origin.value` is the URL text itself. `declaration` is the configuration text as written, which can differ from the normalized `identity`.

## Problems

| Key       | Type   | Meaning                                                                                                   |
| --------- | ------ | --------------------------------------------------------------------------------------------------------- |
| `source`  | string | `scan` for a path the scan could not read, `probe` for a failed Git observation, `provider` for a failed wildcard expansion. |
| `subject` | string | The absolute path for `scan` and `probe`, the provider host for `provider`.                               |
| `message` | string | The error as Lager received it.                                                                           |

A `scan` or `provider` problem sets `complete` to `false`; a `probe` problem does not. A provider failure leaves the rows from every other provider in place. A root that exists but is not a directory is a `scan` problem with the message `root is not a directory`.

## Notes

The one note is a root that does not exist yet: `{"source": "scan", "subject": "/home/dev/repos", "message": "root does not exist yet"}`. The scan is then complete and empty, `complete` stays `true`, the exit status stays `0`, and stderr repeats the note as `lager: ~/repos: root does not exist yet`. Consumers of schema 2 that do not know `notes` can ignore it.

## Remote members

With `--remote`, each wildcard declaration is expanded through its provider. Exclusions and explicit declarations take precedence over members. A member that already has a row gains its `archived` flag. A member without a row becomes a `declaration` row with `registration` `wildcard` and `checkout` `remote_only`. Archived members get a row of their own only with `--include-archived`.
