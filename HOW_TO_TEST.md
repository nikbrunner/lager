# Repository-management acceptance checklist

## Test context

- Build: repository-hardening implementation. Record `lager --version` with
  the acceptance results.
- Surface: CLI and its terminal prompts/picker; no browser or TUI.
- Persona: ordinary local user, not root. Use only disposable repositories.
- Agent platform: macOS ARM64. Linux evidence is outstanding.
- Change: atomic registration, URL retention, guarded removal, child-result cancellation,
  terminal-safe human fields, and config/provider acceptance evidence.
- Local evidence: formatting, warning-denying Clippy, the locked all-target suite
  (159 tests), and the release build passed on macOS ARM64.
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

- [ ] Run `lager list --json > "$SANDBOX/list.json"` and `lager ls --json > "$SANDBOX/ls.json"`; compare with `cmp`. Expect identical JSON, two missing declarations, no decorative output. Run `lager list` again in the same shell; declarations persist.
  - Agent check: PASS — compiled-binary smoke compared exact `list`/`ls` JSON; config remained readable across invocations.

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

- [ ] Run `lager --config "$SANDBOX/display.toml" list`; expect one repository row, two real column separators, and one warning line. The field text shows `\n`, `\r`, `\t`, `\x1b`, `\x7f`, `\x85`; no cursor movement, terminal title change, or injected row. Run with `--json`; a JSON parser must recover the original control characters in the destination, not the human escape notation.
  - Agent check: PASS — `terminal_output_e2e` covers every C0/C1/DEL value, all three unknown-key warning levels, exact row/column counts, and raw JSON field values.

- [ ] In a real terminal, run `lager --config "$SANDBOX/prompt.toml" init`; press Enter for the default root and answer no to root creation and GitHub setup. Expect readable prompts, normal keyboard control, and the stored root `repos`. Cancel a new registration prompt with Ctrl-C; check the terminal still accepts normal input.
  - Agent check: PASS — safe input/default and confirmation PTY tests, registration prompt cancellation, and the full local suite pass; real-terminal acceptance remains for the tester.

- [ ] Edit the display fixture root to `"repos\\x1bend"` and list again; expect `repos\\x1bend` visibly distinct from a real ESC rendered as `\x1b`. Confirm long escaped labels remain understandable in your terminal.
  - Agent check: PASS — `literal_escape_notation_cannot_collide_with_an_exact_path_control` selects the intended unchanged candidate; terminal layout/accessibility still needs human judgment.

## Regression smoke test

- [ ] Register `https://github.com/org/transport` without cloning, then inspect `lager list --json`; expect HTTPS retained verbatim. Try both `https://token@github.com/org/rejected` and `https:///token@github.com/org/rejected`; expect redacted errors without persisting or echoing the token. Unregister `org/transport` afterward.
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
- Known limitations: broader process/signal supervision is deferred. No 1.0,
  inventory, standalone worktree, or TUI completion claim.
- Tester/date: [fill in].
- Preserve this sandbox while reporting defects. It contains disposable data;
  delete it manually only after confirming the printed path and finishing review.
