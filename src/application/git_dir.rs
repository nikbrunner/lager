//! What the top-level entries of a Git directory say about state a removal deletes.

/// Entries Git keeps in both a repository's Git directory and a linked worktree's.
/// `modules` has its own guard.
const SHARED: &[&str] = &[
    "HEAD",
    "index",
    "logs",
    "refs",
    "info",
    "config.worktree",
    "FETCH_HEAD",
    "ORIG_HEAD",
    "COMMIT_EDITMSG",
    "REBASE_HEAD",
    "AUTO_MERGE",
    "modules",
    "gc.log",
    "reftable",
    "fsmonitor--daemon",
    // Each holds only an id a coding agent keeps (its base commit, its session), no changes.
    "CLAUDE_BASE",
    "opencode",
];
const REPOSITORY: &[&str] = &[
    "objects",
    "hooks",
    "description",
    "config",
    "packed-refs",
    "shallow",
    "lfs",
];
const LINKED: &[&str] = &["commondir", "gitdir", "locked"];

/// The hint of a named state is the last ` (git …)`; this drops it.
pub fn without_hint(warning: &str) -> &str {
    warning
        .rsplit_once(" (git ")
        .filter(|(_, hint)| hint.ends_with(')'))
        .map_or(warning, |(state, _)| state)
}

/// Names the state in a Git directory that a removal cannot show to be safe: operations in
/// progress with the commands that end them, and every entry Git does not put there in
/// normal use. `entries` are the top-level names; `holds` answers for a path below the
/// directory whether it is there and not empty, and is true when it cannot tell. `place`
/// shows the directory and `git` is the command the hints name.
pub fn losses(
    entries: &[String],
    linked: bool,
    place: &str,
    git: &str,
    holds: impl Fn(&str) -> Option<bool>,
) -> Vec<String> {
    let has = |name: &str| entries.iter().any(|entry| entry == name);
    let operation = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ]
    .into_iter()
    .any(has);
    // A command run elsewhere (`git -C PATH`) names its path once.
    let ends = |command: &str| {
        let again = if git == "git" {
            format!("git {command} ")
        } else {
            String::new()
        };
        format!("({git} {command} --continue or {again}--abort)")
    };
    let rebase = format!("stopped rebase {}", ends("rebase"));
    let rebase_autostash = format!("autostash of a stopped rebase {}", ends("rebase"));
    let exists = |path: &str| holds(path).is_some();
    let mut named: Vec<String> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    let mut entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    entries.sort_unstable();
    for entry in entries {
        if SHARED.contains(&entry)
            || entry.ends_with(".ipc")
            || (if linked { LINKED } else { REPOSITORY }).contains(&entry)
        {
            continue;
        }
        let reasons: Vec<String> = match entry {
            "rebase-merge" if exists("rebase-merge/autostash") => {
                vec![rebase.clone(), rebase_autostash.clone()]
            }
            "rebase-merge" => vec![rebase.clone()],
            "rebase-apply" if exists("rebase-apply/applying") => {
                vec![format!("stopped git am {}", ends("am"))]
            }
            "rebase-apply" if exists("rebase-apply/autostash") => {
                vec![rebase.clone(), rebase_autostash.clone()]
            }
            "rebase-apply" => vec![rebase.clone()],
            "MERGE_HEAD" => vec![format!("merge in progress {}", ends("merge"))],
            "CHERRY_PICK_HEAD" => vec![format!("cherry-pick in progress {}", ends("cherry-pick"))],
            "REVERT_HEAD" => vec![format!("revert in progress {}", ends("revert"))],
            // Git writes these beside the state of an operation that is named itself.
            "MERGE_MSG" | "MERGE_MODE" if operation => vec![],
            "sequencer" => vec![format!(
                "unfinished sequence of picks or reverts {}, or the same with revert)",
                ends("cherry-pick").trim_end_matches(')')
            )],
            "MERGE_AUTOSTASH" => vec![format!("autostash of a stopped merge {}", ends("merge"))],
            "CHECKOUT_AUTOSTASH_HEAD" => vec![format!(
                "autostash of an interrupted checkout ({git} stash apply the commit that file names)"
            )],
            _ if entry.starts_with("BISECT_") => {
                vec![format!("bisect in progress ({git} bisect reset)")]
            }
            // An empty file or directory holds nothing a removal could lose.
            _ if holds(entry) == Some(false) => vec![],
            _ => {
                unknown.push(entry);
                vec![]
            }
        };
        for reason in reasons {
            if !named.contains(&reason) {
                named.push(reason);
            }
        }
    }
    if !unknown.is_empty() {
        named.push(format!(
            "unknown entries in {place}: {}",
            unknown.join(", ")
        ));
    }
    named
}

#[cfg(test)]
mod tests {
    use super::{losses, without_hint};

    fn names(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| (*entry).to_owned()).collect()
    }

    fn full(_: &str) -> Option<bool> {
        Some(true)
    }

    #[test]
    fn normal_entries_are_silent_and_the_rest_is_named_once() {
        let repository = names(&[
            "HEAD",
            "config",
            "objects",
            "refs",
            "hooks",
            "modules",
            "ORIG_HEAD",
            "gc.log",
            "fsmonitor--daemon.ipc",
        ]);
        assert!(losses(&repository, false, ".git", "git", full).is_empty());
        // A repository's own entries are not what Git puts in a linked Git directory.
        assert_eq!(
            losses(
                &names(&["HEAD", "gitdir", "commondir", "objects"]),
                true,
                "/r/.git/worktrees/w",
                "git -C /w",
                full
            ),
            ["unknown entries in /r/.git/worktrees/w: objects"]
        );
        assert_eq!(
            losses(
                &names(&[
                    "zeta",
                    "MERGE_MSG",
                    "MERGE_HEAD",
                    "BISECT_LOG",
                    "BISECT_START",
                    "alpha",
                    "empty"
                ]),
                false,
                ".git",
                "git",
                |path| Some(path != "empty")
            ),
            [
                "bisect in progress (git bisect reset)",
                "merge in progress (git merge --continue or git merge --abort)",
                "unknown entries in .git: alpha, zeta",
            ]
        );
        assert_eq!(
            losses(
                &names(&["MERGE_MSG", "rebase-merge"]),
                false,
                ".git",
                "git",
                |path| { (path != "rebase-merge/autostash").then_some(false) }
            ),
            ["stopped rebase (git rebase --continue or git rebase --abort)"]
        );
        assert_eq!(
            without_hint("stopped git am (git -C /w am --continue or --abort)"),
            "stopped git am"
        );
        assert_eq!(
            without_hint(
                "tracked files hidden from git status (skip-worktree or assume-unchanged)"
            ),
            "tracked files hidden from git status (skip-worktree or assume-unchanged)"
        );
    }
}
