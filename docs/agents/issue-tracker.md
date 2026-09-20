# Issue tracker: GitHub

Issues and specs live in GitHub Issues. Use the `gh` CLI, which infers the repository from the Git remote.

## Operations

- Create: `gh issue create --title "..." --body-file <file>`.
- Read: `gh issue view <number> --json number,title,body,labels,comments`.
- List: `gh issue list --state open --json number,title,body,labels,comments`. Use label and state filters as needed.
- Comment: `gh issue comment <number> --body-file <file>`.
- Apply labels: `gh issue edit <number> --add-label "..."`.
- Remove labels: `gh issue edit <number> --remove-label "..."`.
- Close: `gh issue close <number> --comment "..."`.

“Publish to the issue tracker” means create a GitHub issue.
“Fetch the relevant ticket” means read the issue, including labels and comments.

## Pull requests as a triage surface

**PRs as a request surface: no.**

If enabled, triage external PRs using the same labels and states as issues.
Use `gh pr` equivalents for reading, commenting, labelling, and closing;
use `gh pr diff <number>` to inspect changes.
Include authors with association CONTRIBUTOR, FIRST_TIME_CONTRIBUTOR, or NONE.

Issues and PRs share a number space. For an ambiguous reference, resolve
with `gh pr view <number>` and fall back to `gh issue view <number>`.

## Wayfinding operations

- Map: one issue labelled `wayfinder:map`, containing Notes, Decisions-so-far, and Fog.
- Child tickets: link as GitHub sub-issues. If unavailable, use a task list in the map and `Part of #<map>` in each child.
- Ticket types: `wayfinder:research`, `wayfinder:prototype`, `wayfinder:grilling`, or `wayfinder:task`.
- Blocking: use native GitHub issue dependencies. If unavailable, record `Blocked by: #<number>` in the child.
- Frontier: select the first open, unassigned child in map order whose blockers are all closed.
- Claim: `gh issue edit <number> --add-assignee @me` as the session's first write.
- Resolve: comment with the answer, close the ticket, then append a context pointer to the map's Decisions-so-far.
