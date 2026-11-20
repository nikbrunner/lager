mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

/// A temporary HOME with one originless repository at `repos/acme/core` on branch `main`.
struct Fixture {
    _temp: tempfile::TempDir,
    /// Real path of the temporary directory; Git reports real paths.
    base: PathBuf,
    home: PathBuf,
    config: PathBuf,
    primary: PathBuf,
    /// The fake `rex`'s directory and state, first on `PATH` when present.
    rex: Option<(PathBuf, PathBuf)>,
}

/// Runs Git and returns whatever it did; a stopped rebase or merge exits non-zero.
fn git_output(directory: &Path, args: &[&str]) -> Output {
    support::external(directory, "git")
        .arg("-C")
        .arg(directory)
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args([
            "-c",
            "user.name=lager",
            "-c",
            "user.email=lager@example.invalid",
        ])
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .output()
        .unwrap()
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = git_output(directory, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(directory: &Path, file: &str, message: &str) -> String {
    fs::write(directory.join(file), format!("{message}\n")).unwrap();
    git(directory, &["add", file]);
    git(directory, &["commit", "-qm", message]);
    git(directory, &["rev-parse", "HEAD"])
}

fn init(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    commit(path, "README", "initial");
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// Every entry below `directory`, without the contents of Git directories.
fn tree(directory: &Path) -> Vec<String> {
    let mut entries = walkdir::WalkDir::new(directory)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            entry.path().parent().and_then(Path::file_name) != Some(std::ffi::OsStr::new(".git"))
        })
        .map(|entry| {
            let entry = entry.unwrap();
            entry
                .path()
                .strip_prefix(directory)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn short(id: &str) -> &str {
    &id[..7]
}

/// The rows `worktree list` prints: path, branch, state and with sessions the session, two
/// spaces apart.
fn table<const N: usize>(rows: &[[&str; N]]) -> String {
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap()
    };
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(column, cell)| match column + 1 < N {
                    true => format!("{cell:width$}  ", width = width(column)),
                    false => (*cell).to_owned(),
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
                + "\n"
        })
        .collect()
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let home = base.join("home");
        let config = base.join("config.toml");
        let primary = home.join("repos/acme/core");
        init(&primary);
        fs::write(&config, "root = \"repos\"\n").unwrap();
        Self {
            _temp: temp,
            base,
            home,
            config,
            primary,
            rex: None,
        }
    }

    /// The fake `rex` first on `PATH`, with a `[session]` section when `configured`. No call
    /// can reach a real Rex: the environment is cleared, and `REX_SERVER` names a socket that
    /// does not exist.
    fn with_rex(configured: bool) -> Self {
        let mut fixture = Self::new();
        if configured {
            fs::write(&fixture.config, format!("root = \"repos\"\n{SESSION}")).unwrap();
        }
        fixture.rex = Some(support::install_fake_rex(&fixture.base.join("rex")));
        fixture
    }

    fn command(&self, cwd: &Path) -> Command {
        let Some((bin, state)) = &self.rex else {
            let mut command = support::lager(&self.home, &self.config);
            command.current_dir(cwd);
            return command;
        };
        let mut command = support::lager_with_path(&self.home, &self.config, Some(bin));
        command
            .current_dir(cwd)
            .env("REX_ARGS_LOG", state.join("args.log"))
            .env("REX_STATE", state)
            .env(
                "REX_SERVER",
                format!("unix://{}", self.base.join("none.sock").display()),
            )
            .env("REX_RUNTIME_DIR", self.base.join("rex-runtime"));
        command
    }

    fn rex_state(&self) -> &Path {
        &self.rex.as_ref().expect("a fixture with the fake rex").1
    }

    /// Every rex call so far, global flags removed.
    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.rex_state().join("args.log")).unwrap_or_default();
        log.lines()
            .map(|line| {
                line.strip_prefix("--autostart=false --timeout 8s ")
                    .unwrap_or_else(|| panic!("rex call without the global flags: {line}"))
                    .to_owned()
            })
            .collect()
    }

    fn creates(&self) -> Vec<Value> {
        self.calls()
            .iter()
            .filter_map(|call| call.strip_prefix("do - --json --args "))
            .map(|args| serde_json::from_str(args).unwrap())
            .collect()
    }

    fn destroys(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.contains("session.destroy"))
            .collect()
    }

    fn mode(&self, name: &str) {
        fs::write(self.rex_state().join(name), "").unwrap();
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().unwrap()
    }

    /// Runs `lager worktree ...` in the primary checkout.
    fn worktree(&self, args: &[&str]) -> Output {
        self.run(&self.primary, &[&["worktree"], args].concat())
    }

    fn container(&self) -> PathBuf {
        self.home.join("repos/acme/core.worktrees")
    }

    /// Creates a linked worktree with native Git at the default placement.
    fn linked(&self, name: &str, args: &[&str]) -> PathBuf {
        let path = self.container().join(name);
        git(
            &self.primary,
            &[&["worktree", "add", "-q", text(&path)], args].concat(),
        );
        path
    }

    fn head(&self) -> String {
        git(&self.primary, &["rev-parse", "HEAD"])
    }

    /// Paths Git lists as worktrees, the primary first.
    fn listed(&self) -> Vec<PathBuf> {
        git(&self.primary, &["worktree", "list", "--porcelain"])
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect()
    }

    /// Every local branch with its commit.
    fn branches(&self) -> Vec<String> {
        git(
            &self.primary,
            &[
                "for-each-ref",
                "--format=%(refname:short) %(objectname)",
                "refs/heads",
            ],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    /// Asserts that a refused command left the repository as `before` recorded it.
    fn assert_unchanged(&self, before: &(Vec<PathBuf>, Vec<String>)) {
        assert_eq!((self.listed(), self.branches()), *before);
    }

    /// Asserts that Git lists no linked worktree.
    fn assert_only_primary(&self) {
        assert_eq!(self.listed(), std::slice::from_ref(&self.primary));
    }

    fn state(&self) -> (Vec<PathBuf>, Vec<String>) {
        (self.listed(), self.branches())
    }
}

fn assert_exit(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
}

// ---- add: branch modes -------------------------------------------------------------------

#[test]
fn add_creates_a_new_branch_named_after_the_worktree_from_the_primary_head() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let destination = fixture.container().join("hotfix");

    let output = fixture.worktree(&["add", ".", "hotfix"]);

    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        format!(
            "HEAD is now at {} initial\nCreated worktree {} on new branch hotfix\n",
            short(&head),
            destination.display()
        )
    );
    assert_eq!(
        stderr(&output),
        "Preparing worktree (new branch 'hotfix')\n"
    );
    assert_eq!(
        fixture.listed(),
        [fixture.primary.clone(), destination.clone()]
    );
    assert_eq!(
        fixture.branches(),
        [format!("hotfix {head}"), format!("main {head}")]
    );
    assert_eq!(
        git(&destination, &["symbolic-ref", "HEAD"]),
        "refs/heads/hotfix"
    );
    assert_eq!(tree(&destination), [".git", "README"]);
    assert_eq!(
        tree(&fixture.container()),
        ["hotfix", "hotfix/.git", "hotfix/README"]
    );
}

#[test]
fn add_through_a_linked_worktree_branches_from_the_primary_head_beside_the_primary() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let linked = fixture.linked("other", &["-b", "other"]);
    let ahead = commit(&linked, "ahead", "ahead");
    assert_ne!(ahead, head);

    let output = fixture.run(&linked, &["worktree", "add", ".", "hotfix"]);

    assert_exit(&output, 0);
    let destination = fixture.container().join("hotfix");
    assert_eq!(
        stdout(&output),
        format!(
            "HEAD is now at {} initial\nCreated worktree {} on new branch hotfix\n",
            short(&head),
            destination.display()
        )
    );
    assert_eq!(git(&destination, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fixture.listed(),
        [fixture.primary.clone(), destination, linked]
    );
}

#[test]
fn add_new_branch_names_the_branch_independently_of_the_directory() {
    let fixture = Fixture::new();
    let head = fixture.head();

    let output = fixture.worktree(&["add", ".", "spike", "--new-branch", "dev/spike"]);

    assert_exit(&output, 0);
    let destination = fixture.container().join("spike");
    assert!(stdout(&output).ends_with(&format!(
        "Created worktree {} on new branch dev/spike\n",
        destination.display()
    )));
    assert_eq!(
        fixture.branches(),
        [format!("dev/spike {head}"), format!("main {head}")]
    );
    assert_eq!(
        git(&destination, &["symbolic-ref", "HEAD"]),
        "refs/heads/dev/spike"
    );
}

#[test]
fn add_branch_checks_out_an_existing_local_branch_without_moving_it() {
    let fixture = Fixture::new();
    let first = fixture.head();
    git(&fixture.primary, &["branch", "feature/login"]);
    let second = commit(&fixture.primary, "second", "second");

    let output = fixture.worktree(&["add", ".", "review-41", "--branch", "feature/login"]);

    assert_exit(&output, 0);
    let destination = fixture.container().join("review-41");
    assert_eq!(
        stdout(&output),
        format!(
            "HEAD is now at {} initial\nCreated worktree {} on branch feature/login\n",
            short(&first),
            destination.display()
        )
    );
    assert_eq!(
        stderr(&output),
        "Preparing worktree (checking out 'feature/login')\n"
    );
    assert_eq!(
        fixture.branches(),
        [format!("feature/login {first}"), format!("main {second}")]
    );
    assert_eq!(
        git(&destination, &["symbolic-ref", "HEAD"]),
        "refs/heads/feature/login"
    );
}

#[test]
fn add_start_branches_from_a_local_commit_and_never_fetches_one() {
    let fixture = Fixture::new();
    let first = fixture.head();
    git(&fixture.primary, &["tag", "v1"]);
    let second = commit(&fixture.primary, "second", "second");
    git(
        &fixture.primary,
        &[
            "remote",
            "add",
            "origin",
            "http://127.0.0.1:9/acme/core.git",
        ],
    );
    support::clear_outbound_requests();

    let output = fixture.worktree(&["add", ".", "spike", "--start", "v1"]);
    assert_exit(&output, 0);
    let destination = fixture.container().join("spike");
    assert_eq!(git(&destination, &["rev-parse", "HEAD"]), first);
    assert_eq!(
        fixture.branches(),
        [format!("main {second}"), format!("spike {first}")]
    );

    let before = fixture.state();
    for (revision, name) in [
        ("origin/main", "remote"),
        ("HEAD^{tree}", "tree"),
        ("nope", "missing"),
    ] {
        let output = fixture.worktree(&["add", ".", name, "--start", revision]);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!("error: --start does not name a local commit: {revision}\n")
        );
        assert!(!fixture.container().join(name).exists());
    }
    fixture.assert_unchanged(&before);
    assert!(!fixture.primary.join(".git/FETCH_HEAD").exists());
    assert_eq!(git(&fixture.primary, &["for-each-ref", "refs/remotes"]), "");
    assert_eq!(support::outbound_requests(), Vec::<String>::new());
}

#[test]
fn add_rejects_invalid_requests_before_anything_is_created() {
    let fixture = Fixture::new();
    git(&fixture.primary, &["branch", "taken"]);
    let before = fixture.state();
    let name = "error: NAME is one path component of letters, digits, `.`, `_` or `-`, starting with a letter or digit\n";
    let cases: &[(&[&str], i32, &str)] = &[
        (
            &["x", "--branch", "taken", "--start", "main"],
            2,
            "error: --start applies only to a new branch\n",
        ),
        (&["a/b"], 2, name),
        (&[".."], 2, name),
        (&[".hidden"], 2, name),
        (&["a b"], 2, name),
        (&["caf\u{e9}"], 2, name),
        (&[""], 2, name),
        (&["a..b"], 2, "error: invalid branch name: a..b\n"),
        (
            &["x", "--new-branch", "a..b"],
            2,
            "error: invalid branch name: a..b\n",
        ),
        (
            &["x", "--new-branch", "@{-1}"],
            2,
            "error: invalid branch name: @{-1}\n",
        ),
        (
            &["x", "--new-branch", "HEAD"],
            2,
            "error: invalid branch name: HEAD\n",
        ),
        (
            &["taken"],
            1,
            "error: branch already exists: taken; --branch checks it out\n",
        ),
        (
            &["x", "--new-branch", "main"],
            1,
            "error: branch already exists: main; --branch checks it out\n",
        ),
        (
            &["x", "--branch", "nope"],
            1,
            "error: no local branch named nope\n",
        ),
    ];
    for (args, code, message) in cases {
        let output = fixture.worktree(&[&["add", "."], *args].concat());
        assert_exit(&output, *code);
        assert_eq!(stdout(&output), "", "{args:?}");
        assert_eq!(stderr(&output), *message, "{args:?}");
        assert!(!fixture.container().exists(), "{args:?}");
        fixture.assert_unchanged(&before);
    }

    let both = fixture.worktree(&["add", ".", "x", "--branch", "taken", "--new-branch", "y"]);
    assert_exit(&both, 2);
    assert!(stderr(&both).contains("cannot be used with"), "{both:?}");
    // A usage error does not depend on the repository being found.
    let usage = fixture.worktree(&["add", "acme/absent", "x", "--branch", "y", "--start", "z"]);
    assert_exit(&usage, 2);
    let unresolved = fixture.worktree(&["add", "acme/absent", "x"]);
    assert_exit(&unresolved, 1);
    assert_eq!(
        stderr(&unresolved),
        format!(
            "error: not a Git checkout: {}\n",
            fixture.home.join("repos/acme/absent").display()
        )
    );
    assert!(!fixture.container().exists());
    fixture.assert_unchanged(&before);
}

#[test]
fn add_leaves_a_branch_checked_out_elsewhere_to_git_and_reports_what_remains() {
    let fixture = Fixture::new();
    let before = fixture.state();

    let output = fixture.worktree(&["add", ".", "second", "--branch", "main"]);

    assert_exit(&output, 1);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!(
            "Preparing worktree (checking out 'main')\nfatal: 'main' is already used by worktree at '{}'\nerror: worktree was not created: git exited with status 128\nremaining: directory {}\n",
            fixture.primary.display(),
            fixture.container().display()
        )
    );
    fixture.assert_unchanged(&before);
    assert_eq!(tree(&fixture.container()), Vec::<String>::new());
}

#[test]
fn add_reports_the_branch_a_failed_checkout_leaves_behind() {
    let fixture = Fixture::new();
    git(&fixture.primary, &["config", "filter.boom.clean", "cat"]);
    git(&fixture.primary, &["config", "filter.boom.smudge", "cat"]);
    git(
        &fixture.primary,
        &["config", "filter.boom.required", "true"],
    );
    fs::write(
        fixture.primary.join(".gitattributes"),
        "README filter=boom\n",
    )
    .unwrap();
    git(&fixture.primary, &["add", ".gitattributes"]);
    git(&fixture.primary, &["commit", "-qm", "filter"]);
    git(
        &fixture.primary,
        &["config", "filter.boom.smudge", "exit 1"],
    );
    let head = fixture.head();

    let output = fixture.worktree(&["add", ".", "broken"]);

    assert_exit(&output, 1);
    let destination = fixture.container().join("broken");
    let diagnostics = stderr(&output);
    assert!(
        diagnostics.ends_with(&format!(
            "error: worktree was not created: git exited with status 128\nremaining: directory {}\nremaining: branch broken\n",
            fixture.container().display()
        )),
        "{diagnostics}"
    );
    assert!(!stdout(&output).contains("Created"));
    // The report names exactly what is there: the created parent and the new branch.
    assert!(!destination.exists());
    assert_eq!(tree(&fixture.container()), Vec::<String>::new());
    fixture.assert_only_primary();
    assert_eq!(
        fixture.branches(),
        [format!("broken {head}"), format!("main {head}")]
    );
}

// ---- add: placement ----------------------------------------------------------------------

#[test]
fn add_places_worktrees_under_worktree_root_by_the_primary_path_below_root() {
    for (value, root) in [
        ("~/trees", "trees"),
        ("trees/deep", "trees/deep"),
        ("~", ""),
    ] {
        let fixture = Fixture::new();
        fs::write(
            &fixture.config,
            format!("root = \"repos\"\nworktree_root = \"{value}\"\n"),
        )
        .unwrap();
        let destination = fixture.home.join(root).join("acme/core.worktrees/hotfix");

        let output = fixture.worktree(&["add", ".", "hotfix"]);

        assert_exit(&output, 0);
        assert!(
            stdout(&output).ends_with(&format!(
                "Created worktree {} on new branch hotfix\n",
                destination.display()
            )),
            "{output:?}"
        );
        assert_eq!(
            fixture.listed(),
            [fixture.primary.clone(), destination.clone()]
        );
        assert_eq!(tree(&destination), [".git", "README"]);
        assert!(!fixture.container().exists());
    }
}

#[test]
fn add_with_worktree_root_needs_path_for_a_primary_outside_root() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.config,
        "root = \"repos\"\nworktree_root = \"~/trees\"\n",
    )
    .unwrap();
    let outside = fixture.base.join("outside/repo");
    init(&outside);

    let refused = fixture.run(&outside, &["worktree", "add", ".", "hotfix"]);
    assert_exit(&refused, 2);
    assert_eq!(stdout(&refused), "");
    assert_eq!(
        stderr(&refused),
        format!(
            "error: {} is outside the configured root; worktree_root needs --path here\n",
            outside.display()
        )
    );
    assert_eq!(
        tree(&fixture.base.join("outside")),
        ["repo", "repo/.git", "repo/README"]
    );
    assert!(!fixture.home.join("trees").exists());
    assert_eq!(
        git(&outside, &["branch", "--format=%(refname:short)"]),
        "main"
    );

    let destination = fixture.base.join("elsewhere/hotfix");
    let placed = fixture.run(
        &outside,
        &[
            "worktree",
            "add",
            ".",
            "hotfix",
            "--path",
            text(&destination),
        ],
    );
    assert_exit(&placed, 0);
    assert_eq!(tree(&destination), [".git", "README"]);
    assert!(!fixture.home.join("trees").exists());
}

#[test]
fn add_path_accepts_absolute_relative_and_parent_paths_and_persists_nothing() {
    let fixture = Fixture::new();
    let config = fs::read(&fixture.config).unwrap();
    let cwd = fixture.base.join("scratch/here");
    fs::create_dir_all(&cwd).unwrap();
    let cases = [
        (
            "one",
            text(&fixture.base.join("absolute/one")).to_owned(),
            fixture.base.join("absolute/one"),
        ),
        ("two", "sub/two".to_owned(), cwd.join("sub/two")),
        (
            "three",
            "../there/./x/../three".to_owned(),
            fixture.base.join("scratch/there/three"),
        ),
    ];
    for (name, path, destination) in &cases {
        let output = fixture.run(
            &cwd,
            &[
                "worktree",
                "add",
                text(&fixture.primary),
                name,
                "--path",
                path,
            ],
        );
        assert_exit(&output, 0);
        assert!(
            stdout(&output).ends_with(&format!(
                "Created worktree {} on new branch {name}\n",
                destination.display()
            )),
            "{output:?}"
        );
        assert_eq!(tree(destination), [".git", "README"]);
    }
    let mut listed = vec![fixture.primary.clone()];
    listed.extend(cases.iter().map(|case| case.2.clone()));
    assert_eq!(fixture.listed(), listed);
    assert_eq!(fs::read(&fixture.config).unwrap(), config);
    assert!(!fixture.container().exists());
}

#[test]
fn add_refuses_every_existing_destination_even_an_empty_directory() {
    let fixture = Fixture::new();
    let stale = fixture.linked("stale", &["-b", "stale"]);
    fs::remove_dir_all(&stale).unwrap();
    for name in ["empty", "full"] {
        fs::create_dir_all(fixture.container().join(name)).unwrap();
    }
    fs::write(fixture.container().join("full/keep"), "keep\n").unwrap();
    fs::write(fixture.container().join("file"), "file\n").unwrap();
    std::os::unix::fs::symlink("nowhere", fixture.container().join("dangling")).unwrap();
    let before = (fixture.state(), tree(&fixture.container()));

    for name in ["empty", "full", "file", "dangling"] {
        let output = fixture.worktree(&["add", ".", name]);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!(
                "error: destination exists: {}\n",
                fixture.container().join(name).display()
            )
        );
    }
    let registered = fixture.worktree(&["add", ".", "stale", "--new-branch", "again"]);
    assert_exit(&registered, 1);
    assert_eq!(
        stderr(&registered),
        format!(
            "error: destination is still registered as a worktree: {}; clear it with: git -C {} worktree prune\n",
            stale.display(),
            fixture.primary.display()
        )
    );
    assert_eq!((fixture.state(), tree(&fixture.container())), before);
    assert_eq!(
        fs::read_to_string(fixture.container().join("full/keep")).unwrap(),
        "keep\n"
    );
}

#[test]
fn add_refuses_a_destination_below_a_symlink() {
    let fixture = Fixture::new();
    let target = fixture.base.join("target");
    fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, fixture.container()).unwrap();
    let link = fixture.base.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let before = fixture.state();

    let derived = fixture.worktree(&["add", ".", "hotfix"]);
    assert_exit(&derived, 1);
    assert_eq!(
        stderr(&derived),
        format!(
            "error: destination has a symlink ancestor: {}\n",
            fixture.container().display()
        )
    );
    let explicit = fixture.worktree(&[
        "add",
        ".",
        "hotfix",
        "--path",
        text(&link.join("deep/hotfix")),
    ]);
    assert_exit(&explicit, 1);
    assert_eq!(
        stderr(&explicit),
        format!(
            "error: destination has a symlink ancestor: {}\n",
            link.display()
        )
    );
    assert_eq!(tree(&target), Vec::<String>::new());
    fixture.assert_unchanged(&before);
}

#[test]
fn add_refuses_a_destination_inside_a_checkout_or_git_metadata() {
    let fixture = Fixture::new();
    let other = fixture.base.join("other");
    init(&other);
    let bare = fixture.base.join("bare.git");
    git(&fixture.base, &["init", "-q", "--bare", text(&bare)]);
    let linked = fixture.linked("linked", &["-b", "linked"]);
    let before = (fixture.state(), tree(&fixture.base));
    let cases = [
        (
            fixture.primary.join("inside"),
            format!(
                "destination is inside the checkout {}",
                fixture.primary.display()
            ),
        ),
        (
            fixture.primary.join("a/b/inside"),
            format!(
                "destination is inside the checkout {}",
                fixture.primary.display()
            ),
        ),
        (
            linked.join("inside"),
            format!("destination is inside the checkout {}", linked.display()),
        ),
        (
            other.join("inside"),
            format!("destination is inside the checkout {}", other.display()),
        ),
        (
            fixture.primary.join(".git/inside"),
            format!(
                "destination is inside Git metadata: {}",
                fixture.primary.join(".git/inside").display()
            ),
        ),
        (
            bare.join("inside"),
            format!("destination is inside Git metadata: {}", bare.display()),
        ),
    ];
    for (destination, message) in cases {
        let output = fixture.worktree(&["add", ".", "x", "--path", text(&destination)]);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(stderr(&output), format!("error: {message}\n"));
        assert_eq!((fixture.state(), tree(&fixture.base)), before);
    }
}

#[test]
fn add_creates_missing_parents_only_for_a_valid_request() {
    let fixture = Fixture::new();
    let destination = fixture.base.join("deep/a/b/wt");

    let refused = fixture.worktree(&[
        "add",
        ".",
        "x",
        "--branch",
        "nope",
        "--path",
        text(&destination),
    ]);
    assert_exit(&refused, 1);
    assert!(!fixture.base.join("deep").exists());

    let created = fixture.worktree(&["add", ".", "x", "--path", text(&destination)]);
    assert_exit(&created, 0);
    assert_eq!(
        tree(&fixture.base.join("deep")),
        ["a", "a/b", "a/b/wt", "a/b/wt/.git", "a/b/wt/README"]
    );
}

// ---- list --------------------------------------------------------------------------------

/// Primary on `main`, `hotfix`, a locked detached `pinned` and a prunable external worktree.
fn populated() -> (Fixture, String) {
    let fixture = Fixture::new();
    // A neighbour with its own worktree: the listing is one repository, not a scan.
    let neighbour = fixture.home.join("repos/acme/other");
    init(&neighbour);
    git(
        &neighbour,
        &[
            "worktree",
            "add",
            "-q",
            text(&fixture.home.join("repos/acme/other.worktrees/x")),
        ],
    );
    let head = fixture.head();
    fixture.linked("hotfix", &["-b", "hotfix"]);
    let pinned = fixture.linked("pinned", &["--detach"]);
    git(
        &fixture.primary,
        &[
            "worktree",
            "lock",
            "--reason",
            "reviewing on laptop",
            text(&pinned),
        ],
    );
    let spike = fixture.base.join("scratch/core-spike");
    git(
        &fixture.primary,
        &["worktree", "add", "-q", "-b", "dev/spike", text(&spike)],
    );
    fs::remove_dir_all(&spike).unwrap();
    let expected = table(&[
        [text(&fixture.primary), "main", "primary"],
        [text(&fixture.container().join("hotfix")), "hotfix", ""],
        [
            text(&pinned),
            &format!("detached {}", short(&head)),
            "locked: reviewing on laptop",
        ],
        [
            text(&spike),
            "dev/spike",
            "prunable: gitdir file points to non-existent location",
        ],
    ]);
    (fixture, expected)
}

#[test]
fn list_prints_the_cwd_repository_with_primary_external_locked_and_prunable_rows() {
    let (fixture, expected) = populated();
    let nested = fixture.primary.join("src/deep");
    fs::create_dir_all(&nested).unwrap();

    let output = fixture.run(&nested, &["worktree", "list"]);

    assert_exit(&output, 0);
    assert_eq!(stdout(&output), expected);
    assert_eq!(stderr(&output), "");
}

#[test]
fn list_resolves_paths_references_and_linked_worktrees_to_the_same_repository() {
    let (fixture, expected) = populated();
    let hotfix = fixture.container().join("hotfix");
    let runs = [
        fixture.run(&hotfix, &["worktree", "list"]),
        fixture.run(&fixture.base, &["worktree", "list", text(&fixture.primary)]),
        fixture.run(&fixture.base, &["worktree", "list", text(&hotfix)]),
        fixture.run(&fixture.home, &["worktree", "list", "./repos/acme/core"]),
        fixture.run(&hotfix, &["worktree", "list", "../../core"]),
        fixture.run(&fixture.base, &["worktree", "list", "acme/core"]),
        fixture.run(&fixture.base, &["worktree", "list", "github.com/acme/core"]),
    ];
    for output in runs {
        assert_exit(&output, 0);
        assert_eq!(stdout(&output), expected);
        assert_eq!(stderr(&output), "");
    }
}

#[test]
fn list_outside_a_repository_needs_a_target_and_unresolved_targets_print_no_json() {
    let fixture = Fixture::new();
    let src = fixture.primary.join("src");
    fs::create_dir_all(&src).unwrap();
    let bare = fixture.base.join("bare.git");
    git(&fixture.base, &["init", "-q", "--bare", text(&bare)]);
    let cases: &[(&[&str], i32, String)] = &[
        (
            &[],
            2,
            format!(
                "error: not a Git checkout: {}; name a repository\n",
                fixture.base.display()
            ),
        ),
        (
            &["acme/absent"],
            1,
            format!(
                "error: not a Git checkout: {}\n",
                fixture.home.join("repos/acme/absent").display()
            ),
        ),
        (
            &[text(&src)],
            1,
            format!("error: not the root of a checkout: {}\n", src.display()),
        ),
        (
            &[text(&bare)],
            1,
            format!("error: not a Git checkout: {}\n", bare.display()),
        ),
        (
            &["acme/*"],
            1,
            "error: a wildcard does not name one repository\n".to_owned(),
        ),
    ];
    for (args, code, message) in cases {
        for json in [&[][..], &["--json"]] {
            let output = fixture.run(
                &fixture.base,
                &[&["worktree", "list"], *args, json].concat(),
            );
            assert_exit(&output, *code);
            assert_eq!(stdout(&output), "", "{args:?}");
            assert_eq!(stderr(&output), *message, "{args:?}");
        }
    }
}

#[test]
fn list_json_carries_every_field_of_every_record_sorted_by_path() {
    let fixture = Fixture::new();
    let head = fixture.head();
    git(
        &fixture.primary,
        &["remote", "add", "origin", "git@github.com:acme/core.git"],
    );
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let pinned = fixture.linked("pinned", &["--detach"]);
    git(
        &fixture.primary,
        &[
            "worktree",
            "lock",
            "--reason",
            "reviewing\non laptop",
            text(&pinned),
        ],
    );
    let plain = fixture.linked("plain-lock", &["-b", "plain"]);
    git(&fixture.primary, &["worktree", "lock", text(&plain)]);
    let unborn = fixture.linked("unborn", &["--orphan", "-b", "unborn"]);
    let spike = fixture.base.join("scratch/core-spike");
    git(
        &fixture.primary,
        &["worktree", "add", "-q", "-b", "dev/spike", text(&spike)],
    );
    fs::remove_dir_all(&spike).unwrap();
    let record = |path: &Path, primary: bool, branch: Value, head: Value| {
        json!({
            "repository_identity": "github.com/acme/core",
            "primary_path": text(&fixture.primary),
            "path": text(path),
            "is_primary": primary,
            "branch": branch,
            "head": head,
            "locked": false,
            "lock_reason": null,
            "prunable": false,
            "prune_reason": null,
        })
    };
    let mut locked = record(&pinned, false, Value::Null, json!(head));
    locked["locked"] = json!(true);
    locked["lock_reason"] = json!("reviewing\non laptop");
    let mut unreasoned = record(&plain, false, json!("refs/heads/plain"), json!(head));
    unreasoned["locked"] = json!(true);
    let mut prunable = record(&spike, false, json!("refs/heads/dev/spike"), json!(head));
    prunable["prunable"] = json!(true);
    prunable["prune_reason"] = json!("gitdir file points to non-existent location");

    let output = fixture.run(&fixture.base, &["worktree", "list", "acme/core", "--json"]);

    assert_exit(&output, 0);
    assert_eq!(stderr(&output), "");
    let raw = stdout(&output);
    let document: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        document,
        json!({
            "schema_version": 1,
            "worktrees": [
                record(&fixture.primary, true, json!("refs/heads/main"), json!(head)),
                record(&hotfix, false, json!("refs/heads/hotfix"), json!(head)),
                locked,
                unreasoned,
                record(&unborn, false, json!("refs/heads/unborn"), Value::Null),
                prunable,
            ],
            "errors": [],
        })
    );
    // One object and nothing else on stdout, with the documented field order.
    assert!(raw.starts_with("{\n") && raw.ends_with("}\n"), "{raw}");
    let first = &raw[..raw.find("},").unwrap()];
    let order = [
        "schema_version",
        "worktrees",
        "repository_identity",
        "primary_path",
        "\"path\"",
        "is_primary",
        "branch",
        "head",
        "locked",
        "lock_reason",
        "prunable",
        "prune_reason",
    ]
    .map(|key| first.find(key).unwrap());
    assert!(order.is_sorted(), "{first}");

    git(&fixture.primary, &["remote", "remove", "origin"]);
    let originless = fixture.run(&fixture.base, &["worktree", "list", "acme/core", "--json"]);
    assert_exit(&originless, 0);
    let document: Value = serde_json::from_str(&stdout(&originless)).unwrap();
    assert!(
        document["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["repository_identity"].is_null()),
        "{document}"
    );
}

#[test]
fn list_json_reports_metadata_git_does_not_list_and_keeps_the_readable_records() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let admin = fixture.primary.join(".git/worktrees");
    fs::create_dir(admin.join("broken")).unwrap();
    fs::write(admin.join("empty"), "").unwrap();
    let hidden = fixture.linked("hidden", &["-b", "hidden"]);
    let pointer = admin.join("hidden/gitdir");
    fs::set_permissions(&pointer, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&pointer).is_ok() {
        return; // A privileged user reads everything; the case does not exist.
    }

    // Git calls a worktree it cannot reach prunable; the path is unreadable, not gone.
    let sealed = fixture.base.join("sealed");
    let unreachable = sealed.join("wt");
    git(
        &fixture.primary,
        &["worktree", "add", "-q", "-b", "sealed", text(&unreachable)],
    );
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();

    let output = fixture.worktree(&["list", "--json"]);
    let human = fixture.worktree(&["list"]);
    fs::set_permissions(&pointer, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).unwrap();

    assert_exit(&output, 1);
    let document: Value = serde_json::from_str(&stdout(&output)).unwrap();
    let records = document["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            assert_eq!(record["head"], json!(head));
            (
                record["path"].as_str().unwrap(),
                record["prunable"].as_bool().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        records,
        [
            (text(&fixture.primary), false),
            (text(&hotfix), false),
            (text(&unreachable), true),
        ]
    );
    let errors = document["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| {
            assert!(!error["error"].as_str().unwrap().is_empty());
            (
                error["path"].as_str().unwrap(),
                error["code"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        errors,
        [
            (text(&admin.join("broken")), "invalid_metadata"),
            (text(&admin.join("empty")), "invalid_metadata"),
            (text(&admin.join("hidden")), "unreadable"),
            (text(&unreachable), "unreadable"),
        ]
    );
    let diagnostics = stderr(&output);
    assert_eq!(
        diagnostics,
        format!(
            "error: {}: worktree entry has no gitdir file\nerror: {}: worktree entry has no gitdir file\nerror: {}: Permission denied (os error 13)\nerror: {}: Permission denied (os error 13)\n",
            admin.join("broken").display(),
            admin.join("empty").display(),
            admin.join("hidden").display(),
            unreachable.display()
        )
    );
    assert_exit(&human, 1);
    assert_eq!(
        stdout(&human),
        table(&[
            [text(&fixture.primary), "main", "primary"],
            [text(&hotfix), "hotfix", ""],
            [
                text(&unreachable),
                "sealed",
                "prunable: gitdir file points to non-existent location",
            ],
        ])
    );
    assert_eq!(stderr(&human), diagnostics);
    assert!(hidden.join("README").is_file());
}

/// A `git` that fails the calls matching `pattern` and runs every other call natively.
fn failing_git(fixture: &Fixture, pattern: &str, after: usize) -> PathBuf {
    let directory = fixture.base.join("shim");
    fs::create_dir_all(&directory).unwrap();
    let count = directory.join("count");
    let script = format!(
        "#!/bin/sh\ncase \" $* \" in\n  *\" {pattern} \"*)\n    n=$(cat '{count}' 2>/dev/null || echo 0)\n    n=$((n + 1))\n    echo $n > '{count}'\n    if [ $n -gt {after} ]; then echo boom >&2; exit 3; fi;;\nesac\nexec '{git}' \"$@\"\n",
        count = count.display(),
        git = support::real_tool("git").display()
    );
    fs::write(directory.join("git"), script).unwrap();
    fs::set_permissions(directory.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    directory
}

#[test]
fn list_json_reports_a_failed_git_call_after_resolution_and_exits_one() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let listing = |shim: &Path, message: &str| {
        let output = support::lager_with_path(&fixture.home, &fixture.config, Some(shim))
            .current_dir(&fixture.primary)
            .args(["worktree", "list", "--json"])
            .output()
            .unwrap();
        assert_exit(&output, 1);
        assert_eq!(
            stderr(&output),
            format!("error: {}: {message}\n", fixture.primary.display())
        );
        serde_json::from_str::<Value>(&stdout(&output)).unwrap()
    };

    // The second `worktree list` is the listing itself; the first resolves the repository.
    let total = listing(&failing_git(&fixture, "worktree list", 1), "boom");
    assert_eq!(
        total,
        json!({
            "schema_version": 1,
            "worktrees": [],
            "errors": [{"path": text(&fixture.primary), "code": "git_failed", "error": "boom"}],
        })
    );
    fs::remove_dir_all(fixture.base.join("shim")).unwrap();

    let partial = listing(&failing_git(&fixture, "remote", 0), "origin: boom");
    assert_eq!(
        partial,
        json!({
            "schema_version": 1,
            "worktrees": [{
                "repository_identity": null,
                "primary_path": text(&fixture.primary),
                "path": text(&fixture.primary),
                "is_primary": true,
                "branch": "refs/heads/main",
                "head": head,
                "locked": false,
                "lock_reason": null,
                "prunable": false,
                "prune_reason": null,
            }],
            "errors": [{"path": text(&fixture.primary), "code": "git_failed", "error": "origin: boom"}],
        })
    );
}

#[test]
fn list_sorts_by_the_path_text_a_json_reader_compares() {
    let fixture = Fixture::new();
    let sibling = fixture.linked("y", &["-b", "y"]);
    // Native Git places a worktree inside the checkout; as text it sorts after the sibling.
    let nested = fixture.primary.join(".worktrees/x");
    git(
        &fixture.primary,
        &["worktree", "add", "-q", "-b", "x", text(&nested)],
    );

    let output = fixture.worktree(&["list", "--json"]);

    assert_exit(&output, 0);
    let document: Value = serde_json::from_str(&stdout(&output)).unwrap();
    let paths = document["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [text(&fixture.primary), text(&sibling), text(&nested)]
    );
    assert!(paths.is_sorted());
}

#[test]
fn list_escapes_control_characters_in_its_own_text_and_keeps_json_original() {
    let fixture = Fixture::new();
    let pinned = fixture.linked("pinned", &["-b", "pinned"]);
    let reason = "x\u{1b}]2;INJECT\u{7}\ty";
    git(
        &fixture.primary,
        &["worktree", "lock", "--reason", reason, text(&pinned)],
    );

    let human = fixture.worktree(&["list"]);
    assert_eq!(
        stdout(&human),
        table(&[
            [text(&fixture.primary), "main", "primary"],
            [text(&pinned), "pinned", "locked: x\\x1b]2;INJECT\\x07\\ty"],
        ])
    );
    let json = fixture.worktree(&["list", "--json"]);
    let document: Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(document["worktrees"][1]["lock_reason"], json!(reason));
}

// ---- remove ------------------------------------------------------------------------------

fn summary(path: &Path, branch: &str, losses: &str) -> String {
    format!(
        "Remove worktree\n  path      {}\n  branch    {branch}\n  losses    {losses}\n",
        path.display()
    )
}

const LOSSES_REFUSED: &str =
    "error: local changes would be lost; rerun with --force to accept these losses\n";

#[test]
fn remove_deletes_a_clean_worktree_keeps_its_branch_and_counts_unpushed_commits() {
    let fixture = Fixture::new();
    let head = fixture.head();
    git(
        &fixture.primary,
        &["update-ref", "refs/remotes/origin/main", &head],
    );
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    commit(&hotfix, "one", "one");
    let tip = commit(&hotfix, "two", "two");

    let output = fixture.worktree(&["remove", "../core.worktrees/hotfix", "--yes"]);

    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        format!(
            "{}Removed worktree {} · branch hotfix kept\n",
            summary(&hotfix, "hotfix · kept · 2 commits not pushed", "none"),
            hotfix.display()
        )
    );
    assert_eq!(stderr(&output), "");
    assert!(!hotfix.exists());
    fixture.assert_only_primary();
    assert_eq!(
        fixture.branches(),
        [format!("hotfix {tip}"), format!("main {head}")]
    );
    assert!(!fixture.primary.join(".git/worktrees/hotfix").exists());
}

#[test]
fn remove_works_in_an_originless_repository_from_any_directory() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    commit(&hotfix, "one", "one");
    let tip = git(&hotfix, &["rev-parse", "HEAD"]);

    let output = fixture.run(
        &fixture.base,
        &["worktree", "remove", text(&hotfix), "--yes"],
    );

    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        format!(
            "{}Removed worktree {} · branch hotfix kept\n",
            summary(&hotfix, "hotfix · kept", "none"),
            hotfix.display()
        )
    );
    fixture.assert_only_primary();
    assert_eq!(
        fixture.branches(),
        [format!("hotfix {tip}"), format!("main {head}")]
    );
}

/// A worktree with one kind of local state: refused without `--force`, removed with it.
fn remove_discloses(prepare: impl Fn(&Path), losses: &str) {
    let fixture = Fixture::new();
    fs::write(fixture.primary.join(".gitignore"), "*.log\n").unwrap();
    git(&fixture.primary, &["add", ".gitignore"]);
    git(&fixture.primary, &["commit", "-qm", "ignore"]);
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    prepare(&hotfix);
    let before = (fixture.state(), tree(&hotfix));

    for args in [&["--yes"][..], &[]] {
        let refused = fixture.worktree(&[&["remove", text(&hotfix)], args].concat());
        assert_eq!(
            refused.status.code(),
            Some(if args.is_empty() { 2 } else { 1 })
        );
        if !args.is_empty() {
            assert_eq!(stdout(&refused), summary(&hotfix, "hotfix · kept", losses));
            assert_eq!(stderr(&refused), LOSSES_REFUSED);
        }
        assert_eq!((fixture.state(), tree(&hotfix)), before);
    }

    let forced = fixture.worktree(&["remove", text(&hotfix), "--yes", "--force"]);
    assert_exit(&forced, 0);
    assert_eq!(
        stdout(&forced),
        format!(
            "{}Removed worktree {} · branch hotfix kept\n",
            summary(&hotfix, "hotfix · kept", losses),
            hotfix.display()
        )
    );
    assert_eq!(stderr(&forced), "");
    assert!(!hotfix.exists());
    fixture.assert_only_primary();
    assert_eq!(fixture.branches(), before.0.1);
}

#[test]
fn remove_discloses_modified_files_and_needs_force() {
    remove_discloses(
        |worktree| fs::write(worktree.join("README"), "changed\n").unwrap(),
        "1 modified file",
    );
}

#[test]
fn remove_discloses_staged_renames_and_needs_force() {
    remove_discloses(
        |worktree| {
            git(worktree, &["mv", "README", "README.md"]);
            fs::write(worktree.join(".gitignore"), "*.log\n*.tmp\n").unwrap();
        },
        "2 modified files",
    );
}

#[test]
fn remove_discloses_untracked_files_and_needs_force() {
    remove_discloses(
        |worktree| fs::write(worktree.join("new"), "new\n").unwrap(),
        "1 untracked file",
    );
}

#[test]
fn remove_discloses_ignored_files_and_needs_force() {
    remove_discloses(
        |worktree| fs::write(worktree.join("build.log"), "log\n").unwrap(),
        "ignored files",
    );
}

#[test]
fn remove_discloses_every_class_of_loss_together() {
    remove_discloses(
        |worktree| {
            fs::write(worktree.join("README"), "changed\n").unwrap();
            fs::write(worktree.join(".gitignore"), "*.log\n*.tmp\n").unwrap();
            fs::write(worktree.join("new"), "new\n").unwrap();
            fs::write(worktree.join("build.log"), "log\n").unwrap();
        },
        "2 modified files, 1 untracked file, ignored files",
    );
}

#[test]
fn remove_discloses_files_hidden_from_git_status_and_needs_force() {
    for flag in ["--skip-worktree", "--assume-unchanged"] {
        remove_discloses(
            |worktree| {
                git(worktree, &["update-index", flag, "README"]);
                fs::write(worktree.join("README"), "edited behind the flag\n").unwrap();
                assert_eq!(git(worktree, &["status", "--porcelain"]), "");
            },
            "1 modified file",
        );
    }
}

/// A repository inside the worktree blocks its removal, with and without `--force`.
fn remove_blocks_nested(prepare: impl Fn(&Path), nested: &str) {
    let fixture = Fixture::new();
    fs::write(fixture.primary.join(".gitignore"), "build/\n").unwrap();
    git(&fixture.primary, &["add", ".gitignore"]);
    git(&fixture.primary, &["commit", "-qm", "ignore"]);
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    prepare(&hotfix);
    let before = (fixture.state(), tree(&hotfix));

    for args in [&["--yes"][..], &["--yes", "--force"]] {
        let output = fixture.worktree(&[&["remove", text(&hotfix)], args].concat());
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!("error: blocked: nested repository {nested}\n")
        );
        assert_eq!((fixture.state(), tree(&hotfix)), before);
    }
    assert_eq!(
        git(&hotfix.join(nested), &["log", "--format=%s"]),
        "initial"
    );
}

#[test]
fn remove_blocks_an_untracked_nested_repository_even_with_force() {
    remove_blocks_nested(
        |worktree| init(&worktree.join("tools/inner")),
        "tools/inner",
    );
}

#[test]
fn remove_blocks_an_ignored_directory_that_is_a_repository_even_with_force() {
    remove_blocks_nested(|worktree| init(&worktree.join("build")), "build");
}

#[test]
fn remove_counts_a_repository_deep_inside_an_ignored_directory_as_ignored_files() {
    remove_discloses(
        |worktree| {
            init(&worktree.join("cache.log/deps/inner"));
            assert!(worktree.join("cache.log/deps/inner/.git").is_dir());
        },
        "ignored files",
    );
}

#[test]
fn remove_blocks_a_repository_inside_a_directory_with_tracked_files_even_with_force() {
    remove_blocks_nested(
        |worktree| {
            fs::create_dir(worktree.join("sub")).unwrap();
            commit(worktree, "sub/f", "tracked");
            let sub = worktree.join("sub");
            git(&sub, &["init", "-q", "-b", "main"]);
            git(&sub, &["add", "f"]);
            git(&sub, &["commit", "-qm", "initial"]);
            // Git reads the directory as plain tracked files and reports nothing.
            assert_eq!(
                git(
                    worktree,
                    &["status", "--porcelain", "-uall", "--ignored=matching"]
                ),
                ""
            );
        },
        "sub",
    );
}

#[test]
fn remove_counts_a_tracked_directory_replaced_by_a_file_as_plain_losses() {
    remove_discloses(
        |worktree| {
            fs::create_dir(worktree.join("sub")).unwrap();
            commit(worktree, "sub/f", "tracked");
            fs::remove_dir_all(worktree.join("sub")).unwrap();
            fs::write(worktree.join("sub"), "now a file\n").unwrap();
        },
        "1 modified file, 1 untracked file",
    );
}

#[test]
fn remove_blocks_an_untracked_directory_holding_a_git_entry_even_with_force() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    // Git accepts neither entry as a repository and lists the files beside it as untracked.
    fs::create_dir_all(hotfix.join("deep/as-dir/.git")).unwrap();
    fs::write(hotfix.join("deep/as-dir/file"), "work\n").unwrap();
    fs::create_dir(hotfix.join("as-file")).unwrap();
    fs::write(hotfix.join("as-file/.git"), "gitdir: /nowhere\n").unwrap();
    fs::write(hotfix.join("as-file/file"), "work\n").unwrap();
    assert_eq!(
        git(&hotfix, &["status", "--porcelain", "-uall"]),
        "?? as-file/file\n?? deep/as-dir/file"
    );
    let before = (fixture.state(), tree(&hotfix));

    for args in [&["--yes"][..], &["--yes", "--force"]] {
        let output = fixture.worktree(&[&["remove", text(&hotfix)], args].concat());
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            "error: blocked: nested repository as-file\nerror: blocked: nested repository deep/as-dir\n"
        );
        assert_eq!((fixture.state(), tree(&hotfix)), before);
    }
}

#[test]
fn remove_does_not_count_a_grafts_file_as_protection() {
    let (fixture, bisect, lost) = detached();
    let main = fixture.head();
    // With the graft honoured, `main` has the lost commit as its parent.
    fs::write(
        fixture.primary.join(".git/info/grafts"),
        format!("{main} {lost}\n"),
    )
    .unwrap();
    assert_eq!(
        git(
            &fixture.primary,
            &["branch", "--contains", &lost, "--format=%(refname:short)"]
        ),
        "main"
    );

    assert_detached_blocked(&fixture, &bisect, &lost);
}

#[test]
fn remove_blocks_submodule_repositories_git_keeps_for_the_worktree() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let modules = fixture.primary.join(".git/worktrees/hotfix/modules");
    fs::create_dir(&modules).unwrap();
    let before = (fixture.state(), tree(&hotfix));

    let output = fixture.worktree(&["remove", text(&hotfix), "--yes", "--force"]);

    assert_exit(&output, 1);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        format!(
            "error: blocked: Git keeps submodule repositories of this worktree in {}\n",
            modules.display()
        )
    );
    assert_eq!((fixture.state(), tree(&hotfix)), before);
    assert!(modules.is_dir());
}

#[test]
fn remove_blocks_a_worktree_whose_head_git_cannot_name() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["--detach"]);
    fs::write(
        fixture.primary.join(".git/worktrees/hotfix/HEAD"),
        format!("{}\n", "0".repeat(40)),
    )
    .unwrap();
    let before = tree(&hotfix);

    let output = fixture.worktree(&["remove", text(&hotfix), "--yes", "--force"]);

    assert_exit(&output, 1);
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).starts_with("error: blocked: HEAD is unknown\n"),
        "{output:?}"
    );
    assert_eq!(tree(&hotfix), before);
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix]);
}

#[test]
fn unknown_configuration_keys_warn_on_stderr_and_keep_json_stdout_clean() {
    let fixture = Fixture::new();
    fs::write(&fixture.config, "root = \"repos\"\nmystery = 1\n").unwrap();

    let output = fixture.worktree(&["list", "--json"]);

    assert_exit(&output, 0);
    assert_eq!(
        stderr(&output),
        "lager: warning: unknown config key `mystery`\n"
    );
    let document: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(document["errors"], json!([]));
}

/// A detached worktree `bisect` whose HEAD commit no branch holds, and that commit.
fn detached() -> (Fixture, PathBuf, String) {
    let fixture = Fixture::new();
    let bisect = fixture.linked("bisect", &["--detach"]);
    let lost = commit(&bisect, "lost", "lost");
    (fixture, bisect, lost)
}

fn assert_detached_blocked(fixture: &Fixture, bisect: &Path, lost: &str) {
    let before = (fixture.state(), tree(bisect));
    for args in [&["--yes"][..], &["--yes", "--force"]] {
        let output = fixture.worktree(&[&["remove", text(bisect)], args].concat());
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!(
                "error: blocked: detached HEAD {} is not reachable from a branch, tag or remote ref\nhint: keep the commit first: git -C {} branch keep-bisect\n",
                short(lost),
                bisect.display()
            )
        );
        assert_eq!((fixture.state(), tree(bisect)), before);
        assert_eq!(git(bisect, &["rev-parse", "HEAD"]), lost);
    }
}

#[test]
fn remove_blocks_an_unprotected_detached_head_even_with_force() {
    let (fixture, bisect, lost) = detached();
    assert_detached_blocked(&fixture, &bisect, &lost);
}

#[test]
fn remove_does_not_count_stash_pseudo_worktree_notes_or_replace_refs_as_protection() {
    let (fixture, bisect, lost) = detached();
    let main = fixture.head();
    for reference in [
        "refs/stash",
        "ORIG_HEAD",
        "refs/worktree/keep",
        "refs/bisect/bad",
        "refs/notes/keep",
        "refs/keep/custom",
    ] {
        git(&bisect, &["update-ref", reference, &lost]);
    }
    git(&fixture.primary, &["update-ref", "ORIG_HEAD", &lost]);
    // With replacement honoured, `main` would have the lost commit as its parent.
    git(&fixture.primary, &["replace", "--graft", &main, &lost]);
    assert_eq!(
        git(
            &fixture.primary,
            &["branch", "--contains", &lost, "--format=%(refname:short)"]
        ),
        "main"
    );
    // A tag that does not peel to a commit holds no commit.
    let tree_id = git(
        &fixture.primary,
        &["rev-parse", &format!("{lost}^{{tree}}")],
    );
    git(&fixture.primary, &["tag", "tree-tag", &tree_id]);

    assert_detached_blocked(&fixture, &bisect, &lost);
}

fn remove_protected(protect: impl Fn(&Fixture, &str), protecting: &str) {
    let (fixture, bisect, lost) = detached();
    protect(&fixture, &lost);
    let branches = fixture.branches();

    let output = fixture.worktree(&["remove", text(&bisect), "--yes"]);

    assert_exit(&output, 0);
    let kept = format!("detached {} · kept by {protecting}", short(&lost));
    assert_eq!(
        stdout(&output),
        format!(
            "{}Removed worktree {} · {kept}\n",
            summary(&bisect, &kept, "none"),
            bisect.display()
        )
    );
    assert!(!bisect.exists());
    assert!(!fixture.listed().contains(&bisect));
    assert_eq!(fixture.branches(), branches);
    assert_eq!(git(&fixture.primary, &["cat-file", "-t", &lost]), "commit");
}

#[test]
fn remove_allows_a_detached_head_kept_by_a_branch() {
    remove_protected(
        |fixture, lost| {
            // The branch is ahead of the detached commit, which is its ancestor.
            git(&fixture.primary, &["branch", "keeper", lost]);
            let keeper = fixture.linked("keeper", &["keeper"]);
            commit(&keeper, "more", "more");
        },
        "refs/heads/keeper",
    );
}

#[test]
fn remove_allows_a_detached_head_kept_by_an_annotated_tag() {
    remove_protected(
        |fixture, lost| {
            git(&fixture.primary, &["tag", "-a", "-m", "keep", "v9", lost]);
        },
        "refs/tags/v9",
    );
}

#[test]
fn remove_allows_a_detached_head_kept_by_a_remote_ref() {
    remove_protected(
        |fixture, lost| {
            git(
                &fixture.primary,
                &["update-ref", "refs/remotes/origin/topic", lost],
            );
        },
        "refs/remotes/origin/topic",
    );
}

#[test]
fn remove_blocks_a_head_on_a_ref_outside_refs_heads_even_with_force() {
    for reference in ["refs/worktree/keep", "refs/custom/keep"] {
        let fixture = Fixture::new();
        let bisect = fixture.linked("bisect", &["--detach"]);
        git(&bisect, &["update-ref", reference, "HEAD"]);
        git(&bisect, &["symbolic-ref", "HEAD", reference]);
        let lost = commit(&bisect, "lost", "lost");
        assert!(
            git(&fixture.primary, &["worktree", "list", "--porcelain"])
                .contains(&format!("\nbranch {reference}"))
        );

        // The listing reports the ref as Git does.
        let listing = fixture.worktree(&["list", "--json"]);
        assert_exit(&listing, 0);
        let document: Value = serde_json::from_str(&stdout(&listing)).unwrap();
        assert_eq!(document["worktrees"][1]["path"], json!(text(&bisect)));
        assert_eq!(document["worktrees"][1]["branch"], json!(reference));
        assert!(stdout(&fixture.worktree(&["list"])).contains(&format!("  {reference}\n")));

        assert_detached_blocked(&fixture, &bisect, &lost);
        assert_eq!(git(&bisect, &["symbolic-ref", "HEAD"]), reference);
    }
}

#[test]
fn remove_names_the_shared_ref_that_holds_the_commit_of_a_head_outside_refs_heads() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let bisect = fixture.linked("bisect", &["--detach"]);
    git(&bisect, &["update-ref", "refs/worktree/keep", "HEAD"]);
    git(&bisect, &["symbolic-ref", "HEAD", "refs/worktree/keep"]);
    let branches = fixture.branches();

    // The ref only this worktree sees goes with it, so it is a named loss.
    let output = fixture.worktree(&["remove", text(&bisect), "--yes", "--force"]);

    assert_exit(&output, 0);
    let kept = format!("detached {} · kept by refs/heads/main", short(&head));
    assert_eq!(
        stdout(&output),
        format!(
            "{}Removed worktree {} · {kept}\n",
            summary(&bisect, &kept, "per-worktree ref refs/worktree/keep"),
            bisect.display()
        )
    );
    assert!(!bisect.exists());
    fixture.assert_only_primary();
    assert_eq!(fixture.branches(), branches);
}

#[test]
fn remove_blocks_when_git_cannot_prove_reachability_submodule_or_local_state() {
    let cases = [
        (
            "for-each-ref",
            "cannot prove that detached HEAD {head} is kept by a branch, tag or remote ref: boom\nhint: keep the commit first: git -C {path} branch keep-bisect\nerror: blocked: cannot read the refs of this worktree: boom",
        ),
        (
            "ls-files",
            "submodule state is unknown: boom\nerror: blocked: local state is unknown: boom",
        ),
        ("status", "local state is unknown: boom"),
    ];
    for (call, message) in cases {
        let (fixture, bisect, lost) = detached();
        // The commit is held by a tag, so only the failed call stands in the way.
        git(&fixture.primary, &["tag", "keep", &lost]);
        let shim = failing_git(&fixture, call, 0);
        let before = (fixture.state(), tree(&bisect));

        let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&shim))
            .current_dir(&fixture.primary)
            .args(["worktree", "remove", text(&bisect), "--yes", "--force"])
            .output()
            .unwrap();

        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "", "{call}");
        assert_eq!(
            stderr(&output),
            format!(
                "error: blocked: {}\n",
                message
                    .replace("{head}", short(&lost))
                    .replace("{path}", text(&bisect))
            ),
            "{call}"
        );
        assert_eq!((fixture.state(), tree(&bisect)), before, "{call}");
    }
}

#[test]
fn remove_refuses_the_primary_checkout() {
    let fixture = Fixture::new();
    fixture.linked("hotfix", &["-b", "hotfix"]);
    let before = (fixture.state(), tree(&fixture.primary));
    let blocked = format!(
        "error: blocked: {} is the primary checkout, not a linked worktree\n",
        fixture.primary.display()
    );

    let output = fixture.run(
        &fixture.base,
        &[
            "worktree",
            "remove",
            text(&fixture.primary),
            "--yes",
            "--force",
        ],
    );
    assert_exit(&output, 1);
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), blocked);

    git(
        &fixture.primary,
        &["remote", "add", "origin", "git@github.com:acme/core.git"],
    );
    let hinted = fixture.worktree(&["remove", ".", "--yes", "--force"]);
    assert_exit(&hinted, 1);
    assert_eq!(
        stderr(&hinted),
        format!("{blocked}hint: lager remove github.com/acme/core\n")
    );
    assert_eq!((fixture.state(), tree(&fixture.primary)), before);
}

#[test]
fn remove_refuses_paths_that_are_not_a_worktree_root() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    fs::create_dir_all(hotfix.join("src")).unwrap();
    fs::write(hotfix.join("src/keep"), "keep\n").unwrap();
    git(&hotfix, &["add", "src/keep"]);
    git(&hotfix, &["commit", "-qm", "src"]);
    let plain = fixture.base.join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("keep"), "keep\n").unwrap();
    let link = fixture.base.join("link");
    std::os::unix::fs::symlink(&hotfix, &link).unwrap();
    let before = (fixture.state(), tree(&fixture.base));
    let cases = [
        (
            plain.clone(),
            format!("{} is not a Git worktree", plain.display()),
        ),
        (
            plain.join("keep"),
            format!("{} is not a Git worktree", plain.join("keep").display()),
        ),
        (
            hotfix.join("src"),
            format!(
                "{} is inside the worktree {}, not a worktree itself",
                hotfix.join("src").display(),
                hotfix.display()
            ),
        ),
        (
            link.clone(),
            format!("{} is a symlink, not a worktree", link.display()),
        ),
    ];
    for (path, message) in cases {
        let output = fixture.worktree(&["remove", text(&path), "--yes", "--force"]);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(stderr(&output), format!("error: blocked: {message}\n"));
        assert_eq!((fixture.state(), tree(&fixture.base)), before);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn remove_refuses_a_directory_the_repository_does_not_name_back() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let copy = fixture.base.join("copy");
    copy_tree(&hotfix, &copy);
    let moved = fixture.linked("moving", &["-b", "moving"]);
    let renamed = fixture.container().join("renamed");
    fs::rename(&moved, &renamed).unwrap();
    // Two worktrees whose `.git` files are swapped: Git lists each path, neither is named back.
    let left = fixture.linked("left", &["-b", "left"]);
    let right = fixture.linked("right", &["-b", "right"]);
    fs::copy(right.join(".git"), left.join(".git")).unwrap();
    // A worktree whose `.git` file names a Git directory outside the repository's own.
    let foreign = fixture.linked("foreign", &["-b", "foreign"]);
    let admin = fixture.primary.join(".git/worktrees/foreign");
    let outside = fixture.base.join("outside-admin");
    copy_tree(&admin, &outside);
    fs::write(
        outside.join("commondir"),
        format!("{}\n", fixture.primary.join(".git").display()),
    )
    .unwrap();
    fs::write(
        foreign.join(".git"),
        format!("gitdir: {}\n", outside.display()),
    )
    .unwrap();
    assert_eq!(
        git(&foreign, &["rev-parse", "--git-common-dir"]),
        text(&fixture.primary.join(".git"))
    );
    // A directory whose `.git` file names a worktree of another repository.
    let other = fixture.base.join("other");
    init(&other);
    git(
        &other,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wb",
            text(&fixture.base.join("wb")),
        ],
    );
    let tampered = fixture.base.join("tampered");
    copy_tree(&hotfix, &tampered);
    fs::write(
        tampered.join(".git"),
        format!("gitdir: {}\n", other.join(".git/worktrees/wb").display()),
    )
    .unwrap();
    let before = (fixture.state(), tree(&fixture.base));

    let admin = fixture.primary.join(".git/worktrees");
    for (path, git_dir) in [
        (&copy, admin.join("hotfix")),
        (&renamed, admin.join("moving")),
        (&left, admin.join("right")),
        (&foreign, outside.clone()),
        (&tampered, other.join(".git/worktrees/wb")),
    ] {
        let output = fixture.worktree(&["remove", text(path), "--yes", "--force"]);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        // What is wrong, and no command: a repair would bind the path to the named directory.
        assert_eq!(
            stderr(&output),
            format!(
                "error: blocked: {} and the Git directory its .git file names, {}, do not name each other\n",
                path.display(),
                git_dir.display()
            )
        );
        assert_eq!((fixture.state(), tree(&fixture.base)), before);
    }
    assert_eq!(
        git(&other, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(fixture.primary.join(".git/worktrees/moving/gitdir"))
            .unwrap()
            .trim(),
        text(&moved.join(".git"))
    );
}

#[test]
fn remove_refuses_a_locked_worktree_and_names_the_unlock_command() {
    let fixture = Fixture::new();
    let pinned = fixture.linked("pinned", &["-b", "pinned"]);
    let bare = fixture.linked("bare-lock", &["-b", "bare-lock"]);
    git(
        &fixture.primary,
        &[
            "worktree",
            "lock",
            "--reason",
            "reviewing on laptop",
            text(&pinned),
        ],
    );
    git(&fixture.primary, &["worktree", "lock", text(&bare)]);
    let before = (fixture.state(), tree(&fixture.container()));

    for (path, message) in [
        (&pinned, "worktree is locked: reviewing on laptop"),
        (&bare, "worktree is locked"),
    ] {
        let output = fixture.worktree(&["remove", text(path), "--yes", "--force"]);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!(
                "error: blocked: {message}\nhint: git -C {} worktree unlock {}\n",
                fixture.primary.display(),
                path.display()
            )
        );
    }
    assert_eq!((fixture.state(), tree(&fixture.container())), before);
    assert!(
        fixture
            .primary
            .join(".git/worktrees/pinned/locked")
            .is_file()
    );
    assert!(
        fixture
            .primary
            .join(".git/worktrees/bare-lock/locked")
            .is_file()
    );
}

#[test]
fn remove_reports_an_absent_path_with_prune_guidance_and_never_prunes() {
    let fixture = Fixture::new();
    let stale = fixture.linked("stale", &["-b", "stale"]);
    fs::remove_dir_all(&stale).unwrap();
    let metadata = fixture.primary.join(".git/worktrees/stale");
    let pointer = fs::read(metadata.join("gitdir")).unwrap();
    let before = fixture.state();

    let inside = fixture.worktree(&["remove", text(&stale), "--yes", "--force"]);
    assert_exit(&inside, 1);
    assert_eq!(stdout(&inside), "");
    assert_eq!(
        stderr(&inside),
        format!(
            "error: blocked: path is absent; Git still lists this worktree as prunable\nhint: git -C {} worktree prune\n",
            fixture.primary.display()
        )
    );
    let outside = fixture.run(
        &fixture.base,
        &["worktree", "remove", text(&stale), "--yes"],
    );
    assert_exit(&outside, 1);
    assert_eq!(
        stderr(&outside),
        format!(
            "error: blocked: path is absent: {}\nhint: if Git still lists it, run inside its repository: git worktree prune\n",
            stale.display()
        )
    );
    // A worktree is named by its path; its branch name is just a path that does not exist.
    let by_branch = fixture.worktree(&["remove", "stale", "--yes"]);
    assert_exit(&by_branch, 1);
    assert_eq!(
        stderr(&by_branch),
        format!(
            "error: blocked: path is absent: {}\nhint: if Git still lists it, run inside its repository: git worktree prune\n",
            fixture.primary.join("stale").display()
        )
    );
    fixture.assert_unchanged(&before);
    assert_eq!(fs::read(metadata.join("gitdir")).unwrap(), pointer);
}

/// The primary gains a committed submodule at `vendor/theme`.
fn with_submodule() -> Fixture {
    let fixture = Fixture::new();
    let source = fixture.base.join("theme-source");
    init(&source);
    git(
        &fixture.primary,
        &["submodule", "add", "-q", text(&source), "vendor/theme"],
    );
    git(&fixture.primary, &["commit", "-qm", "submodule"]);
    fixture
}

#[test]
fn remove_blocks_a_populated_submodule_even_with_force() {
    let fixture = with_submodule();
    let docs = fixture.linked("docs", &["-b", "docs"]);
    git(&docs, &["submodule", "update", "-q", "--init"]);
    assert!(docs.join("vendor/theme/README").is_file());
    let before = (fixture.state(), tree(&docs));

    for args in [&["--yes"][..], &["--yes", "--force"]] {
        let output = fixture.worktree(&[&["remove", text(&docs)], args].concat());
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!(
                "error: blocked: initialized submodule vendor/theme\nhint: git -C {} submodule deinit vendor/theme\n",
                docs.display()
            )
        );
        assert_eq!((fixture.state(), tree(&docs)), before);
    }
}

#[test]
fn remove_allows_a_worktree_whose_submodule_is_declared_but_unpopulated() {
    let fixture = with_submodule();
    let docs = fixture.linked("docs", &["-b", "docs"]);
    assert!(docs.join(".gitmodules").is_file());
    assert_eq!(tree(&docs.join("vendor/theme")), Vec::<String>::new());

    let output = fixture.worktree(&["remove", text(&docs), "--yes"]);

    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        format!(
            "{}Removed worktree {} · branch docs kept\n",
            summary(&docs, "docs · kept", "none"),
            docs.display()
        )
    );
    assert!(!docs.exists());
    fixture.assert_only_primary();
}

const GIT_STATUS: [&str; 4] = ["status", "--porcelain", "-uall", "--ignored=matching"];

/// `prepare` fills the unpopulated submodule directory `vendor/theme` of a linked worktree;
/// `intact` reads it after the refused removals.
fn remove_blocks_unreported_submodule_files(prepare: impl Fn(&Path), intact: impl Fn(&Path)) {
    let fixture = with_submodule();
    let docs = fixture.linked("docs", &["-b", "docs"]);
    let theme = docs.join("vendor/theme");
    prepare(&theme);
    assert!(!theme.join(".git").exists());
    // Git does not walk a directory its index names as a submodule.
    assert_eq!(git(&docs, &GIT_STATUS), "");
    let before = (fixture.state(), tree(&docs));

    for args in [&["--yes"][..], &["--yes", "--force"]] {
        let output = fixture.worktree(&[&["remove", text(&docs)], args].concat());
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            "error: blocked: submodule state is unknown: vendor/theme: unpopulated submodule directory holds files Git does not report\n"
        );
        assert_eq!((fixture.state(), tree(&docs)), before);
    }
    intact(&theme);
}

#[test]
fn remove_blocks_a_file_inside_an_unpopulated_submodule_directory_even_with_force() {
    remove_blocks_unreported_submodule_files(
        |theme| fs::write(theme.join("notes.txt"), "notes\n").unwrap(),
        |theme| {
            assert_eq!(
                fs::read_to_string(theme.join("notes.txt")).unwrap(),
                "notes\n"
            )
        },
    );
}

#[test]
fn remove_blocks_a_repository_inside_an_unpopulated_submodule_directory_even_with_force() {
    remove_blocks_unreported_submodule_files(
        |theme| init(&theme.join("inner")),
        |theme| {
            assert_eq!(
                git(&theme.join("inner"), &["log", "--format=%s"]),
                "initial"
            )
        },
    );
}

/// A linked worktree `hotfix` whose one commit conflicts with `main` in `a`, with an
/// uncommitted edit to the tracked `README`, the only content an autostash would hold.
fn diverged() -> (Fixture, PathBuf) {
    let fixture = Fixture::new();
    commit(&fixture.primary, "a", "base");
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    commit(&hotfix, "a", "topic");
    commit(&fixture.primary, "a", "main");
    fs::write(hotfix.join("README"), "edited\n").unwrap();
    (fixture, hotfix)
}

/// Asserts that the autostash in `file` of the worktree's Git directory blocks the removal.
/// `{path}` in `hint` stands for the worktree.
fn assert_autostash_blocked(
    fixture: &Fixture,
    hotfix: &Path,
    file: &str,
    holder: &str,
    hint: &str,
) {
    let autostash = fixture.primary.join(".git/worktrees/hotfix").join(file);
    let held = fs::read_to_string(&autostash).unwrap();
    assert_eq!(
        git(hotfix, &["show", "--format=", "--name-only", held.trim()]),
        "README"
    );
    assert_eq!(git(&fixture.primary, &["stash", "list"]), "");
    let before = (fixture.state(), tree(hotfix));

    for args in [&["--yes"][..], &["--yes", "--force"]] {
        let output = fixture.worktree(&[&["remove", text(hotfix)], args].concat());
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            format!(
                "error: blocked: {holder} holds an autostash in {}\nhint: {}\n",
                autostash.display(),
                hint.replace("{path}", text(hotfix))
            )
        );
        assert_eq!((fixture.state(), tree(hotfix)), before);
        assert_eq!(fs::read_to_string(&autostash).unwrap(), held);
    }
}

#[test]
fn remove_blocks_the_autostash_of_a_rebase_stopped_on_a_break_even_with_force() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    fs::write(hotfix.join("README"), "edited\n").unwrap();
    git(
        &hotfix,
        &[
            "-c",
            "sequence.editor=/bin/sh -c 'echo break >\"$1\"' --",
            "rebase",
            "-i",
            "--autostash",
            "main",
        ],
    );
    // Nothing shows the edit: the tree is clean and HEAD sits on the commit `main` holds.
    assert_eq!(git(&hotfix, &GIT_STATUS), "");
    assert_eq!(git(&hotfix, &["rev-parse", "HEAD"]), fixture.head());

    assert_autostash_blocked(
        &fixture,
        &hotfix,
        "rebase-merge/autostash",
        "a stopped rebase",
        "git -C {path} rebase --abort",
    );
}

#[test]
fn remove_blocks_the_autostash_of_a_conflicted_apply_rebase_even_with_force() {
    let (fixture, hotfix) = diverged();
    let stopped = git_output(&hotfix, &["rebase", "--apply", "--autostash", "main"]);
    assert!(!stopped.status.success(), "{stopped:?}");
    assert_eq!(git(&hotfix, &GIT_STATUS), "UU a");
    assert_eq!(git(&hotfix, &["rev-parse", "HEAD"]), fixture.head());

    assert_autostash_blocked(
        &fixture,
        &hotfix,
        "rebase-apply/autostash",
        "a stopped rebase",
        "git -C {path} rebase --abort",
    );
}

#[test]
fn remove_blocks_the_autostash_of_a_conflicted_merge_even_with_force() {
    let (fixture, hotfix) = diverged();
    let stopped = git_output(&hotfix, &["merge", "--autostash", "main"]);
    assert!(!stopped.status.success(), "{stopped:?}");
    assert_eq!(git(&hotfix, &GIT_STATUS), "UU a");

    assert_autostash_blocked(
        &fixture,
        &hotfix,
        "MERGE_AUTOSTASH",
        "a stopped merge",
        "git -C {path} merge --abort",
    );
}

#[test]
fn remove_blocks_a_hand_planted_checkout_autostash_even_with_force() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    fs::write(hotfix.join("README"), "edited\n").unwrap();
    let held = git(&hotfix, &["stash", "create"]);
    git(&hotfix, &["checkout", "-q", "--", "README"]);
    // Planted by hand: Git leaves this file only when `checkout -m` is interrupted.
    fs::write(
        fixture
            .primary
            .join(".git/worktrees/hotfix/CHECKOUT_AUTOSTASH_HEAD"),
        format!("{held}\n"),
    )
    .unwrap();
    assert_eq!(git(&hotfix, &GIT_STATUS), "");

    assert_autostash_blocked(
        &fixture,
        &hotfix,
        "CHECKOUT_AUTOSTASH_HEAD",
        "an interrupted checkout",
        "apply the commit that file names, then delete the file: git -C {path} stash apply COMMIT",
    );
}

#[test]
fn remove_outside_a_terminal_requires_yes() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let before = (fixture.state(), tree(&hotfix));

    for args in [&[][..], &["--force"]] {
        let output = fixture.worktree(&[&["remove", text(&hotfix)], args].concat());
        assert_exit(&output, 2);
        assert_eq!(stdout(&output), "");
        assert_eq!(
            stderr(&output),
            "error: worktree remove requires --yes outside a terminal\n"
        );
        assert_eq!((fixture.state(), tree(&hotfix)), before);
    }
}

#[test]
fn git_receives_force_only_for_accepted_losses_and_no_fetch_prune_or_repair() {
    let fixture = Fixture::new();
    let head = fixture.head();
    let shim = fixture.base.join("logging");
    fs::create_dir_all(&shim).unwrap();
    let log = shim.join("log");
    fs::write(
        shim.join("git"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            support::real_tool("git").display()
        ),
    )
    .unwrap();
    fs::set_permissions(shim.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let run = |args: &[&str]| {
        let output = support::lager_with_path(&fixture.home, &fixture.config, Some(&shim))
            .current_dir(&fixture.primary)
            .args(args)
            .output()
            .unwrap();
        assert_exit(&output, 0);
    };
    let clean = fixture.container().join("clean");
    let dirty = fixture.container().join("dirty");

    run(&["worktree", "add", ".", "clean"]);
    run(&["worktree", "add", ".", "dirty"]);
    fs::write(dirty.join("draft"), "draft\n").unwrap();
    run(&["worktree", "remove", text(&clean), "--yes", "--force"]);
    run(&["worktree", "remove", text(&dirty), "--yes", "--force"]);

    let calls = fs::read_to_string(&log).unwrap();
    let mutations = calls
        .lines()
        .filter(|call| call.contains(" worktree add ") || call.contains(" worktree remove "))
        .collect::<Vec<_>>();
    let primary = fixture.primary.display();
    assert_eq!(
        mutations,
        [
            format!(
                "-C {primary} worktree add -b clean -- {} {head}",
                clean.display()
            ),
            format!(
                "-C {primary} worktree add -b dirty -- {} {head}",
                dirty.display()
            ),
            format!("-C {primary} worktree remove -- {}", clean.display()),
            format!(
                "-C {primary} worktree remove --force -- {}",
                dirty.display()
            ),
        ]
    );
    for forbidden in [
        "fetch", "pull", "prune", "repair", "branch -", "reset", "-B",
    ] {
        assert!(!calls.contains(forbidden), "{forbidden}: {calls}");
    }
    fixture.assert_only_primary();
}

// ---- remove: interactive confirmation ----------------------------------------------------

/// Runs `command` on a terminal, sends `answer` once `ready` is on screen, and returns the
/// exit status with everything the terminal showed. An empty `ready` expects no prompt.
fn terminal_session(
    mut command: Command,
    ready: &str,
    answer: &[u8],
    before_answer: impl FnOnce(),
) -> (Option<i32>, String) {
    use rustix_openpty::rustix::termios::{LocalModes, tcgetattr};
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let pty = rustix_openpty::openpty(None, None).unwrap();
    let mut writer = fs::File::from(pty.controller);
    let mut reader = writer.try_clone().unwrap();
    let terminal = fs::File::from(pty.user);
    command
        .env("TERM", "xterm")
        .stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal.try_clone().unwrap()))
        .stderr(Stdio::from(terminal));
    let mut child = command.spawn().unwrap();
    drop(command);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(length) = reader.read(&mut buffer) {
            if length == 0 || sender.send(buffer[..length].to_vec()).is_err() {
                break;
            }
        }
    });
    // Has to hold in a parallel suite on a loaded machine.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut transcript = String::new();
    let mut before_answer = Some(before_answer);
    loop {
        if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(20)) {
            transcript.push_str(&String::from_utf8_lossy(&bytes));
        }
        // The prompt is drawn before the terminal reads keys raw; a key sent earlier is lost.
        if before_answer.is_some()
            && !ready.is_empty()
            && transcript.contains(ready)
            && !tcgetattr(&writer)
                .unwrap()
                .local_modes
                .intersects(LocalModes::ICANON | LocalModes::ISIG)
        {
            before_answer.take().unwrap()();
            writer.write_all(answer).unwrap();
            writer.flush().unwrap();
        }
        if let Some(status) = child.try_wait().unwrap() {
            while let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(20)) {
                transcript.push_str(&String::from_utf8_lossy(&bytes));
            }
            assert!(
                ready.is_empty() || before_answer.is_none(),
                "exited before the prompt: {transcript:?}"
            );
            return (status.code(), transcript.replace("\r\n", "\n"));
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("terminal session timed out: {transcript:?}");
        }
    }
}

/// Removes a worktree `hotfix` on a terminal; with `--force` it holds an untracked `draft`.
fn confirm(
    args: &[&str],
    ready: &str,
    answer: &[u8],
    before_answer: impl FnOnce(&Path),
) -> (Fixture, PathBuf, Option<i32>, String) {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    if args.contains(&"--force") {
        fs::write(hotfix.join("draft"), "draft\n").unwrap();
    }
    let mut command = fixture.command(&fixture.primary);
    command
        .args(["worktree", "remove", text(&hotfix)])
        .args(args);
    let (code, transcript) = terminal_session(command, ready, answer, || before_answer(&hotfix));
    (fixture, hotfix, code, transcript)
}

#[test]
fn remove_confirmation_defaults_to_no_and_declining_is_a_skip() {
    let (fixture, hotfix, code, transcript) = confirm(&[], "Remove this worktree?", b"\r", |_| {});

    assert_eq!(code, Some(0), "{transcript}");
    assert!(
        transcript.starts_with(&summary(&hotfix, "hotfix · kept", "none")),
        "{transcript}"
    );
    assert!(
        transcript.ends_with(&format!("Skipped {}\n", hotfix.display())),
        "{transcript}"
    );
    assert!(!transcript.contains("Removed"), "{transcript}");
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix.clone()]);
    assert_eq!(tree(&hotfix), [".git", "README"]);
}

#[test]
fn remove_confirmation_accepts_yes_and_removes_the_confirmed_worktree() {
    let (fixture, hotfix, code, transcript) = confirm(&[], "Remove this worktree?", b"y", |_| {});

    assert_eq!(code, Some(0), "{transcript}");
    assert!(
        transcript.ends_with(&format!(
            "Removed worktree {} · branch hotfix kept\n",
            hotfix.display()
        )),
        "{transcript}"
    );
    assert!(!hotfix.exists());
    fixture.assert_only_primary();
    assert_eq!(fixture.branches().len(), 2);
}

#[test]
fn remove_confirmation_names_the_losses_force_accepts() {
    let (fixture, hotfix, code, transcript) = confirm(
        &["--force"],
        "Accept these losses and remove this worktree?",
        b"n",
        |_| {},
    );

    assert_eq!(code, Some(0), "{transcript}");
    assert!(
        transcript.starts_with(&summary(&hotfix, "hotfix · kept", "1 untracked file")),
        "{transcript}"
    );
    assert!(
        transcript.ends_with(&format!("Skipped {}\n", hotfix.display())),
        "{transcript}"
    );
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix.clone()]);
    assert_eq!(tree(&hotfix), [".git", "README", "draft"]);
}

#[test]
fn remove_on_a_terminal_refuses_losses_without_force_before_any_prompt() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    fs::write(hotfix.join("draft"), "draft\n").unwrap();
    let mut command = fixture.command(&fixture.primary);
    command.args(["worktree", "remove", text(&hotfix)]);

    let (code, transcript) = terminal_session(command, "", b"", || {});

    assert_eq!(code, Some(1), "{transcript}");
    assert_eq!(
        transcript,
        format!(
            "{}{LOSSES_REFUSED}",
            summary(&hotfix, "hotfix · kept", "1 untracked file")
        )
    );
    assert_eq!(tree(&hotfix), [".git", "README", "draft"]);
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix.clone()]);
}

/// The service is what the inventory popup calls: it refuses on its own, whatever the caller
/// checked before.
#[test]
fn the_removal_service_refuses_unaccepted_losses_blockers_and_outdated_inspections() {
    use lager::application::ports::WorktreeLosses;
    use lager::application::worktrees::{RemoveError, inspect_removal, remove};
    use lager::infrastructure::{filesystem::Filesystem, git::NativeGit};
    // The service runs Git in this process, so the process is one with the test environment.
    if std::env::var_os("LAGER_WORKTREE_SERVICE_TEST").is_none() {
        let scope = tempfile::tempdir().unwrap();
        let output = support::external(scope.path(), std::env::current_exe().unwrap())
            .args([
                "--exact",
                "the_removal_service_refuses_unaccepted_losses_blockers_and_outdated_inspections",
                "--nocapture",
            ])
            .env("LAGER_WORKTREE_SERVICE_TEST", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(stdout(&output).contains("1 passed"), "{output:?}");
        return;
    }
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    fs::write(hotfix.join("draft"), "draft\n").unwrap();
    let branches = fixture.branches();

    let inspection = inspect_removal(&NativeGit, &Filesystem, &hotfix, None, None);
    assert_eq!(inspection.path, hotfix);
    assert_eq!(
        inspection.primary.as_deref(),
        Some(fixture.primary.as_path())
    );
    assert_eq!(inspection.branch.as_deref(), Some("refs/heads/hotfix"));
    assert_eq!(inspection.blockers, []);
    assert_eq!(
        inspection.losses,
        WorktreeLosses {
            modified: 0,
            untracked: 1,
            ignored: false
        }
    );
    assert!(inspection.force_helps());
    assert_eq!(
        remove(&NativeGit, &Filesystem, &inspection, false, None),
        Err(RemoveError::Losses)
    );

    git(&fixture.primary, &["worktree", "lock", text(&hotfix)]);
    let locked = inspect_removal(&NativeGit, &Filesystem, &hotfix, None, None);
    assert_eq!(locked.blockers.len(), 1);
    assert!(!locked.force_helps());
    assert_eq!(
        remove(&NativeGit, &Filesystem, &locked, true, None),
        Err(RemoveError::Blocked(locked.blockers.clone()))
    );
    assert_eq!(
        remove(&NativeGit, &Filesystem, &inspection, true, None),
        Err(RemoveError::Stale)
    );
    assert_eq!(tree(&hotfix), [".git", "README", "draft"]);

    git(&fixture.primary, &["worktree", "unlock", text(&hotfix)]);
    assert_eq!(
        remove(&NativeGit, &Filesystem, &inspection, true, None),
        Ok(())
    );
    assert!(!hotfix.exists());
    fixture.assert_only_primary();
    assert_eq!(fixture.branches(), branches);
}

/// The removal service runs Git in this process, so it needs a process with the test
/// environment: the first call runs `test` again in one and returns `false`.
fn in_service_process(test: &str) -> bool {
    if std::env::var_os("LAGER_WORKTREE_SERVICE_TEST").is_some() {
        return true;
    }
    let scope = tempfile::tempdir().unwrap();
    let output = support::external(scope.path(), std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("LAGER_WORKTREE_SERVICE_TEST", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("1 passed"), "{output:?}");
    false
}

#[test]
fn the_removal_service_checks_submodule_directories_and_autostashes_again_before_git_runs() {
    use lager::application::worktrees::{RemoveError, inspect_removal, remove};
    use lager::infrastructure::{filesystem::Filesystem, git::NativeGit};
    if !in_service_process(
        "the_removal_service_checks_submodule_directories_and_autostashes_again_before_git_runs",
    ) {
        return;
    }
    let fixture = with_submodule();
    let docs = fixture.linked("docs", &["-b", "docs"]);
    let confirmed = inspect_removal(&NativeGit, &Filesystem, &docs, None, None);
    assert_eq!(confirmed.blockers, []);
    assert!(confirmed.losses.is_empty());
    let notes = docs.join("vendor/theme/notes.txt");
    let autostash = fixture.primary.join(".git/worktrees/docs/MERGE_AUTOSTASH");

    for late in [&notes, &autostash] {
        // Written after the inspection that was confirmed.
        fs::write(late, "late\n").unwrap();
        let before = (fixture.state(), tree(&docs));
        assert_eq!(
            remove(&NativeGit, &Filesystem, &confirmed, true, None),
            Err(RemoveError::Stale)
        );
        let current = inspect_removal(&NativeGit, &Filesystem, &docs, None, None);
        assert_eq!(current.blockers.len(), 1, "{:?}", current.blockers);
        assert!(!current.force_helps());
        assert_eq!(
            remove(&NativeGit, &Filesystem, &current, true, None),
            Err(RemoveError::Blocked(current.blockers.clone()))
        );
        assert_eq!((fixture.state(), tree(&docs)), before);
        assert_eq!(fs::read_to_string(late).unwrap(), "late\n");
        fs::remove_file(late).unwrap();
    }

    assert_eq!(
        remove(&NativeGit, &Filesystem, &confirmed, false, None),
        Ok(())
    );
    assert!(!docs.exists());
    fixture.assert_only_primary();
}

#[test]
fn remove_confirmation_cancelled_with_ctrl_c_exits_130_and_removes_nothing() {
    let (fixture, hotfix, code, transcript) =
        confirm(&[], "Remove this worktree?", b"\x03", |_| {});

    assert_eq!(code, Some(130), "{transcript}");
    assert!(
        !transcript.contains("Removed") && !transcript.contains("Skipped"),
        "{transcript}"
    );
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix.clone()]);
    assert_eq!(tree(&hotfix), [".git", "README"]);
}

#[test]
fn remove_refuses_a_worktree_that_changed_after_the_confirmation_was_shown() {
    let (fixture, hotfix, code, transcript) =
        confirm(&[], "Remove this worktree?", b"y", |worktree| {
            fs::write(worktree.join("late"), "written after the summary\n").unwrap();
        });

    assert_eq!(code, Some(1), "{transcript}");
    assert!(
        transcript
            .ends_with("error: the worktree changed after it was inspected; nothing removed\n"),
        "{transcript}"
    );
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix.clone()]);
    assert_eq!(tree(&hotfix), [".git", "README", "late"]);
}

#[test]
fn remove_refuses_a_worktree_whose_head_moved_after_the_confirmation_was_shown() {
    let (fixture, hotfix, code, transcript) =
        confirm(&[], "Remove this worktree?", b"y", |worktree| {
            git(worktree, &["checkout", "-q", "--detach"]);
        });

    assert_eq!(code, Some(1), "{transcript}");
    assert!(
        transcript
            .ends_with("error: the worktree changed after it was inspected; nothing removed\n"),
        "{transcript}"
    );
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix.clone()]);
    assert_eq!(tree(&hotfix), [".git", "README"]);
}

#[test]
fn remove_refuses_another_worktree_put_at_the_confirmed_path() {
    let (fixture, hotfix, code, transcript) =
        confirm(&[], "Remove this worktree?", b"y", |worktree| {
            // Same path, branch, commit and cleanliness; a different worktree.
            let primary = worktree.parent().unwrap().parent().unwrap().join("core");
            let aside = worktree.with_file_name("aside");
            git(
                &primary,
                &["worktree", "move", text(worktree), text(&aside)],
            );
            git(&aside, &["checkout", "-q", "--detach"]);
            git(
                &primary,
                &["worktree", "add", "-q", text(worktree), "hotfix"],
            );
        });

    assert_eq!(code, Some(1), "{transcript}");
    assert!(
        transcript
            .ends_with("error: the worktree changed after it was inspected; nothing removed\n"),
        "{transcript}"
    );
    assert_eq!(
        fixture.listed(),
        [
            fixture.primary.clone(),
            hotfix.with_file_name("aside"),
            hotfix.clone()
        ]
    );
    assert_eq!(tree(&hotfix), [".git", "README"]);
    assert_eq!(git(&hotfix, &["symbolic-ref", "HEAD"]), "refs/heads/hotfix");
}

// ---- scope -------------------------------------------------------------------------------

#[test]
fn worktrees_are_not_declarations_and_new_ones_hold_only_tracked_files() {
    let fixture = Fixture::new();
    fs::write(fixture.primary.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
    git(&fixture.primary, &["add", ".gitignore"]);
    git(&fixture.primary, &["commit", "-qm", "ignore"]);
    fs::write(fixture.primary.join(".env"), "SECRET=1\n").unwrap();
    fs::create_dir(fixture.primary.join("node_modules")).unwrap();
    fs::write(fixture.primary.join("node_modules/dep"), "dep\n").unwrap();
    fs::write(fixture.primary.join("notes"), "untracked\n").unwrap();
    let config = fs::read(&fixture.config).unwrap();
    let hotfix = fixture.container().join("hotfix");

    assert_exit(&fixture.worktree(&["add", ".", "hotfix"]), 0);
    assert_eq!(tree(&hotfix), [".git", ".gitignore", "README"]);
    assert_eq!(fs::read(&fixture.config).unwrap(), config);

    assert_exit(&fixture.worktree(&["remove", text(&hotfix), "--yes"]), 0);
    let ensured = fixture.run(&fixture.base, &["ensure"]);
    assert_exit(&ensured, 0);
    assert!(!hotfix.exists());
    fixture.assert_only_primary();
    assert_eq!(fs::read(&fixture.config).unwrap(), config);
}

#[test]
fn mutations_need_explicit_targets_and_excluded_commands_do_not_exist() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let before = (fixture.state(), tree(&fixture.container()));
    let cases: &[&[&str]] = &[
        &["worktree", "add"],
        &["worktree", "add", "hotfix2"],
        &["worktree", "remove"],
        &["worktree", "remove", "--yes"],
        &["worktree", "prune"],
        &["worktree", "repair"],
        &["worktree", "move", text(&hotfix), "elsewhere"],
        &["worktree", "lock", text(&hotfix)],
        &[
            "worktree",
            "remove",
            text(&hotfix),
            "--yes",
            "--delete-branch",
        ],
        &["wt", "list"],
    ];
    for args in cases {
        let output = fixture.run(&fixture.primary, args);
        assert_exit(&output, 2);
        assert_eq!(stdout(&output), "", "{args:?}");
        assert_eq!(
            (fixture.state(), tree(&fixture.container())),
            before,
            "{args:?}"
        );
    }
}

// ---- configuration -----------------------------------------------------------------------

#[test]
fn worktree_root_rejects_absolute_parent_empty_and_non_string_values() {
    for value in [
        "\"/abs/trees\"",
        "\"../trees\"",
        "\"~/../trees\"",
        "\"a/../b\"",
        "\"\"",
        "3",
    ] {
        let fixture = Fixture::new();
        fs::write(
            &fixture.config,
            format!("root = \"repos\"\nworktree_root = {value}\n"),
        )
        .unwrap();
        let before = (fixture.state(), tree(&fixture.home.join("repos/acme")));

        for args in [
            &["worktree", "add", ".", "hotfix"][..],
            &["worktree", "list"],
            &["status"],
        ] {
            let output = fixture.run(&fixture.primary, args);
            assert_exit(&output, 1);
            assert_eq!(stdout(&output), "", "{value}");
            assert!(
                stderr(&output).contains("worktree_root must be"),
                "{value}: {output:?}"
            );
            assert!(!stderr(&output).contains("trees"), "{value}: {output:?}");
        }
        assert_eq!(
            (fixture.state(), tree(&fixture.home.join("repos/acme"))),
            before
        );
    }
}

#[test]
fn usage_errors_are_reported_before_the_configuration_is_read() {
    let fixture = Fixture::new();
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    fs::write(&fixture.config, "root = [\n").unwrap();

    let start = fixture.worktree(&["add", ".", "x", "--branch", "main", "--start", "main"]);
    assert_exit(&start, 2);
    assert_eq!(
        stderr(&start),
        "error: --start applies only to a new branch\n"
    );
    let unconfirmed = fixture.worktree(&["remove", text(&hotfix)]);
    assert_exit(&unconfirmed, 2);
    let valid = fixture.worktree(&["add", ".", "x"]);
    assert_exit(&valid, 1);
    assert!(
        stderr(&valid).contains("could not parse config"),
        "{valid:?}"
    );
    assert_eq!(fixture.listed(), [fixture.primary.clone(), hotfix]);
}

#[test]
fn worktree_root_stays_portable_through_a_config_mutation_behind_a_symlink() {
    let fixture = Fixture::new();
    let real = fixture.base.join("dotfiles/config.toml");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "root = \"repos\"\nworktree_root = \"~/trees\"\n").unwrap();
    fs::remove_file(&fixture.config).unwrap();
    std::os::unix::fs::symlink(&real, &fixture.config).unwrap();

    let registered = fixture.run(&fixture.base, &["register", "github.com/acme/other"]);
    assert_exit(&registered, 0);
    assert_eq!(stderr(&registered), "");
    let added = fixture.worktree(&["add", ".", "hotfix"]);
    assert_exit(&added, 0);

    assert!(
        fs::symlink_metadata(&fixture.config)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let written = fs::read_to_string(&real).unwrap();
    assert!(
        written.starts_with("root = \"repos\"\nworktree_root = \"~/trees\"\n"),
        "{written}"
    );
    assert!(written.contains("github.com/acme/other"), "{written}");
    assert!(!written.contains(text(&fixture.home)), "{written}");
    assert_eq!(
        tree(&fixture.home.join("trees/acme/core.worktrees/hotfix")),
        [".git", "README"]
    );
}

#[test]
fn worktree_commands_run_without_a_configuration_file() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.config).unwrap();

    let added = fixture.worktree(&["add", ".", "hotfix"]);
    assert_exit(&added, 0);
    let listed = fixture.worktree(&["list"]);
    assert_exit(&listed, 0);
    let hotfix = fixture.container().join("hotfix");
    assert_eq!(
        stdout(&listed),
        table(&[
            [text(&fixture.primary), "main", "primary"],
            [text(&hotfix), "hotfix", ""]
        ])
    );
    let reference = fixture.worktree(&["list", "acme/core"]);
    assert_exit(&reference, 1);
    assert_eq!(
        stderr(&reference),
        "error: a repository reference needs a configuration; pass a checkout path\n"
    );
    assert!(!fixture.config.exists());
}

// ---- sessions ----------------------------------------------------------------------------

const SESSION: &str = r#"
[session]
host = "rex"

[session.layouts.default]
[session.layouts.default.tabs.Agents]
root = { pane = "Agent I" }
splits = [{ source = "Agent I", pane = "Agent II", direction = "right" }]
"#;

fn expected_create(label: &str, cwd: &Path) -> Value {
    let block = |pane: &str| {
        json!({ "block": {
            "flavor": "com.superlogical.terminal.shell",
            "options": { "cwd": text(cwd) },
            "label": pane,
        }})
    };
    json!({
        "label": label,
        "initial_windows": [{
            "window_label": "Agents",
            "layout": { "split": {
                "direction": "horizontal",
                "ratio": 0.5,
                "before": block("Agent I"),
                "after": block("Agent II"),
            }},
        }],
        "focus": null,
    })
}

fn destroy(id: &str) -> String {
    format!("api call session.destroy {{\"session_id\":\"{id}\"}}")
}

fn tabs(n: usize) -> Value {
    json!([{ "name": "Agents", "panes": {
        "Agent I": format!("block:{n}-1-1"),
        "Agent II": format!("block:{n}-1-2"),
    }}])
}

#[test]
fn session_flags_need_a_session_section_before_git_runs() {
    let fixture = Fixture::with_rex(false);
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);
    let before = (fixture.state(), tree(&fixture.container()));
    let message = format!(
        "error: sessions are not configured; add [session] to {}\n",
        fixture.config.display()
    );
    let runs: [&[&str]; 4] = [
        &["add", ".", "spike", "--open-session"],
        &["add", ".", "spike", "--create-session"],
        &["add", ".", "spike", "--create-session", "--json"],
        &["remove", text(&hotfix), "--yes", "--close-session"],
    ];

    for args in runs {
        let output = fixture.worktree(args);
        assert_exit(&output, 1);
        assert_eq!(stdout(&output), "", "{args:?}");
        assert_eq!(stderr(&output), message, "{args:?}");
        assert_eq!((fixture.state(), tree(&fixture.container())), before);
    }
    fs::remove_file(&fixture.config).unwrap();
    let output = fixture.worktree(&["add", ".", "spike", "--create-session"]);
    assert_exit(&output, 1);
    assert_eq!(stderr(&output), message);
    assert_eq!(fixture.state(), before.0);

    let both = fixture.worktree(&["add", ".", "spike", "--open-session", "--create-session"]);
    assert_exit(&both, 2);
    assert!(stderr(&both).contains("cannot be used with"), "{both:?}");
    assert_eq!(fixture.calls(), Vec::<String>::new());
}

#[test]
fn without_a_flag_the_worktree_commands_leave_sessions_alone() {
    let fixture = Fixture::with_rex(true);
    let head = fixture.head();
    let hotfix = fixture.container().join("hotfix");

    let added = fixture.worktree(&["add", ".", "hotfix"]);
    assert_exit(&added, 0);
    assert_eq!(
        stdout(&added),
        format!(
            "HEAD is now at {} initial\nCreated worktree {} on new branch hotfix\n",
            short(&head),
            hotfix.display()
        )
    );
    assert_eq!(stderr(&added), "Preparing worktree (new branch 'hotfix')\n");
    let removed = fixture.worktree(&["remove", text(&hotfix), "--yes"]);
    assert_exit(&removed, 0);
    assert_eq!(
        stdout(&removed),
        format!(
            "{}Removed worktree {} · branch hotfix kept\n",
            summary(&hotfix, "hotfix · kept", "none"),
            hotfix.display()
        )
    );
    assert_eq!(stderr(&removed), "");
    assert_eq!(fixture.calls(), Vec::<String>::new());
}

#[test]
fn add_create_session_creates_the_worktree_then_its_session() {
    let fixture = Fixture::with_rex(true);
    let head = fixture.head();
    let hotfix = fixture.container().join("hotfix");

    let output = fixture.worktree(&["add", ".", "hotfix", "--create-session"]);

    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        format!(
            "HEAD is now at {} initial\nCreated worktree {} on new branch hotfix\nCreated session core · Hotfix\n",
            short(&head),
            hotfix.display()
        )
    );
    assert_eq!(
        stderr(&output),
        "Preparing worktree (new branch 'hotfix')\n"
    );
    assert_eq!(
        fixture.creates(),
        [expected_create("core · Hotfix", &hotfix)]
    );
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|call| call.contains("client.perform")),
        "{:?}",
        fixture.calls()
    );

    let again = fixture.run(&fixture.base, &["session", "create", "acme/core", "hotfix"]);
    assert_eq!(stdout(&again), "Session core · Hotfix already exists\n");
    assert_eq!(fixture.creates().len(), 1);
}

#[test]
fn add_open_session_creates_the_session_and_shows_it() {
    let fixture = Fixture::with_rex(true);

    let output = fixture.worktree(&["add", ".", "hotfix", "--open-session"]);

    assert_exit(&output, 0);
    assert!(
        stdout(&output).ends_with(&format!(
            "Created worktree {} on new branch hotfix\nOpened session core · Hotfix\n",
            fixture.container().join("hotfix").display()
        )),
        "{output:?}"
    );
    assert_eq!(fixture.creates().len(), 1);
    assert_eq!(
        fixture.calls().last().unwrap(),
        "api call client.perform {\"action\":\"session.select\",\"args\":{\"session_id\":\"session:1\"},\"client_id\":\"client:app\"}"
    );
}

#[test]
fn add_json_prints_one_document_and_sends_git_output_to_stderr() {
    let fixture = Fixture::with_rex(true);
    let head = fixture.head();
    git(&fixture.primary, &["branch", "review"]);

    let plain = fixture.worktree(&["add", ".", "hotfix", "--json"]);
    assert_exit(&plain, 0);
    let raw = stdout(&plain);
    assert!(raw.starts_with("{\n") && raw.ends_with("}\n"), "{raw}");
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap(),
        json!({
            "schema_version": 1,
            "path": text(&fixture.container().join("hotfix")),
            "branch": "hotfix",
            "new_branch": true,
            "session": null,
        })
    );
    assert_eq!(
        stderr(&plain),
        format!(
            "Preparing worktree (new branch 'hotfix')\nHEAD is now at {} initial\n",
            short(&head)
        )
    );
    assert_eq!(fixture.calls(), Vec::<String>::new());

    for (name, flag, created, id) in [
        ("review", "--create-session", true, 1),
        ("spike", "--open-session", true, 2),
    ] {
        let mut args = vec!["add", ".", name, flag, "--json"];
        if name == "review" {
            args.extend(["--branch", "review"]);
        }
        let output = fixture.worktree(&args);
        assert_exit(&output, 0);
        let label = format!("core · {}{}", name[..1].to_uppercase(), &name[1..]);
        assert_eq!(
            serde_json::from_str::<Value>(&stdout(&output)).unwrap(),
            json!({
                "schema_version": 1,
                "path": text(&fixture.container().join(name)),
                "branch": name,
                "new_branch": name != "review",
                "session": {
                    "created": created,
                    "id": format!("session:{id}"),
                    "label": label,
                    "tabs": tabs(id),
                },
            }),
            "{output:?}"
        );
    }
}

#[test]
fn add_reports_a_session_that_a_plain_git_removal_left_running() {
    let fixture = Fixture::with_rex(true);
    let hotfix = fixture.container().join("hotfix");
    assert_exit(
        &fixture.worktree(&["add", ".", "hotfix", "--create-session"]),
        0,
    );
    git(&fixture.primary, &["worktree", "remove", text(&hotfix)]);

    let output = fixture.worktree(&[
        "add",
        ".",
        "hotfix",
        "--branch",
        "hotfix",
        "--create-session",
    ]);

    assert_exit(&output, 0);
    assert!(
        stdout(&output).ends_with(&format!(
            "Created worktree {} on branch hotfix\nSession core · Hotfix already exists\n",
            hotfix.display()
        )),
        "{output:?}"
    );
    assert_eq!(fixture.creates().len(), 1);
}

#[test]
fn a_failed_session_keeps_the_new_worktree_and_exits_one() {
    let fixture = Fixture::with_rex(true);
    fixture.mode("offline");
    let reason = format!(
        "session host unavailable: no Rex server is running at {}/server.sock, and autostart is disabled",
        fixture.rex_state().display()
    );
    let hotfix = fixture.container().join("hotfix");

    let output = fixture.worktree(&["add", ".", "hotfix", "--create-session"]);

    assert_exit(&output, 1);
    assert_eq!(
        stdout(&output),
        format!(
            "HEAD is now at {} initial\nCreated worktree {} on new branch hotfix\n",
            short(&fixture.head()),
            hotfix.display()
        )
    );
    assert_eq!(
        stderr(&output),
        format!(
            "Preparing worktree (new branch 'hotfix')\nerror: worktree created, session not opened: {reason}\n"
        )
    );
    assert!(fixture.listed().contains(&hotfix));

    let json = fixture.worktree(&["add", ".", "spike", "--open-session", "--json"]);
    assert_exit(&json, 1);
    assert_eq!(stdout(&json), "");
    assert!(
        stderr(&json).ends_with(&format!(
            "error: worktree created, session not opened: {reason}\n"
        )),
        "{json:?}"
    );
    assert!(
        fixture
            .listed()
            .contains(&fixture.container().join("spike"))
    );
}

/// A `git` in the fake rex's directory that logs each `worktree remove` with its exit status
/// to the rex log, and fails it while `<state>/git-fails` exists.
fn logging_git(fixture: &Fixture) {
    let state = fixture.rex_state();
    let bin = &fixture.rex.as_ref().unwrap().0;
    let script = format!(
        "#!/bin/sh\ncase \" $* \" in\n  *\" worktree remove \"*)\n    if [ -e '{fails}' ]; then status=3; echo boom >&2; else '{git}' \"$@\"; status=$?; fi\n    echo \"git worktree remove exit $status\" >> '{log}'\n    exit $status;;\nesac\nexec '{git}' \"$@\"\n",
        fails = state.join("git-fails").display(),
        log = state.join("args.log").display(),
        git = support::real_tool("git").display()
    );
    fs::write(bin.join("git"), script).unwrap();
    fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn remove_close_session_destroys_only_after_a_successful_removal() {
    let fixture = Fixture::with_rex(true);
    let hotfix = fixture.container().join("hotfix");
    assert_exit(
        &fixture.worktree(&["add", ".", "hotfix", "--create-session"]),
        0,
    );
    logging_git(&fixture);
    let log = || fs::read_to_string(fixture.rex_state().join("args.log")).unwrap();
    let close = ["remove", text(&hotfix), "--yes", "--close-session"];

    fs::write(hotfix.join("draft"), "draft\n").unwrap();
    let refused = fixture.worktree(&close);
    assert_exit(&refused, 1);
    assert_eq!(stderr(&refused), LOSSES_REFUSED);
    fs::remove_file(hotfix.join("draft")).unwrap();

    git(&fixture.primary, &["worktree", "lock", text(&hotfix)]);
    let blocked = fixture.worktree(&close);
    assert_exit(&blocked, 1);
    git(&fixture.primary, &["worktree", "unlock", text(&hotfix)]);

    fixture.mode("git-fails");
    let failed = fixture.worktree(&close);
    assert_exit(&failed, 1);
    assert!(
        stderr(&failed).contains("git worktree remove failed"),
        "{failed:?}"
    );
    assert!(!log().contains("session.destroy"), "{}", log());
    fs::remove_file(fixture.rex_state().join("git-fails")).unwrap();

    let output = fixture.worktree(&close);

    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        format!(
            "{}Removed worktree {} · branch hotfix kept\nClosed session core · Hotfix\n",
            summary(&hotfix, "hotfix · kept", "none"),
            hotfix.display()
        )
    );
    assert_eq!(stderr(&output), "");
    assert!(!hotfix.exists());
    let tail: Vec<_> = log().lines().rev().take(3).map(str::to_owned).collect();
    assert_eq!(
        tail,
        [
            format!("--autostart=false --timeout 8s {}", destroy("session:1")),
            "--autostart=false --timeout 8s api call session.list".to_owned(),
            "git worktree remove exit 0".to_owned(),
        ]
    );
    assert_eq!(
        log().matches("git worktree remove exit").count(),
        2,
        "the refused and blocked removals never reached Git: {}",
        log()
    );
    assert_eq!(
        stdout(&fixture.run(&fixture.base, &["session", "list"])),
        ""
    );
}

#[test]
fn remove_close_session_reports_a_failed_close_and_a_missing_session() {
    let fixture = Fixture::with_rex(true);
    let hotfix = fixture.container().join("hotfix");
    let plain = fixture.linked("plain", &["-b", "plain"]);
    assert_exit(
        &fixture.worktree(&["add", ".", "hotfix", "--create-session"]),
        0,
    );

    let none = fixture.worktree(&["remove", text(&plain), "--yes", "--close-session"]);
    assert_exit(&none, 0);
    assert!(
        stdout(&none).ends_with(&format!(
            "Removed worktree {0} · branch plain kept\nNo session for {0}\n",
            plain.display()
        )),
        "{none:?}"
    );

    fixture.mode("destroy-fails");
    let output = fixture.worktree(&["remove", text(&hotfix), "--yes", "--close-session"]);

    assert_exit(&output, 1);
    assert!(
        stdout(&output).ends_with(&format!(
            "Removed worktree {} · branch hotfix kept\n",
            hotfix.display()
        )),
        "{output:?}"
    );
    assert_eq!(
        stderr(&output),
        "error: worktree removed, session not closed: session host failed: stream response \"control-1\" failed: internal error (status=500 code=internal)\n"
    );
    assert!(!hotfix.exists());
    let listed = fixture.run(&fixture.base, &["session", "list"]);
    assert!(stdout(&listed).starts_with("core · Hotfix  "), "{listed:?}");
}

#[test]
fn a_symlinked_checkout_path_maps_to_the_same_session() {
    let fixture = Fixture::with_rex(true);
    let link = fixture.base.join("link");
    std::os::unix::fs::symlink(&fixture.home, &link).unwrap();
    let hotfix = fixture.container().join("hotfix");

    let added = fixture.run(
        &fixture.base,
        &[
            "worktree",
            "add",
            text(&link.join("repos/acme/core")),
            "hotfix",
            "--create-session",
        ],
    );
    assert_exit(&added, 0);
    assert!(
        stdout(&added).ends_with("Created session core · Hotfix\n"),
        "{added:?}"
    );
    for args in [
        &["session", "create", "acme/core", "hotfix"][..],
        &[
            "session",
            "create",
            text(&link.join("repos/acme/core.worktrees/hotfix")),
        ],
    ] {
        let again = fixture.run(&fixture.base, args);
        assert_eq!(
            stdout(&again),
            "Session core · Hotfix already exists\n",
            "{args:?}"
        );
    }

    let removed = fixture.run(
        &fixture.base,
        &[
            "worktree",
            "remove",
            text(&link.join("repos/acme/core.worktrees/hotfix")),
            "--yes",
            "--close-session",
        ],
    );

    assert_exit(&removed, 0);
    assert!(
        stdout(&removed).ends_with("Closed session core · Hotfix\n"),
        "{removed:?}"
    );
    assert!(!hotfix.exists());
    assert_eq!(fixture.creates().len(), 1);
    assert_eq!(fixture.destroys(), [destroy("session:1")]);
}

#[test]
fn list_shows_each_worktrees_session_once_sessions_are_configured() {
    let fixture = Fixture::with_rex(true);
    let hotfix = fixture.container().join("hotfix");
    assert_exit(
        &fixture.worktree(&["add", ".", "hotfix", "--create-session"]),
        0,
    );
    let plain = fixture.linked("plain", &["-b", "plain"]);
    assert_exit(
        &fixture.run(&fixture.base, &["session", "create", "acme/core"]),
        0,
    );

    let output = fixture.worktree(&["list"]);

    assert_exit(&output, 0);
    assert_eq!(stderr(&output), "");
    assert_eq!(
        stdout(&output),
        table(&[
            [text(&fixture.primary), "main", "primary", "core"],
            [text(&hotfix), "hotfix", "", "core · Hotfix"],
            [text(&plain), "plain", "", ""],
        ])
    );
    let json = fixture.worktree(&["list", "--json"]);
    assert_exit(&json, 0);
    let document: Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(document["schema_version"], 2);
    let sessions: Vec<_> = document["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["session"].clone())
        .collect();
    assert_eq!(
        sessions,
        [
            json!({ "id": "session:2", "label": "core", "tabs": tabs(2) }),
            json!({ "id": "session:1", "label": "core · Hotfix", "tabs": tabs(1) }),
            Value::Null,
        ]
    );
    let raw = stdout(&json);
    let first = &raw[..raw.find("},").unwrap()];
    assert!(
        first.find("prune_reason").unwrap() < first.find("\"session\"").unwrap(),
        "{first}"
    );

    fixture.mode("offline");
    let warning = format!(
        "lager: warning: session host unavailable: no Rex server is running at {}/server.sock, and autostart is disabled\n",
        fixture.rex_state().display()
    );
    let offline = fixture.worktree(&["list"]);
    assert_exit(&offline, 0);
    assert_eq!(stderr(&offline), warning);
    assert_eq!(
        stdout(&offline),
        table(&[
            [text(&fixture.primary), "main", "primary", "unknown"],
            [text(&hotfix), "hotfix", "", "unknown"],
            [text(&plain), "plain", "", "unknown"],
        ])
    );
    let offline = fixture.worktree(&["list", "--json"]);
    assert_exit(&offline, 0);
    assert_eq!(stderr(&offline), warning);
    let document: Value = serde_json::from_str(&stdout(&offline)).unwrap();
    assert_eq!(document["schema_version"], 2);
    assert!(
        document["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["session"] == "unknown"),
        "{document}"
    );
}

#[test]
fn list_without_a_session_section_keeps_three_columns_and_schema_one() {
    let fixture = Fixture::with_rex(false);
    let hotfix = fixture.linked("hotfix", &["-b", "hotfix"]);

    let output = fixture.worktree(&["list"]);
    assert_exit(&output, 0);
    assert_eq!(
        stdout(&output),
        table(&[
            [text(&fixture.primary), "main", "primary"],
            [text(&hotfix), "hotfix", ""],
        ])
    );
    let json = fixture.worktree(&["list", "--json"]);
    let document: Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(document["schema_version"], 1);
    assert!(
        document["worktrees"][0].get("session").is_none(),
        "{document}"
    );
    assert_eq!(fixture.calls(), Vec::<String>::new());
}

#[test]
fn list_asks_the_host_once_and_warns_once_when_it_cannot_tell() {
    let fixture = Fixture::with_rex(true);
    fixture.linked("hotfix", &["-b", "hotfix"]);
    let lists = || {
        fixture
            .calls()
            .iter()
            .filter(|call| *call == "api call session.list")
            .count()
    };
    let unknown = |output: &Output, warning: &str| {
        assert_exit(output, 0);
        assert_eq!(stderr(output), warning);
        let rows = stdout(output);
        assert_eq!(rows.lines().count(), 2, "{rows}");
        assert!(rows.lines().all(|row| row.ends_with("  unknown")), "{rows}");
    };

    assert_exit(&fixture.worktree(&["list"]), 0);
    assert_eq!(lists(), 1);

    fixture.mode("bad-list");
    unknown(
        &fixture.worktree(&["list"]),
        "lager: warning: session host failed: rex listed no session ids\n",
    );
    assert_eq!(lists(), 2);
    fs::remove_file(fixture.rex_state().join("bad-list")).unwrap();

    let record = fixture.base.join("lager-test-cache/sessions.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, "not json").unwrap();
    let output = fixture.worktree(&["list"]);
    let warning = stderr(&output);
    assert!(
        warning.starts_with(&format!(
            "lager: warning: could not read sessions {}: ",
            record.display()
        )) && warning.lines().count() == 1,
        "{warning}"
    );
    unknown(&output, &warning);
}
