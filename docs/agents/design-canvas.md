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
