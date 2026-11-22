# Ubiquitous Language

## Repositories

| Term | Definition | Aliases to avoid |
| --- | --- | --- |
| **Repository identity** | The normalized host and repository path, such as `github.com/org/project`, by which references are compared. | repo name, URL |
| **Declaration** | Portable configuration that names a repository or a wildcard set to keep available. | config entry, managed repo |
| **Wildcard** | A declaration that covers every repository a provider lists under a pattern, minus its exclusions. | glob, pattern rule |
| **Registration** | Whether a declaration covers an identity: explicit, through a wildcard, excluded from one, or unregistered. | managed, tracked |
| **Provider** | A source of repository catalogs: GitHub through `gh`, Bitbucket Cloud or Bitbucket Data Center. | host, forge |

## Local state

| Term | Definition | Aliases to avoid |
| --- | --- | --- |
| **Checkout** | A concrete local working directory of a repository. | clone (as a noun), local copy |
| **Primary checkout** | A repository's main working directory, as distinct from its linked worktrees. | main repo |
| **Worktree** | An additional working directory that native Git manages for a repository. | branch folder |
| **Inventory entry** | A declaration, a discovered primary or standalone checkout, or a remote candidate, shown as one row. | item |
| **Remote candidate** | A repository a provider lists that has no declaration and no checkout. | remote repo |

## Sessions

| Term | Definition | Aliases to avoid |
| --- | --- | --- |
| **Session** | A terminal session in the terminal host, with tabs and panes, opened for one checkout. | workspace |
| **Terminal host** | The terminal program that holds sessions; Rex is the supported one. | multiplexer, terminal |
| **Layout** | The tabs and panes a new session starts with, taken from the configuration. | template |
| **Label** | The name the terminal host shows for a session. | session name, title |

## Relationships

- **Registration** and **Checkout** state are independent: a **Declaration** can have no **Checkout**, and a **Checkout** can have no **Declaration**.
- One **Repository identity** can have several **Checkouts**, each with its own **Worktrees**.
- Removing a **Worktree** keeps its branch.
- One **Terminal host** is configured at a time, and a **Session** belongs to exactly one **Checkout**.
- Lager finds its own **Sessions** through its record on the local machine, never through a **Label**. A **Session** it did not create is unknown to it.
- Lager owns **Declarations**, operations, interface state and the session record. Git owns repository metadata and worktree bookkeeping. The **Terminal host** owns sessions, tabs, panes and focus.

## Example dialogue

> **Dev:** "A **Wildcard** covers `github.com/org/*`. Does every repository under it get a **Checkout**?"
> **Domain expert:** "Only after `add` or `ensure`. The **Wildcard** gives those identities a **Registration**; whether files exist is the **Checkout** state, and the two stay independent."
> **Dev:** "And a repository the **Provider** lists that the **Wildcard** excludes?"
> **Domain expert:** "With no **Declaration** and no **Checkout**, it shows as a **Remote candidate** while remote discovery is on."
> **Dev:** "If I open a **Session** for a **Worktree** and rename its **Label** in Rex, does Lager lose it?"
> **Domain expert:** "No. Lager finds it through its record, so the **Label** can change freely."

## Flagged ambiguities

- `docs/reference/cli.md` calls a repository identity a "canonical ID" ("Repository arguments accept canonical IDs"). Use **Repository identity** there too.
