# Graph Report - inventory  (2026-09-21)

## Corpus Check
- 81 files · ~146,576 words
- Verdict: corpus is large enough that graph structure adds value.
- Unclassified: 5 file(s) not represented in the graph (top: (none) 3, .toml 2)

## Summary
- 1498 nodes · 4242 edges · 59 communities (51 shown, 8 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 32 edges (avg confidence: 0.86)
- Token cost: 153,129 input · 27,175 output

## Community Hubs (Navigation)
- Warehouse application operations
- CLI command routing
- Inventory terminal acceptance tests
- Configuration and registration tests
- Atomic configuration storage
- Local inventory scanning
- Configuration initialization
- Inventory plans and workflows
- Ensure checkout acceptance tests
- Native Git operations
- Inventory report assembly
- Repository declaration registry
- Inventory terminal rendering
- Repository hardening regression tests
- Provider integration acceptance tests
- Bitbucket Data Center discovery
- Bitbucket Cloud discovery
- GitHub repository discovery
- Repository reference identity
- Public adapter compatibility
- Fzf repository selection
- Interactive inventory session
- Portable configuration model
- Configured provider dispatch
- Terminal prompts and interaction
- Storage and removal ports
- Inventory loading and refresh
- Safe removal acceptance tests
- Inventory key bindings
- Application request contracts
- Build version provenance
- Architecture and contributor policies
- Filesystem discovery adapter
- Lager 2.0 product direction
- Inventory row selection
- Terminal output escaping tests
- Child process cancellation tests
- Removal command acceptance tests
- CLI usage and automation
- Checkout deletion safety rules
- Clone behavior acceptance tests
- Declarations and wildcard exclusions
- Lager 1.0 inventory specification
- Provider and configuration contracts
- JSON output and prototypes
- Lager warehouse branding
- Release build and distribution
- Terminal safe presentation
- Release history
- Child exit status handling
- Pre-push quality checks
- Release Please configuration
- Pre-commit formatting check
- Cargo package manifest

## God Nodes (most connected - your core abstractions)
1. `Config` - 43 edges
2. `RepositoryRef` - 36 edges
3. `Mutation` - 33 edges
4. `Interaction` - 30 edges
5. `Command` - 30 edges
6. `InteractionError` - 28 edges
7. `RepositorySummary` - 27 edges
8. `init_repo()` - 27 edges
9. `InventoryRow` - 25 edges
10. `screen_text()` - 25 edges

## Surprising Connections (you probably didn't know these)
- `Proposed native Git terminal handoff` --semantically_similar_to--> `Post-clone hook: registration and /bin/sh -c execution`  [INFERRED] [semantically similar]
  plans/inventory.html → docs/tutorials/first-repository.md
- `Proposed local/config refresh without provider requests` --semantically_similar_to--> `lager list compares declarations with disk offline`  [INFERRED] [semantically similar]
  plans/inventory.html → docs/tutorials/first-repository.md
- `legacy_calls()` --calls--> `register_many()`  [INFERRED]
  tests/public_api_compat.rs → src/application/registry.rs
- `Repository-hardening acceptance checklist (historical snapshot)` --conceptually_related_to--> `inventory / inv: read-only offline terminal overview`  [AMBIGUOUS]
  HOW_TO_TEST.md → docs/reference/cli.md
- `GitLab discovery fixture (prototype-only, not shipped provider)` --conceptually_related_to--> `Provider configuration guide`  [AMBIGUOUS]
  docs/prd/1.0.0-concepts.html → docs/how-to/providers.md

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **Portable desired-state reconciliation: declarations, identity, checkout, ensure and list** — docs_explanation_declarations_and_local_state_declaration, docs_explanation_declarations_and_local_state_repository_identity, docs_explanation_declarations_and_local_state_checkout, docs_reference_cli_ensure, docs_reference_cli_list [EXTRACTED 1.00]
- **Guarded exact-target deletion with bounded force and deferred config cleanup** — docs_reference_cli_remove, docs_how_to_remove_safely_removal_guards, docs_how_to_remove_safely_dependent_worktree_metadata, docs_how_to_remove_safely_bounded_force, docs_how_to_remove_safely_exact_path_revalidation, docs_how_to_remove_safely_disk_before_configuration [EXTRACTED 1.00]
- **Host-independent management services underpin host-dependent Forklift** — docs_prd_1_0_0_inventory, docs_prd_1_0_0_worktree_services, docs_prd_2_0_0_forklift, agents_shared_application_services, docs_prd_2_0_0_typed_host_adapter, docs_prd_2_0_0_herdr [EXTRACTED 1.00]
- **Desired-state reconciliation separates declaration, checkout, observation and ensure** — docs_tutorials_first_repository_declaration, docs_tutorials_first_repository_checkout, docs_tutorials_first_repository_list, docs_tutorials_first_repository_ensure [EXTRACTED 1.00]
- **Proposed coherent keymap refresh, mutation preservation and discoverable effective hints** — plans_inventory_mode_aware_keybindings, plans_inventory_keybinding_config_preservation, plans_inventory_local_refresh, plans_inventory_action_menu [EXTRACTED 1.00]
- **Proposed marked-target preview, guarded authorization and independent disk/config outcomes** — plans_inventory_row_selection_marks, plans_inventory_guarded_removal, plans_inventory_batch_outcomes [EXTRACTED 1.00]

## Communities (59 total, 8 thin omitted)

### Community 0 - "Warehouse application operations"
Cohesion: 0.06
Nodes (100): EffectiveCandidates, G, H, localstate, P, ParsedCloneInput, serialize, EnsureReporter (+92 more)

### Community 1 - "CLI command routing"
Cohesion: 0.05
Nodes (76): application, AsRef, clap, infrastructure, isterminal, OnceLock, parser, prompt_eligible (+68 more)

### Community 2 - "Inventory terminal acceptance tests"
Cohesion: 0.07
Nodes (78): atomic, AtomicU64, Child, Instant, Pid, stdio, Termios, FixtureProcesses (+70 more)

### Community 3 - "Configuration and registration tests"
Cohesion: 0.05
Nodes (46): fs, support, symlink, clone_config_failure_keeps_fresh_clone(), git(), mutation_preserves_mode_symlink_and_atomic_target(), remove_missing_member_is_an_idempotent_binary_noop(), Box (+38 more)

### Community 4 - "Atomic configuration storage"
Cohesion: 0.12
Nodes (48): Cell, Document, DocumentMut, env, fileext, FnOnce, Range, Mutation (+40 more)

### Community 5 - "Local inventory scanning"
Cohesion: 0.08
Nodes (47): AtomicBool, commandext, defaulthasher, hash, inventory, JoinHandle, normalize_remote, osstr (+39 more)

### Community 6 - "Configuration initialization"
Cohesion: 0.10
Nodes (27): resolve_portable_path, FakeInteraction, init(), init_diagnostics(), init_routes_root_creation_through_the_filesystem_port(), InitChoices, interactive_init_asks_only_for_omitted_choices(), MissingTools (+19 more)

### Community 7 - "Inventory plans and workflows"
Cohesion: 0.07
Nodes (41): Destination conflict protection, Force removal retains ownership and shape checks, fzf interactive repository selection, GitHub discovery: gh authentication and github.com host, list --json machine-output contract, Lager troubleshooting, Wildcard expansion: provider, API URL, credentials and archived preference, lager add creates checkout; --register preserves declaration (+33 more)

### Community 8 - "Ensure checkout acceptance tests"
Cohesion: 0.13
Nodes (31): process, executable(), NativeTools, Path, add_reports_destination_after_fresh_clone(), assert_colored_line(), clone_repeated_inputs_continue_after_failure(), clone_success_repeated_matching_noop_honors_add_and_conflicts_are_untouched() (+23 more)

### Community 9 - "Native Git operations"
Cohesion: 0.18
Nodes (32): Errno, ffi, file, OwnedFd, classify_destination(), cleanup_destination(), clear_directory(), clone_repository() (+24 more)

### Community 10 - "Inventory report assembly"
Cohesion: 0.13
Nodes (29): HashSet, build_inventory(), checkout(), CheckoutState, ConcreteDeclaration, config(), configured_destination(), declaration_key() (+21 more)

### Community 11 - "Repository declaration registry"
Cohesion: 0.17
Nodes (19): atomic_registration_validates_loaded_config_before_interaction(), FakeInteraction, legacy_registration_keeps_writes_before_ordinary_malformed_input(), RecordingStore, register_batch(), register_cancellation_stops_before_later_config_mutation(), register_many(), registration_choice() (+11 more)

### Community 12 - "Inventory terminal rendering"
Cohesion: 0.09
Nodes (29): CrosstermBackend, execute, layout, Line, load_inventory, localinventoryfilesystem, mpsc, Rc (+21 more)

### Community 13 - "Repository hardening regression tests"
Cohesion: 0.11
Nodes (30): tcplistener, bare_remote(), clone_rejects_existing_symlink_ancestor_without_writing_outside_root(), clone_safely_creates_a_missing_root_and_parent_chain(), clone_stays_in_held_root_when_its_path_is_replaced_after_git_starts(), every_invalid_semantic_config_fails_before_a_subprocess(), explicit_configured_reference_uses_one_exact_path_for_list_ensure_and_hook(), failed_clone_cleans_its_fresh_destination_via_held_parent() (+22 more)

### Community 14 - "Provider integration acceptance tests"
Cohesion: 0.22
Nodes (30): add_keeps_selected_repository_when_another_provider_fails(), add_remove_is_idempotent_and_reenables_wildcard_members(), bare_remote(), bitbucket_discovery_ignores_unused_clone_links_and_excluded_archives(), cloud_bearer_and_basic_environment_credentials_succeed_without_persistence(), cloud_listing_follows_opaque_next_filters_archived_and_ensures_idempotently(), cloud_pagination_accepts_relative_and_same_origin_opaque_links(), cloud_pagination_rejects_cross_origin_before_bearer_or_basic_credentials_leak() (+22 more)

### Community 15 - "Bitbucket Data Center discovery"
Cohesion: 0.13
Nodes (25): blocking, header, httpfixture, adapter_expands_personal_project_without_encoding_tilde(), adapter_paginates_readable_repositories_with_bearer_auth_and_filters_archives(), BitbucketDataCenter, CloneLink, Links (+17 more)

### Community 16 - "Bitbucket Cloud discovery"
Cohesion: 0.17
Nodes (23): adapter_paginates_workspaces_and_repositories_with_bearer_auth_and_filters_archives(), adapter_reports_unknown_auth_mode_before_request(), BitbucketCloud, CloneLink, Collection, Links, Repository, Client (+15 more)

### Community 17 - "GitHub repository discovery"
Cohesion: 0.16
Nodes (23): deserialize, Repository, adapter_preserves_gh_stderr_on_command_failure(), adapter_runs_gh_pagination_filters_archived_and_materializes_ssh(), Connection, Github, Organization, OrganizationData (+15 more)

### Community 18 - "Repository reference identity"
Cohesion: 0.18
Nodes (20): error, clean_path(), default_ssh_url(), malformed(), maps_unknown_hosts_without_collisions(), normalize_remote(), ordinary_username(), parses_common_reference_forms() (+12 more)

### Community 19 - "Public adapter compatibility"
Cohesion: 0.14
Nodes (16): command, HookRunner, Error, ExitStatus, Path, Result, run(), Shell (+8 more)

### Community 20 - "Fzf repository selection"
Cohesion: 0.15
Nodes (22): Default, OsString, repositoryref, candidate(), Fzf, FzfError, label(), maps_exit_130_to_cancellation() (+14 more)

### Community 21 - "Interactive inventory session"
Cohesion: 0.17
Nodes (16): Frame, Rect, abbreviate(), draw_inspection(), draw_root_menu(), draw_unavailable_inspection(), inspection_popup_area(), inspection_scroll_limit() (+8 more)

### Community 22 - "Portable configuration model"
Cohesion: 0.22
Nodes (22): collections, serde, Config, ConfigError, is_builtin_host(), is_canonical_id(), ProviderConfig, RepositoryDeclaration (+14 more)

### Community 23 - "Configured provider dispatch"
Cohesion: 0.20
Nodes (17): duration, hashmap, summary(), RepositorySummary, Adapter, apply_prefix(), call_catalog(), ConfiguredProviders (+9 more)

### Community 24 - "Terminal prompts and interaction"
Cohesion: 0.14
Nodes (15): escape, io, EnsureEvent, InteractionError, FakeInteraction, map_interaction_error(), require_terminal(), Error (+7 more)

### Community 25 - "Storage and removal ports"
Cohesion: 0.21
Nodes (9): AtomicRegistrationStore, config_path(), ConfigStore, GitClient, RemovalFilesystem, Error, Path, PathBuf (+1 more)

### Community 26 - "Inventory loading and refresh"
Cohesion: 0.13
Nodes (18): A, C, D, E, O, R, constructor_failure_restores_after_acquisition_without_starting_scan(), enter_after_restore() (+10 more)

### Community 27 - "Safe removal acceptance tests"
Cohesion: 0.26
Nodes (18): absent_or_empty_worktree_metadata_permits_removal(), blocked_primary_does_not_prevent_independent_eligible_removal(), dependency_arriving_after_initial_inspection_blocks_final_removal(), external_locked_and_stale_dependencies_are_not_pruned_or_bypassed(), Fixture, git(), live_dependent_blocks_forced_primary_removal_and_unregister(), malformed_worktree_directory_or_entry_blocks_removal() (+10 more)

### Community 28 - "Inventory key bindings"
Cohesion: 0.21
Nodes (14): event, Action, Bindings, Mode, normalized(), parse_key(), BTreeMap, InventoryKeyOverrides (+6 more)

### Community 29 - "Application request contracts"
Cohesion: 0.21
Nodes (10): ConfigurationFilesystem, GitRemoval, ProviderCatalog, ProviderFailure, RegistrationRequest, Option, String, Vec (+2 more)

### Community 30 - "Build version provenance"
Cohesion: 0.19
Nodes (16): git_output(), main(), Option, String, watch_git_path(), std, tempdir, checked_output() (+8 more)

### Community 31 - "Architecture and contributor policies"
Cohesion: 0.12
Nodes (17): CI workflow, macOS/Linux formatting, warning-denying Clippy and locked tests, Binary-boundary acceptance tests, Agent architecture guidelines, Shared application services, Domain documentation policy, Single-context vocabulary and architectural decisions, GitHub issue-tracking policy (+9 more)

### Community 32 - "Filesystem discovery adapter"
Cohesion: 0.31
Nodes (7): Filesystem, Error, Path, PathBuf, Result, Vec, walkdir

### Community 33 - "Lager 2.0 product direction"
Cohesion: 0.22
Nodes (15): Simulated standalone worktree cleanup, Planned worktree reciprocal ownership and retained-ref reachability, Standalone native Git worktree services (not started), Agreed future Herdr active-tab initialization, Lager 2.0 exploratory concepts (simulated only), Exploratory grouped checkout/worktree destinations and layout requests, Exploratory independent workspace and worktree cleanup, Exploratory workspace cursor vs explicit host focus (+7 more)

### Community 34 - "Inventory row selection"
Cohesion: 0.28
Nodes (8): InventoryRow, inspection_anchor_does_not_follow_reconciled_table_selection(), repository_path(), RepositoryPath, KeyEvent, Option, Selection, TableState

### Community 35 - "Terminal output escaping tests"
Cohesion: 0.25
Nodes (14): assert_safe_fields(), confirmation_display_is_safe_without_changing_default_answer(), controls(), human_rows_and_unknown_keys_escape_controls_while_json_preserves_values(), literal_escape_notation_cannot_collide_with_an_exact_path_control(), native_git_and_hook_streams_remain_raw_while_ensure_reporter_escapes_fields(), picker_escapes_labels_and_maps_duplicate_labels_to_untouched_candidates(), prompt_display_is_safe_and_empty_input_preserves_raw_default() (+6 more)

### Community 36 - "Child process cancellation tests"
Cohesion: 0.25
Nodes (10): permissionsext, clone_interruption_never_registers_or_hooks_and_preserves_earlier_success(), ensure_and_hook_stop_on_self_sigint_but_continue_ordinary_failure(), Fixture, hook_exit130_preserves_clone_and_registration_and_stops_add_batch(), Output, PathBuf, Self (+2 more)

### Community 37 - "Removal command acceptance tests"
Cohesion: 0.33
Nodes (13): batch_removal_continues_after_failure_and_aggregates_status(), force_never_bypasses_git_boundary_or_origin_guards(), force_remove_deletes_real_clone_before_unregistering_it(), lager(), lager_with_path(), missing_target_honors_unregister_or_keep_registered(), no_argument_remove_has_empty_success_and_picker_cancellation_exit(), remove_rejects_aliases_and_force_without_confirmation() (+5 more)

### Community 38 - "CLI usage and automation"
Cohesion: 0.24
Nodes (13): Native Git and shell process boundary, Automation guide, GitHub discovery via authenticated gh, Safe repository removal guide, CLI reference (documented current behavior), Explicit non-TTY choices, Exit-code reference, Exit statuses 0 / 1 / 2 / 130 (+5 more)

### Community 39 - "Checkout deletion safety rules"
Cohesion: 0.18
Nodes (13): Concrete local checkout, Declarations and local state, Normalized repository identity, Bounded force for disclosed local-state losses, Dependent worktree metadata deletion guard, Exact selected checkout revalidation, Non-forceable checkout ownership and path guards, add: create local checkouts (+5 more)

### Community 40 - "Clone behavior acceptance tests"
Cohesion: 0.32
Nodes (11): I, clone_explicit_local_remote_uses_exact_destination(), git_output(), Box, Error, Path, Result, String (+3 more)

### Community 41 - "Declarations and wildcard exclusions"
Cohesion: 0.24
Nodes (11): Portable desired repository declarations, Disk-success-before-configuration cleanup, All-matching wildcard exclusions and restoration, Wildcard declarations guide, Explicit declaration precedence and hooks, Provider-backed wildcard declaration, ensure: reconcile missing effective declarations, hook: rerun explicit post-clone hooks (+3 more)

### Community 42 - "Lager 1.0 inventory specification"
Cohesion: 0.33
Nodes (11): Lager 1.0 product specification (mixed implementation status), 1.0 inventory specification (partial; offline baseline implemented), Planned inventory mutation batches and stale confirmations, Mode-specific configurable inventory bindings (specification), Ratatui and Crossterm TUI architecture requirement, Planned inventory remote discovery (not started), Unverified responsiveness acceptance targets, inventory / inv: read-only offline terminal overview (+3 more)

### Community 43 - "Provider and configuration contracts"
Cohesion: 0.27
Nodes (10): Opt-in archived repository discovery, Bitbucket Cloud discovery, Bitbucket Data Center discovery, Provider configuration guide, Configuration selection and validation boundary, Configuration reference, Provider credentials via environment references, Legacy registry::register_many incremental behavior (+2 more)

### Community 44 - "JSON output and prototypes"
Cohesion: 0.20
Nodes (10): Partial provider success, Lager 1.0 illustrative concepts (not implementation approval), GitLab discovery fixture (prototype-only, not shipped provider), Simulated inventory table and remote provider requests, Illustrative list-as-inventory alias (conflicts with current CLI), Planned separate worktree list JSON schema, list / ls: offline declaration and local-state report, JSON provider_errors with successful rows (+2 more)

### Community 45 - "Lager warehouse branding"
Cohesion: 0.29
Nodes (8): add (advertised command), Lager banner: warmly lit warehouse with shelves of storage crates and product signage, ensure (advertised command), lager, list (advertised command), remove (advertised command), A home for your repositories, Organized warehouse storage: crates, tall shelving, and marked aisles

### Community 46 - "Release build and distribution"
Cohesion: 0.33
Nodes (6): Checksummed, provenance-attested release archives, Release workflow, Release Please automation, Four verified macOS/Linux release builds, Archive, Mise and Cargo installation, Lager declarative local Git repository manager

### Community 47 - "Terminal safe presentation"
Cohesion: 0.33
Nodes (5): Display, fmt, displayed_reference(), escape(), String

### Community 48 - "Release history"
Cohesion: 0.50
Nodes (4): Release changelog, Early fzf cancellation preservation, Lager 0.1.0 release (2026-09-13), Lager 0.1.1 release (2026-09-13)

### Community 49 - "Child exit status handling"
Cohesion: 0.67
Nodes (3): exitstatusext, is_cancelled(), ExitStatus

### Community 50 - "Pre-push quality checks"
Cohesion: 0.50
Nodes (4): clippy: locked, all targets/features, warnings denied, Unset Git local environment variables before tests, pre-push hook, test: cargo test --locked --all-targets

## Ambiguous Edges - Review These
- `Repository-hardening acceptance checklist (historical snapshot)` → `inventory / inv: read-only offline terminal overview`  [AMBIGUOUS]
  HOW_TO_TEST.md · relation: conceptually_related_to
- `Provider configuration guide` → `GitLab discovery fixture (prototype-only, not shipped provider)`  [AMBIGUOUS]
  docs/prd/1.0.0-concepts.html · relation: conceptually_related_to
- `Lager 1.0 product specification (mixed implementation status)` → `inventory / inv: read-only offline terminal overview`  [AMBIGUOUS]
  docs/prd/1.0.0.md · relation: conceptually_related_to
- `list / ls: offline declaration and local-state report` → `Illustrative list-as-inventory alias (conflicts with current CLI)`  [AMBIGUOUS]
  docs/prd/1.0.0-concepts.html · relation: conceptually_related_to
- `Typed host adapter boundary (planned)` → `Superlogical / rex (possible future host; unestablished API)`  [AMBIGUOUS]
  docs/prd/2.0.0.md · relation: conceptually_related_to

## Knowledge Gaps
- **35 isolated node(s):** `$schema`, `packages`, `lager`, `MAX_CONCURRENT_PROBES`, `PAGE_QUERY` (+30 more)
  These have ≤1 connection - possible missing edges or undocumented components. (Counts symbols only; 242 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **8 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **What is the exact relationship between `Repository-hardening acceptance checklist (historical snapshot)` and `inventory / inv: read-only offline terminal overview`?**
  _Edge tagged AMBIGUOUS (relation: conceptually_related_to) - confidence is low._
- **What is the exact relationship between `Provider configuration guide` and `GitLab discovery fixture (prototype-only, not shipped provider)`?**
  _Edge tagged AMBIGUOUS (relation: conceptually_related_to) - confidence is low._
- **What is the exact relationship between `Lager 1.0 product specification (mixed implementation status)` and `inventory / inv: read-only offline terminal overview`?**
  _Edge tagged AMBIGUOUS (relation: conceptually_related_to) - confidence is low._
- **What is the exact relationship between `list / ls: offline declaration and local-state report` and `Illustrative list-as-inventory alias (conflicts with current CLI)`?**
  _Edge tagged AMBIGUOUS (relation: conceptually_related_to) - confidence is low._
- **What is the exact relationship between `Typed host adapter boundary (planned)` and `Superlogical / rex (possible future host; unestablished API)`?**
  _Edge tagged AMBIGUOUS (relation: conceptually_related_to) - confidence is low._
- **Why does `Config` connect `Portable configuration model` to `Warehouse application operations`, `Atomic configuration storage`, `Configuration initialization`, `Inventory report assembly`, `Repository declaration registry`, `Inventory terminal rendering`, `Public adapter compatibility`, `Interactive inventory session`, `Configured provider dispatch`, `Storage and removal ports`, `Application request contracts`?**
  _High betweenness centrality (0.138) - this node is a cross-community bridge._
- **Why does `RepositoryRef` connect `Repository reference identity` to `Warehouse application operations`, `CLI command routing`, `Atomic configuration storage`, `Inventory report assembly`, `Bitbucket Data Center discovery`, `Bitbucket Cloud discovery`, `GitHub repository discovery`, `Configured provider dispatch`, `Application request contracts`?**
  _High betweenness centrality (0.088) - this node is a cross-community bridge._
## Graph health

The extraction contains 189 dangling-endpoint edges and 288 edges collapsed onto shared endpoint pairs in the undirected graph. No missing-endpoint fields or self-loops were detected. The built graph includes endpoint nodes without extracted definitions; traversal is incomplete. Full diagnostics are in `graph-health.json`.

Token counts cover semantic extraction agents; parent orchestration and advisor usage are not included.
