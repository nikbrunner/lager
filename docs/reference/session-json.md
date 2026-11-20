# Session JSON reference

`lager session create --json` and `lager session list --json` each write exactly one pretty-printed JSON object and a newline to stdout. Errors go to stderr, one line each, and leave stdout empty. This schema is separate from [`status --json`](status-json.md) and [`worktree list --json`](worktree-json.md).

## `create --json`

```json
{
  "schema_version": 1,
  "created": true,
  "path": "/home/dev/repos/acme/core",
  "id": "session:01a1f8aa-76be-721e-8f28-6e3ba611fe57",
  "label": "core",
  "tabs": [
    {
      "name": "Agents",
      "panes": {
        "Agent I": "block:01a1f8aa-7a1e-7c4b-9e2d-5b3f6c1d8062",
        "Agent II": "block:01a1f8aa-7a1e-7c4b-9e2d-5b3f6c1d79b4"
      }
    },
    {
      "name": "Shells",
      "panes": {
        "Shell I": "block:01a1f8aa-7a1e-7c4b-9e2d-5b3f6c1de476"
      }
    }
  ]
}
```

| Key | Type | Meaning |
| --- | --- | --- |
| `schema_version` | integer | `1`. |
| `created` | boolean | `true` when this call created the session, `false` when it existed. |
| `path` | string | The canonical path of the checkout the session belongs to. |
| `id` | string | Rex's session id. |
| `label` | string | The session's label as lager recorded it at creation. |
| `tabs` | array | The session's tabs in layout order: by `order`, then by name. |

Each tab has `name`, the tab's name from the layout, and `panes`, an object that maps each pane name to Rex's id for that pane, sorted by pane name. Pass a pane id to Rex to address that pane.

For a session that already existed, `created` is `false` and every value comes from the record lager wrote when it created the session. The pane ids are the ones Rex returned then. A pane closed or replaced in Rex since keeps its old id here, so a pane id of an existing session can be stale.

## `list --json`

```json
{
  "schema_version": 1,
  "sessions": [
    {
      "path": "/home/dev/repos/acme/core",
      "id": "session:01a1f8aa-76be-721e-8f28-6e3ba611fe57",
      "label": "core",
      "tabs": [
        {
          "name": "Agents",
          "panes": {
            "Agent I": "block:01a1f8aa-7a1e-7c4b-9e2d-5b3f6c1d8062"
          }
        }
      ]
    }
  ]
}
```

`sessions` holds one record per checkout with a live session, sorted by `path`, each with the keys `path`, `id`, `label` and `tabs` of `create --json`. Its pane ids come from the record as well and can be stale in the same way. With no sessions, `sessions` is an empty array.

Keys appear in the order listed here, and every key is present on every record. Values are original, with no escaping beyond JSON's own. Paths are converted to UTF-8; a byte that is not valid UTF-8 becomes U+FFFD.

## Exit status

| Situation | stdout | Exit |
| --- | --- | --- |
| The session was created, or existed (`create`); the listing succeeded (`list`) | The object | `0` |
| No `[session]` section, invalid configuration, Rex unavailable or failing, a missing checkout, or a record file that cannot be read or written | Nothing | `1` |
| Invalid arguments | Nothing | `2` |
