# Verify inventory in a disposable sandbox

This is the owner walkthrough for the inventory TUI and `status`. It covers
listing, search, Inspect and `status --json`; repository actions, providers and
native Git handoff have their own checklists in `HOW_TO_TEST.md`. Use an ordinary
macOS or Linux account and a real terminal. Human checkboxes remain unchecked
until the tester verifies them.

## Prepare

Build from the checkout using your normal HOME, then launch the sandbox:

```sh
cargo build --locked
bash scripts/inventory-sandbox.sh
```

The script uses the checkout's debug binary. It creates a temporary HOME,
configuration, 30 missing declarations and local clean, dirty, duplicate-origin,
misplaced, originless and Unicode-path fixtures. Git global/system configuration
is disabled and inventory uses only the sandbox root. The script prints the
config path for edits from a second terminal and removes the sandbox on exit.

For the no-color pass:

```sh
NO_COLOR=1 bash scripts/inventory-sandbox.sh
```

Record `target/debug/lager --version`, OS, terminal application/profile and
terminal dimensions with your results. Do not substitute the normal Lager
configuration for the printed sandbox config.

## Walkthrough

- [ ] At 80×24, navigate from the first row to the last with j/k and arrows. Resize wider and back. Confirm columns and whole-row selection stay aligned, the footer hints remain visible, and registration/checkout labels make sense without color.
  - Agent check: PASS — PTY overflow, resize, no-color labels and compact-hint tests; human light/dark contrast and the previously reported misalignment remain unverified.
- [ ] Search `/elsewhere` to select the misplaced alpha checkout. Enter and Esc retain the search; normal Backspace restores every row. Search `alpha`: both alpha rows remain distinct. Search the path `org/alpha`: only the declared alpha checkout remains. In Search, type `raRq`: letters change the query, not files, and no refresh starts.
  - Agent check: PASS — ranking, slash path search, typed-letter safety, retained searches and exact-path refresh tests.
- [ ] Open Inspect on each alpha row, the originless row, dirty checkout and wildcard pattern. Scroll to full paths, relationships and escaped warnings. Close with q or Esc to the same selected row; Ctrl-Q quits the app from the popup. Explicit alpha remains explicit even though a wildcard exclusion is also listed.
  - Agent check: PASS — full-value scrolling/resize, wildcard relationships, originless/conflict/unreadable facts and read-only fixture assertions.
- [ ] Open `m`, invoke Refresh through its menu entry, and verify the status line passes through scanning back to "No active operation" while the query and surviving selected path remain. From the menu, press the Search and Clear search shortcuts, then reopen the menu, enter a submenu and verify q/Esc returns to the parent menu and then closes only the menu. Open Help from Search, the menu and Inspect; it must describe the current mode and close back to it.
  - Agent check: PASS — menu/direct refresh, root return, internal menu scrolling, contextual help and delayed-probe interaction tests.
- [ ] In the printed sandbox config, add `[inventory.keys.normal]` with `down = ["n"]` and `refresh = ["x"]`. Press the currently active refresh key. Confirm n moves, j no longer moves, x refreshes, and Help/hints show the new map.
  - Agent check: PASS — binary binding replacement and atomic-refresh tests, including normalization of terminal-equivalent keys.
- [ ] Change that refresh list to `["n"]`, colliding with down. Press x. Expect a visible failure and the previous working map. Repair the list to `["x"]`, refresh successfully, then quit; the printed exit status remains 1 for this run.
  - Agent check: PASS — invalid refreshed maps retain working bindings and failures remain sticky after recovery.
- [ ] In fresh sandbox runs, quit with q and Esc, then interrupt with Ctrl-C. Confirm shell echo/input, cursor and the normal screen are restored; Ctrl-C prints exit 130.
  - Agent check: PASS — controlling-PTY SIGINT/SIGTERM/Ctrl-C restoration and active-descendant cleanup tests; prior ignored/custom signal actions are restored by the guard test.
- [ ] Repeat the navigation/Inspect pass with light and dark terminal profiles and NO_COLOR. Pay particular attention to wildcard blue, excluded amber, unregistered rows, long paths and `庫-café`. Capture any misalignment with a screenshot, dimensions and exact row/query.
  - Agent check: NOT RUN — human terminal/font/theme acceptance; ASCII PTY alignment is verified but does not rule out terminal-specific display defects.

## Regression and sign-off

- [ ] Run `lager status --json` against the same fixture and confirm its rows match the rows the inventory showed, in the same order. Do not run mutation checks on real repositories as part of this walkthrough.
  - Agent check: PASS — `status_prints_the_rows_the_inventory_screen_shows` compares the PTY rows with `status`; CLI, public-library and safe config-persistence regression suites.

| Tester/date | Build | Terminal/profile/size | Result or defect and capture |
| --- | --- | --- | --- |
| | | | |

Sign-off requires the agent checks above and this completed walkthrough.
