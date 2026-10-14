//! Port of pi's git-checkpoint: before each user message the worktree is committed
//! to a ref no branch reaches, so `/rewind` can also restore the code.
//! Ignored files are never captured or touched.

use anyhow::{Result, bail};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REF_ROOT: &str = "refs/pj-checkpoint";
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Stdout of a git call in `dir`, or `None` when git fails (no repo, no such ref).
fn git(dir: &Path, args: &[&str], scratch_index: bool) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    // Own index file: `add -A` picks up untracked files without touching the user's staging area.
    if scratch_index {
        command.env("GIT_INDEX_FILE", scratch_index_path(dir));
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn scratch_index_path(dir: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    dir.hash(&mut hasher);
    std::env::temp_dir().join(format!(
        "pj-checkpoint-index-{}-{:x}",
        std::process::id(),
        hasher.finish()
    ))
}

fn checkpoint_ref(session_id: &str, message_number: usize) -> String {
    format!("{REF_ROOT}/{session_id}/{message_number}")
}

/// A commit of the whole worktree, tracked and untracked, that no branch reaches.
pub fn snapshot(dir: &Path, label: &str) -> Option<String> {
    git(dir, &["add", "-A"], true)?;
    let tree = git(dir, &["write-tree"], true)?;
    // Never pushed, so a commit.gpgsign setup must not prompt for a key on every message.
    git(
        dir,
        &["commit-tree", &tree, "-m", label, "--no-gpg-sign"],
        false,
    )
}

/// Records the worktree as it is before user message `message_number` runs.
/// The message text is the commit message, so a checkpoint left by an earlier,
/// rewound conversation is never mistaken for this one.
pub fn save(dir: &Path, session_id: &str, message_number: usize, message: &str) -> Option<()> {
    let commit = snapshot(dir, message)?;
    git(
        dir,
        &[
            "update-ref",
            &checkpoint_ref(session_id, message_number),
            &commit,
        ],
        false,
    )?;
    Some(())
}

/// The checkpoint saved before user message `message_number` with this text.
pub fn find(dir: &Path, session_id: &str, message_number: usize, message: &str) -> Option<String> {
    let reference = checkpoint_ref(session_id, message_number);
    let commit = git(
        dir,
        &["rev-parse", "--verify", "--quiet", &reference],
        false,
    )?;
    let saved_message = git(dir, &["log", "-1", "--format=%B", &commit], false)?;
    (saved_message == message.trim()).then_some(commit)
}

/// Files that differ between two snapshots.
pub fn changed_files(dir: &Path, from: &str, to: &str) -> Vec<String> {
    git(dir, &["diff", "--name-only", from, to], false)
        .map(|names| names.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// Puts the worktree back to `commit`: files created since are removed first,
/// ignored paths (node_modules, .env) are never touched.
pub fn restore(dir: &Path, commit: &str) -> Result<()> {
    if git(dir, &["clean", "-fdq", "--", ":/"], false).is_none() {
        bail!("git clean failed in {}", dir.display());
    }
    if git(
        dir,
        &["restore", "--source", commit, "--worktree", "--", ":/"],
        false,
    )
    .is_none()
    {
        bail!("git restore --source {commit} failed in {}", dir.display());
    }
    Ok(())
}

/// Drops checkpoints older than a week so they do not keep objects alive forever.
pub fn prune(dir: &Path) {
    let Some(refs) = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname) %(committerdate:unix)",
            REF_ROOT,
        ],
        false,
    ) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    for line in refs.lines() {
        let Some((reference, committed)) = line.split_once(' ') else {
            continue;
        };
        let Ok(committed) = committed.parse::<u64>() else {
            continue;
        };
        if now.saturating_sub(Duration::from_secs(committed)) > KEEP_FOR {
            git(dir, &["update-ref", "-d", reference], false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
        ] {
            git(dir.path(), args, false).unwrap();
        }
        std::fs::write(dir.path().join(".gitignore"), "secret.env\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();
        dir
    }

    #[test]
    fn restores_edits_removes_new_files_and_keeps_ignored_ones() {
        let dir = repo();
        let path = dir.path();
        save(path, "s1", 3, "fix the bug").unwrap();

        std::fs::write(path.join("a.txt"), "after\n").unwrap();
        std::fs::write(path.join("new.txt"), "new\n").unwrap();
        std::fs::write(path.join("secret.env"), "KEY=1\n").unwrap();

        let then = find(path, "s1", 3, "fix the bug").unwrap();
        let now = snapshot(path, "now").unwrap();
        assert_eq!(changed_files(path, &then, &now), ["a.txt", "new.txt"]);

        restore(path, &then).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "before\n"
        );
        assert!(!path.join("new.txt").exists());
        assert!(path.join("secret.env").exists());
    }

    #[test]
    fn a_checkpoint_from_another_message_is_not_found() {
        let dir = repo();
        save(dir.path(), "s1", 3, "fix the bug").unwrap();
        assert_eq!(find(dir.path(), "s1", 3, "something else"), None);
        assert_eq!(find(dir.path(), "s1", 4, "fix the bug"), None);
    }

    #[test]
    fn outside_a_repo_nothing_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(save(dir.path(), "s1", 1, "hi"), None);
    }
}
