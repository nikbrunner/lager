# Repository-management acceptance checklist

For the inventory TUI, use the [inventory acceptance walkthrough](docs/how-to/verify-inventory.md).

## Test context

- Build: the binary under test. Record `lager --version` with
  the acceptance results.
- Surface: CLI, its terminal prompts and picker, the inventory TUI for worktrees and remote discovery, and the Rex app for sessions; no browser.
- Persona: ordinary local user, not root. Use only disposable repositories.
- Agent platform: macOS ARM64. Linux evidence is outstanding.
- Change: atomic registration, URL retention, guarded removal, child-result cancellation,
  terminal-safe human fields, config/provider acceptance evidence, worktree
  commands and the Worktrees popup, remote discovery in the inventory, and the
  session commands.
- Local evidence: formatting, warning-denying Clippy, the locked all-target suite
  (516 tests), and the release build passed on macOS ARM64.
- Boundary: native OS/shell signal and job-control behavior is unchanged.
  Custom signal forwarding and universal signal-driven terminal restoration are
  deferred; this checklist does not claim those guarantees.

## Entry criteria

- [ ] Build using your **normal HOME**, before creating the isolated runtime environment; expect all build tools to use the existing installation, not download a new toolchain.
  - Agent check: PASS — `cargo build --locked`, `cargo fmt --check`, and `cargo clippy --locked --all-targets --all-features -- -D warnings` on macOS ARM64.

```sh
# Run from the project root, with your normal HOME and toolchain.
cargo build --locked
BIN="$(pwd)/target/debug/lager"
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/lager-acceptance.XXXXXX")"
mkdir -p "$SANDBOX/home"
CONFIG="$SANDBOX/config.toml"
export BIN SANDBOX CONFIG

# Only the compiled program gets the isolated HOME. Do not run cargo or mise
# with the sandbox HOME; use the absolute binary rather than a Lager shim.
lager() {
  env HOME="$SANDBOX/home" LAGER_CONFIG="$CONFIG" \
    LAGER_CACHE_DIR="$SANDBOX/cache" GIT_CONFIG_NOSYSTEM=1 \
    GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 "$BIN" "$@"
}
fixture_git() {
  env HOME="$SANDBOX/home" GIT_CONFIG_NOSYSTEM=1 \
    GIT_CONFIG_GLOBAL=/dev/null git "$@"
}
printf 'Sandbox: %s\nBinary: %s\n' "$SANDBOX" "$BIN"
```

- [ ] Confirm Git is installed; use an existing `fzf` only for picker checks. Run `lager --version` and `lager --help`; expect the checkout build and the visible `ls` alias. Do not use real accounts or repositories.
  - Agent check: PASS — compiled-binary sandbox smoke used local Git and isolated HOME/config; existing alias tests cover command parity.

## Critical user journeys

- [ ] Initialize with `lager init --root repos --create-root --no-github </dev/null`; expect exit 0, a portable `root = "repos"` in `$CONFIG`, and `$SANDBOX/home/repos`. Repeat the command; expect exit 1 without overwriting the file.
  - Agent check: PASS — `providers_e2e::init_persists_portable_root_and_github_provider` and sandbox compiled-binary init smoke.

- [ ] Register a batch with `lager register org/first invalid </dev/null`; expect exit 1 and no `org/first` declaration. Then run `lager register org/first org/second </dev/null` twice; expect `added` first and `already managed` on retry.
  - Agent check: PASS — `registration_e2e` atomic/duplicate cases and compiled-binary batch rejection smoke.

- [ ] Run `lager status --json > "$SANDBOX/status.json"`, `lager list --json > "$SANDBOX/list.json"` and `lager ls --json > "$SANDBOX/ls.json"`; compare with `cmp`. Expect identical JSON, two missing declarations, no decorative output. Run `lager status` again in the same shell; declarations persist.
  - Agent check: PASS — `status_e2e::status_list_and_ls_print_identical_bytes_and_exit_codes` ran on 2026-11-13 (macOS ARM64) and compares the three names byte for byte in table and JSON.

Create a local clone fixture and remove the placeholder declarations:

```sh
lager unregister org/first org/second </dev/null
fixture_git init -q "$SANDBOX/source"
printf 'acceptance fixture\n' > "$SANDBOX/source/README"
fixture_git -C "$SANDBOX/source" add README
fixture_git -C "$SANDBOX/source" -c user.name=Fixture \
  -c user.email=fixture@example.invalid commit -qm initial
fixture_git clone -q --bare "$SANDBOX/source" "$SANDBOX/remote.git"
REF="file://$SANDBOX/remote.git"
export REF
```

- [ ] Run `lager add "$REF" --register --post-clone 'printf "fixture hook\n"' </dev/null`; expect a checkout at `$SANDBOX/home/repos/remote`, native Git output and one hook message. Run `lager ensure`, then `lager hook "$REF"`; ensure is a no-op and explicit hook runs again.
  - Agent check: PASS — existing `task5_e2e`/`task6_e2e` cover fresh-clone hooks and ensure idempotence; `terminal_output_e2e` preserves raw native child streams.

- [ ] Run `lager remove "$REF" --keep-registered` in a real terminal and answer no to removal; expect checkout and declaration to survive. Then use `lager remove "$REF" --keep-registered --yes --force </dev/null`; only the sandbox checkout disappears. `lager ensure` recreates it from the local remote.
  - Agent check: PASS — existing removal tests preserve skipped disk/config state; focused removal suite proves bounded force and exact selection.

## Worktrees

These steps use the `remote` checkout from the journeys above; recreate it with `lager ensure` if a removal step left it missing. Never point `lager worktree` at a repository outside the sandbox.

```sh
PRIMARY="$SANDBOX/home/repos/remote"
export PRIMARY
```

- [ ] Run `lager worktree list "$PRIMARY"`; expect one line with the primary path, branch and `primary`. Run `lager worktree add "$REF" review </dev/null`; expect Git's own output and `Created worktree $PRIMARY.worktrees/review on new branch review`. List again, then with `--json`; expect two records, the new one with `"branch": "refs/heads/review"` and `"is_primary": false`.
  - Agent check: PASS — run in a sandbox on macOS ARM64 with the debug binary (the primary's branch was Git's default `master`); `worktree_e2e` (80 tests) covers branch modes, placement, collisions and JSON failures.

- [ ] Run `lager worktree add "$REF" review </dev/null` a second time; expect exit 1 with `branch already exists: review; --branch checks it out`, and nothing new on disk. Run `lager worktree add "$REF" spike --path "$(cd "$SANDBOX" && pwd -P)/spike" --start HEAD </dev/null`; expect the worktree at the explicit path and the configuration file unchanged. The physical path matters on macOS, where `/var` is a symlink and a destination below a symlink is refused.
  - Agent check: PASS — run in the same sandbox.

- [ ] Add a file with `printf 'draft\n' > "$PRIMARY.worktrees/review/notes.txt"` and run `lager worktree remove "$PRIMARY.worktrees/review" </dev/null`; expect exit 2 because a non-interactive removal needs `--yes`. Run it with `--yes`; expect exit 1 with `losses    1 untracked file`. Run it with `--yes --force`; expect the directory gone and `git -C "$PRIMARY" branch --list review` still showing the branch.
  - Agent check: PASS — run in the same sandbox.

- [ ] In a real terminal, open `lager tui`, select the `remote` row and press `w`. Expect a popup listing the primary checkout and `spike` with path, branch and state. Press `a`, type `popup`, press Enter; expect the planned path `$PRIMARY.worktrees/popup` and a new branch, and nothing on disk before "Create worktree" is chosen. Create it, then select it, press `r` and choose Cancel; it stays. Press `r` again and remove it. On a missing or wildcard row, `w` reports that worktrees are unavailable.
  - Agent check: PASS — `inventory_worktree_popup_*` PTY tests (14) cover listing, copy, add through plan, removal with losses, blockers, unavailable rows, keys typed before a confirmation, and Ctrl-C during Git. Real-terminal acceptance remains for the tester.

## Remote discovery

A stub `gh` stands in for GitHub; no network or account is used.

```sh
mkdir -p "$SANDBOX/tools"
cat > "$SANDBOX/tools/gh" <<'SH'
#!/bin/sh
sleep 2
case "$*" in
  *viewer*) owner=me; field=viewer ;;
  *) owner=org; field=organization ;;
esac
printf '[{"data":{"%s":{"repositories":{"nodes":[{"name":"catalog","isArchived":false,"sshUrl":"git@github.com:%s/catalog.git","owner":{"login":"%s"}}],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]' "$field" "$owner" "$owner"
SH
chmod +x "$SANDBOX/tools/gh"
cat > "$SANDBOX/remote.toml" <<'TOML'
root = "repos"

[providers."github.com"]
preset = "github"

[[repositories]]
url = "github.com/org/*"
TOML
remote_lager() { PATH="$SANDBOX/tools:$PATH" lager --config "$SANDBOX/remote.toml" "$@"; }
```

- [ ] Run `remote_lager tui --remote`. Expect the local rows at once and a source line with `github.com: loading…` while navigation and `/` search keep working; after a few seconds (the stub answers each of its two requests after two seconds) it reads `github.com: ready, 2 candidates` and the catalog's repositories appear as candidates with checkout `remote only`. Run `remote_lager status --remote` in another shell; every row it prints is on the screen with the same cells.
  - Agent check: PASS — `inventory_remote_keeps_local_rows_usable_while_providers_load_and_keeps_rows_beside_a_failure` and `inventory_remote_rows_agree_with_status_remote` against stubbed providers.

- [ ] Open `m` → Remote discovery…; expect the provider with its state and the entries Retry failed providers, Refresh all enabled providers, Include archived and Disable remote. Choose Disable remote; the candidates disappear and the source line reads `remote off` with the cached count. Quit.
  - Agent check: PASS — `inventory_remote_menu_retries_only_the_failed_provider`, `inventory_remote_refresh_keeps_cached_rows_stale_while_every_provider_reloads` and `inventory_remote_disable_hides_candidates_removes_their_marks_and_keeps_the_cache`.

- [ ] Run `remote_lager tui` without `--remote` and press `a`. Expect the Add form and the source line switching to `github.com: loading…`. Press Esc; remote discovery stays on and the candidates appear once the stub answers.
  - Agent check: PASS — `inventory_opening_add_enables_remote_and_cancelling_add_leaves_it_on` and `inventory_opening_add_from_the_menu_enables_remote_and_without_providers_asks_nothing`.

## Sessions

These steps use your real Rex, because they check what it shows. They create throwaway sessions for the sandbox `remote` checkout (`$PRIMARY` from Worktrees) and for its worktree `topic`, labelled `remote` and `remote · Topic`, and close them at the end. Rex must be running, with its app open.

The wrapper keeps your real `HOME`, unlike `lager` above: lager finds the bundled `rex` under `HOME`, and Rex may find its server through it. Only the configuration and the session record are sandboxed. Name the checkout by path, never by reference.

```sh
cat > "$SANDBOX/session.toml" <<'TOML'
root = "repos"

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
TOML
session_lager() {
  env LAGER_CONFIG="$SANDBOX/session.toml" LAGER_CACHE_DIR="$SANDBOX/cache" "$BIN" "$@"
}
```

- [ ] From a shell outside the Rex session you want to test, run `session_lager session open "$PRIMARY"`. Expect `Opened session remote` and exit 0. Rex comes to the front, showing a new session `remote` on the `Agents` tab with `Agent I` on the left and `Agent II` on the right, and a second tab `Shells` with `Shell I`. Run `pwd -P` in `Agent II` and in `Shell I`; both print the physical path of `$PRIMARY` (under `/private/var` on macOS).
  - Agent check: NOT RUN — `open` moves the Rex view. `session_e2e::open_creates_once_and_selects_on_the_newest_connected_app` and `create_makes_one_session_with_the_configured_layout` pass against a fake `rex`.

- [ ] Switch Rex to another session, then run `session_lager session open "$PRIMARY"` again. Expect `Opened session remote` and Rex back on the same `remote` session, with no second one. `session_lager session list` prints one line: `remote`, the `$PRIMARY` path and a `session:` id.
  - Agent check: NOT RUN — moves the Rex view. `open_creates_once_and_selects_on_the_newest_connected_app` (one create, a select per open) passes against the fake.

- [ ] Turn off Rex's Remote Control setting. Close the session in Rex by hand, then run `session_lager session open "$PRIMARY"`. Expect Rex not to switch. Run `echo $?` and record what `open` printed and its exit status in the defect log. `session_lager session list` shows a new `remote` session. Turn Remote Control on again and repeat `open`; expect `Opened session remote` and still one session in the list.
  - Agent check: NOT RUN — needs the Rex app with Remote Control off. `open_without_an_app_fails_on_a_running_host` passes against the fake for an app that cannot select.

- [ ] Quit the Rex app, start it again from the Dock, and open a pane in any session. Set the two values the entry block printed, `SANDBOX=…` and `BIN=…`, then run `env -u REX_BIN_DIR PATH=/usr/bin:/bin LAGER_CONFIG="$SANDBOX/session.toml" LAGER_CACHE_DIR="$SANDBOX/cache" "$BIN" session create "$SANDBOX/home/repos/remote"`. Expect exit 0 with `Session remote already exists`, or `Created session remote` if the session did not survive the restart, and Rex staying where it is. No `rex is not installed`: lager finds the bundled `rex` without `PATH`.
  - Agent check: NOT RUN — needs Rex restarted from the Dock; the lookup's bundled path cannot be sandboxed in tests.

- [ ] In a pane of the `remote` session, set `SANDBOX=…` and `BIN=…` as before and run `env LAGER_CONFIG="$SANDBOX/session.toml" LAGER_CACHE_DIR="$SANDBOX/cache" "$BIN" session close .`. Expect the whole session to close, the pane with it, so the `Closed session remote` line is never seen. From another shell, `session_lager session list` prints nothing, and Rex no longer lists `remote`.
  - Agent check: NOT RUN — closes the session the command runs in. `close_ends_the_session_and_forgets_it` passes against the fake; the record is saved before Rex is asked to close.

- [ ] From a shell outside the Rex session you want to test, run `session_lager worktree add "$PRIMARY" topic --open-session </dev/null`. Expect Git's output, then `Created worktree …/remote.worktrees/topic on new branch topic` and `Opened session remote · Topic`, and exit 0. Rex comes to the front on a new session `remote · Topic`; `pwd -P` in `Agent I` prints the physical path of `$PRIMARY.worktrees/topic`. `session_lager worktree list "$PRIMARY"` shows `remote · Topic` at the end of the `topic` row.
  - Agent check: NOT RUN — moves the Rex view. `worktree_e2e::add_open_session_creates_the_session_and_shows_it` and `list_shows_each_worktrees_session_once_sessions_are_configured` pass against a fake `rex`.

- [ ] From a shell outside that session, run `session_lager worktree remove "$PRIMARY.worktrees/topic" --yes --close-session`. Expect the removal summary, `Removed worktree … · branch topic kept` and `Closed session remote · Topic`, exit 0, and the session gone from Rex.
  - Agent check: NOT RUN — needs the real Rex. `remove_close_session_destroys_only_after_a_successful_removal` passes against the fake; its log shows the close after Git's removal and none after a refused one.

The inventory steps run the sandbox inventory with your real Rex and a sandbox `HOME`. Run them in a Rex pane outside the `remote` session: there `rex` is on `PATH` and `REX_SERVER` names your server, so lager finds both without your `HOME`. `rex is not installed` or `Sessions unavailable` in the first step means the pane lacks those variables, not that lager failed.

```sh
session_tui() {
  env HOME="$SANDBOX/home" LAGER_CONFIG="$SANDBOX/session.toml" LAGER_CACHE_DIR="$SANDBOX/cache" "$BIN" tui "$@"
}
```

- [ ] Run `session_tui`, select the `remote` row and press `s`. Expect `Opening session remote…` in the status line, then the inventory closes and the shell prints `Opened session remote` as its last line. `echo $?` prints `0`. Rex comes to the front on a new session `remote`.
  - Agent check: NOT RUN — moves the Rex view. `inventory_e2e::inventory_s_opens_the_session_closes_and_names_it` and `inventory_answers_keys_while_a_session_opens` pass against a fake `rex`.

- [ ] Go back to the first pane and run `session_tui` again. Once the table settles, expect `●` between the mark and `remote`, and `s session` in the footer. Press `s`. Expect `Switched to session remote`, Rex back on the same session, and one `remote` line in `session_lager session list`.
  - Agent check: NOT RUN — moves the Rex view. The second run of `inventory_s_opens_the_session_closes_and_names_it` passes against the fake.

- [ ] Look at the session screens at 132×32 and at 80×24, each in a dark theme, a light theme and with `NO_COLOR=1`. Compare them with the canvas boards: the table and footer with SesMain, `m` on `remote` with SesMenu, `?` with SesHelp, and `lager tui`, whose configuration has no `[session]`, with SesOff. Then run `env REX_SERVER="unix://$SANDBOX/none.sock" session_tui` and compare with SesNoRex: expect the status line `Sessions unavailable: REX_SERVER: no Rex server is running at …`, no `●` and no `s session` hint. `s` there prints `Open session unavailable: …`, and `q` exits `0`.
  - Agent check: NOT RUN — needs eyes. `inventory_says_once_that_sessions_are_unavailable_and_s_repeats_it`, `inventory_explicit_s_wins_over_the_session_key` and `inventory_without_a_session_section_shows_and_asks_nothing_about_sessions` pin the text against the fake.

- [ ] Clean up if a step failed before the close: `session_lager session close "$PRIMARY"`. Expect `Closed session remote` or `No session for …`, and no `remote` session left in Rex. If the `topic` worktree is still there, run `session_lager worktree remove "$PRIMARY.worktrees/topic" --yes --force --close-session`.
  - Agent check: NOT RUN — needs the real Rex.

## Important edge cases

- [ ] In the sandbox, run `lager init </dev/null`; expect usage exit 2 without a prompt. Create a dangling link using `ln -s missing.toml "$SANDBOX/dangling.toml"` and run `lager --config "$SANDBOX/dangling.toml" init --root another --create-root --no-github </dev/null`; expect exit 1, the link unchanged, no target file and no `another` root.
  - Agent check: PASS — `config_e2e::non_tty_init_requires_each_omitted_choice_without_prompting` and `init_refuses_dangling_config_symlink_without_creating_target_or_root`.

- [ ] Verify precedence using the commands below; expect `org/explicit`, then `org/environment`, then `org/home`, respectively.
  - Agent check: PASS — `config_e2e::config_precedence_is_explicit_then_environment_then_home_default`.

```sh
mkdir -p "$SANDBOX/home/.config/lager"
printf "root = 'repos'\n[[repositories]]\nurl = 'org/home'\n" \
  > "$SANDBOX/home/.config/lager/config.toml"
printf "root = 'repos'\n[[repositories]]\nurl = 'org/environment'\n" \
  > "$SANDBOX/environment.toml"
printf "root = 'repos'\n[[repositories]]\nurl = 'org/explicit'\n" \
  > "$SANDBOX/explicit.toml"
env HOME="$SANDBOX/home" LAGER_CONFIG="$SANDBOX/environment.toml" \
  "$BIN" --config "$SANDBOX/explicit.toml" list
env HOME="$SANDBOX/home" LAGER_CONFIG="$SANDBOX/environment.toml" "$BIN" list
env -u LAGER_CONFIG HOME="$SANDBOX/home" "$BIN" list
```

- [ ] Create a dependent checkout with `fixture_git -C "$SANDBOX/home/repos/remote" worktree add --detach "$SANDBOX/dependent"`; run `lager remove "$REF" --yes --force --unregister </dev/null`. Expect exit 1, a dependent-worktree diagnostic, both checkouts and the declaration intact. Remove only the disposable dependent using `fixture_git -C "$SANDBOX/home/repos/remote" worktree remove "$SANDBOX/dependent"` before continuing.
  - Agent check: PASS — `repository_removal_e2e` covers live/external/locked/stale/malformed/unreadable dependent metadata and final revalidation.

- [ ] If `fzf` is installed, run `lager remove --keep-registered --yes --force </dev/null` and select the sandbox `remote` row. Expect exactly that checkout to disappear, its declaration retained, and `lager ensure` to recreate it. If a selected path disappears while the picker is open, expect failure rather than fallback deletion or unregister.
  - Agent check: PASS — captured fake-fzf tests prove safe exact-candidate mapping; `vanished_exact_picker_checkout_never_falls_back_or_unregisters_same_origin` preserves the other clone and config.

## UX and terminal-safety pass

Create a separate display-only config; do not clone from this fixture:

```sh
cat > "$SANDBOX/display.toml" <<'TOML'
root = "repos\n\r\t\u001b\u007f\u0085end"
"future\n\u001b" = true
[[repositories]]
url = "org/repo"
TOML
```

- [ ] Run `lager --config "$SANDBOX/display.toml" status`; expect exit 0, a header, one repository row, no raw tab, one warning line, and one note line for the root, which does not exist yet. The field text shows `\n`, `\r`, `\t`, `\x1b`, `\x7f`, `\x85`; no cursor movement, terminal title change, or injected row. Run with `--json`; a JSON parser must recover the original control characters in the destination, not the human escape notation.
  - Agent check: PASS — `terminal_output_e2e::human_rows_and_unknown_keys_escape_controls_while_json_preserves_values` ran on 2026-11-13 (macOS ARM64) and covers every C0/C1/DEL value, all three unknown-key warning levels, exact line counts, and raw JSON field values.

- [ ] In a real terminal, run `lager --config "$SANDBOX/prompt.toml" init`; press Enter for the default root and answer no to root creation and GitHub setup. Expect readable prompts, normal keyboard control, and the stored root `repos`. Cancel a new registration prompt with Ctrl-C; check the terminal still accepts normal input.
  - Agent check: PASS — safe input/default and confirmation PTY tests, registration prompt cancellation, and the full local suite pass; real-terminal acceptance remains for the tester.

- [ ] Edit the display fixture root to `"repos\\x1bend"` and list again; expect `repos\\x1bend` visibly distinct from a real ESC rendered as `\x1b`. Confirm long escaped labels remain understandable in your terminal.
  - Agent check: PASS — `literal_escape_notation_cannot_collide_with_an_exact_path_control` selects the intended unchanged candidate; terminal layout/accessibility still needs human judgment.

## Regression smoke test

- [ ] Register `https://github.com/org/transport` without cloning, then inspect `lager status --json`; expect HTTPS retained verbatim. Try both `https://token@github.com/org/rejected` and `https:///token@github.com/org/rejected`; expect redacted errors without persisting or echoing the token. Unregister `org/transport` afterward.
  - Agent check: PASS — `references_e2e` covers transport retention, actual clone argv, credential rejection before effects, and malformed loaded configuration.

- [ ] Create a separate config with `printf '# fixture\nroot = 42\n' > "$SANDBOX/invalid.toml"` and run `lager --config "$SANDBOX/invalid.toml" list --json`; expect exit 1, empty stdout, and a line-2 error location on stderr without the source value.
  - Agent check: PASS — `config_e2e::invalid_toml_reports_a_location_without_exposing_values` covers syntax/type errors, locations, and secret redaction.

- [ ] For an ordinary non-root user, repeat a removal with unreadable Git worktree metadata only in a disposable checkout; expect failure, no deleted checkout, and no removed declaration. Restore permissions before cleaning up.
  - Agent check: PASS — `repository_removal_e2e::unreadable_worktree_metadata_fails_closed` verified a genuine permission-denied probe on macOS.

Provider acceptance uses isolated loopback HTTP fixtures, not live accounts:
`providers_e2e::cloud_bearer_and_basic_environment_credentials_succeed_without_persistence`
passed for both auth modes and subsequent config mutation; existing Data Center
auth/pagination tests passed. No real provider credentials were supplied or saved.

## Exploratory prompts

- [ ] Try empty, repeated, and long declarations in a new sandbox config; compare human rows with JSON, cancel pickers, and retry failed registration. Expect no silent partial registration, extra terminal rows, or execution values reconstructed from display labels.
  - Agent check: PASS — registration, picker duplicate/collision, raw JSON, unknown-key preservation, and provider partial-success tests cover the underlying contracts; free exploration not run.

- [ ] Repeat this checklist on Linux with an existing toolchain and an ordinary user; verify child-status cancellation as well as normal native terminal behavior. Do not infer Linux success or custom signal-restoration guarantees from macOS results.
  - Agent check: NOT RUN — no Linux runner available; no targets, services, or toolchains installed.

## Defect log

| Severity | Steps | Expected / actual | Environment | Evidence |
| --- | --- | --- | --- | --- |
| Deferred scope | PID-directed signals and universal terminal restoration | Native ownership retained; no custom supervision guarantee | Unix | See the exit-code reference |
| Coverage gap | Linux checklist | Cross-platform proof / unavailable locally | Linux | Pending CI or human run |
| [fill in] | Exact commands and fixture | Expected versus actual | OS, terminal, binary version | Redacted transcript / config |

## Sign-off

- Decision: local automated gates passed; **pending** human acceptance and Linux validation.
- Known limitations: broader process/signal supervision is deferred. This
  checklist claims no completion until the human run and Linux validation pass.
- Tester/date: [fill in].
- Preserve this sandbox while reporting defects. It contains disposable data;
  delete it manually only after confirming the printed path and finishing review.
