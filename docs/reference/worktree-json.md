# Worktree JSON reference

`lager worktree list [REPOSITORY] --json` and `lager worktree add … --json` each write exactly one JSON object and a newline to stdout. Diagnostics go to stderr, one line each. These schemas are separate from [`status --json`](status-json.md) and [session JSON](session-json.md).

## `worktree list --json`

```json
{
  "schema_version": 1,
  "worktrees": [
    {
      "repository_identity": "github.com/acme/core",
      "primary_path": "/home/dev/repos/acme/core",
      "path": "/home/dev/repos/acme/core.worktrees/pinned",
      "is_primary": false,
      "branch": null,
      "head": "9f2c1ab04d7e6b1f3a5c8d2e4f60718293a4b5c6",
      "locked": true,
      "lock_reason": "reviewing on laptop",
      "prunable": false,
      "prune_reason": null
    }
  ],
  "errors": []
}
```

| Key              | Type    | Meaning                                                                    |
| ---------------- | ------- | -------------------------------------------------------------------------- |
| `schema_version` | integer | `1`, or `2` with a [`[session]` section](config.md#session).               |
| `worktrees`      | array   | One record per working tree Git lists for the repository, sorted by `path`. |
| `errors`         | array   | Every failure that makes the command exit `1`. Empty on success.           |

Keys appear in the order listed here. Every key of the schema version is present on every record, so `session` appears on every record in version `2` and on none in version `1`.

### Records

The records include the primary checkout and every linked worktree, wherever it sits on disk and whichever tool created it.

| Key                   | Type           | Meaning                                                                                              |
| --------------------- | -------------- | ---------------------------------------------------------------------------------------------------- |
| `repository_identity` | string or null | The normalized origin, such as `github.com/acme/core`. `null` without an origin or with one Lager does not support. |
| `primary_path`        | string         | The absolute path of the primary checkout.                                                           |
| `path`                | string         | The absolute path of this working tree, as Git reports it.                                           |
| `is_primary`          | boolean        | `true` for the primary checkout.                                                                     |
| `branch`              | string or null | The full local ref, such as `refs/heads/hotfix`. `null` when HEAD is detached.                       |
| `head`                | string or null | The full Git object ID of HEAD. `null` on a branch without a commit.                                 |
| `locked`              | boolean        | Whether Git holds a lock on the worktree.                                                            |
| `lock_reason`         | string or null | The reason given with the lock. `null` without a lock or without a reason.                           |
| `prunable`            | boolean        | Whether Git considers the worktree prunable. Independent of `locked`.                                |
| `prune_reason`        | string or null | Git's reason, such as `gitdir file points to non-existent location`.                                 |
| `session`             | object, null or string | Schema version `2` only. The working tree's live session, `null` without one, or `"unknown"` when Rex could not answer. |

Values are original: no escaping beyond JSON's own. Paths are converted to UTF-8; a byte that is not valid UTF-8 becomes U+FFFD.

### Sessions

With a `[session]` section in the configuration, `schema_version` is `2` and every record ends with `session`. Without the section the document is schema version `1` and has no `session` key.

A session object has the keys `id`, `label` and `tabs` of a session in [`session list --json`](session-json.md#list---json):

```json
"session": {
  "id": "session:01a1f8ab-8ffc-76ae-836e-30d02892582e",
  "label": "core · Pinned",
  "tabs": [
    { "name": "Agents", "panes": { "Agent I": "block:01a1f8ab-9a0e-7f21-b6c4-2d8e5f7a1c30" } }
  ]
}
```

The command asks Rex once. When Rex is missing, stopped or fails, or lager cannot read its session record file, every record holds `"unknown"`, one `lager: warning:` line on stderr gives the reason, and the exit status stays what the listing makes it. A session lager cannot determine is never reported as `null`.

### Errors

| Key     | Type           | Meaning                                                    |
| ------- | -------------- | ---------------------------------------------------------- |
| `path`  | string or null | The path the failure concerns.                             |
| `code`  | string         | `git_failed`, `unreadable` or `invalid_metadata`.          |
| `error` | string         | A readable message. Its wording can change between versions. |

Errors are sorted by `path`, `null` first, then by `code`. Each one is repeated on stderr.

| Code               | When                                                                                                             |
| ------------------ | ---------------------------------------------------------------------------------------------------------------- |
| `git_failed`       | A Git command failed after the repository was found: the worktree listing or the origin lookup.                   |
| `unreadable`       | A listed worktree path, the repository's `worktrees` directory or one of its entries cannot be read.              |
| `invalid_metadata` | An entry under the repository's `worktrees` directory has no usable `gitdir` file, or Git's listing cannot be parsed. |

Git leaves worktree entries it cannot read out of its own listing. Lager reads the `worktrees` directory itself and reports each such entry, so a short list always comes with an error.

### Exit status

| Situation                                                    | stdout                                             | Exit |
| ------------------------------------------------------------ | -------------------------------------------------- | ---- |
| Listing succeeded                                            | The object with empty `errors`                     | `0`  |
| The repository was found and part or all of the listing failed | The object with the records that were read, possibly none | `1`  |
| Invalid configuration, or a repository that cannot be found   | Nothing                                            | `1`  |
| No `REPOSITORY` outside a repository, or invalid arguments    | Nothing                                            | `2`  |

A record never states a fact Lager could not read. When the origin lookup fails, `repository_identity` is `null` and `errors` names the failure.

## `worktree add --json`

```json
{
  "schema_version": 2,
  "path": "/home/dev/repos/acme/core.worktrees/hotfix",
  "branch": "hotfix",
  "new_branch": true,
  "upstream": null,
  "ahead": null,
  "behind": null,
  "session": {
    "created": true,
    "id": "session:01a1f8ab-8ffc-76ae-836e-30d02892582e",
    "label": "core · Hotfix",
    "tabs": [
      {
        "name": "Agents",
        "panes": {
          "Agent I": "block:01a1f8ab-9a0e-7f21-b6c4-2d8e5f7a1c30",
          "Agent II": "block:01a1f8ab-9a0e-7f21-b6c4-2d8e5f7a2b14"
        }
      }
    ]
  }
}
```

| Key              | Type           | Meaning                                                                                  |
| ---------------- | -------------- | ---------------------------------------------------------------------------------------- |
| `schema_version` | integer        | `2`.                                                                                     |
| `path`           | string         | The absolute path of the new worktree, as the `Created worktree` line names it.          |
| `branch`         | string         | The checked-out branch's short name, such as `hotfix` or `dev/spike`.                    |
| `new_branch`     | boolean        | `true` when the command created the branch, including a local branch created from a remote branch; `false` for an existing local branch. |
| `upstream`       | string or null | The remote-tracking branch, such as `origin/feat/x`: the one a new branch tracks, or the one an existing branch is compared with. `null` for a new branch without one and for an existing branch that has none. |
| `ahead`          | integer or null | Commits only on `branch`, compared with `upstream`. `0` for a branch created from `upstream`. `null` when `upstream` is `null`. |
| `behind`         | integer or null | Commits only on `upstream`, compared with `branch`. `0` for a branch created from `upstream`. `null` when `upstream` is `null`. |
| `session`        | object or null | The worktree's session with `--open-session` or `--create-session`; `null` without them. |

The session object has the keys of [`session create --json`](session-json.md#create---json) except `schema_version` and `path`: `created`, `id`, `label` and `tabs`. `created` is `false` when a session for the same path was still running, and its pane ids then come from lager's record and can be stale.

Keys appear in the order listed here, and every key is always present. Git's own output, `HEAD is now at …` and the output of `--fetch` included, goes to stderr for this run, so stdout holds the document alone.

| Situation                                                          | stdout       | Exit |
| ------------------------------------------------------------------ | ------------ | ---- |
| The worktree was created, and its session when a flag asked for it | The object   | `0`  |
| The worktree was created and its session failed                    | Nothing      | `1`  |
| A refused request, a failed Git command or a missing `[session]` section | Nothing | `1`  |
| Invalid arguments                                                  | Nothing      | `2`  |
