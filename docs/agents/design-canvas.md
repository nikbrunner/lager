# Design canvas

The Design canvas "Lager" at https://claude.ai/artifact/VqhtkQHMxKMFd6T2hN8X9p is the working space for every screen Lager shows: the inventory TUI, its popups and the human output of commands. It is private to Nik and the single source of its boards.

A board fixes what a screen says and how it is laid out. The PRD and the reference docs fix what it does.

## Reading

Read the canvas with the Artifact tool: `read` with the URL, then single files by `path`.

- `SKILL.md` holds the instructions of the canvas type. Read it before changing anything.
- `project/canvas.json` lists every board with its title. Its notes hold the row titles and the open-proposal stickies.
- `project/term.css` carries the terminal palette (dark, light, no-color), the column widths and the popup spacing.
- `project/<Board>.dc.html` is one board: a static frame with fixture data.

## Designing

- A feature has a row of its own, and its boards share a prefix: `Wt*` for worktrees, `Gh*` for GitHub remote actions.
- A board is registered in `canvas.json` under `boards` with its position, size and title, and in `order`. A row is the boards that share a `y`; its title is a note above them.
- A terminal board links `./term.css` and uses its classes and the markup of a neighbouring board.
- An open design question is an orange proposal sticky below its board.
- A change is published once, to the same URL: the boards and the merged `canvas.json` together.

## Popups

A popup in the inventory holds a title, the table or list, and one row of key hints inside its frame. While it is open, the hint bar at the bottom of the screen stays empty and the status line above it shows the current state.

Sentences that explain the interface stay out of the popup. A rule shows when it applies: a key that the rule refuses answers with one line giving the reason, and Help lists the rule. Text that says what the user is about to lose stays in the popup: the losses of a removal and their acknowledgment, the typed name and the permanence line of a GitHub delete, lock states, and the outcome of a batch.
