//! Port of pi's `/worktree`: list the checkouts of a repo, or pick one by branch,
//! directory name or number, creating `<main>-<branch>` when nothing matches.

use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("run git")?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Every checkout from `git worktree list --porcelain`, main one first.
pub fn list(dir: &Path) -> Result<Vec<Worktree>> {
    let porcelain = git(dir, &["worktree", "list", "--porcelain"])?;
    let mut worktrees = Vec::new();
    for block in porcelain.split("\n\n") {
        let Some(path) = block
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("worktree "))
        else {
            continue;
        };
        let branch = block
            .lines()
            .find_map(|line| line.strip_prefix("branch refs/heads/"))
            .unwrap_or("detached");
        worktrees.push(Worktree {
            path: path.into(),
            branch: branch.to_string(),
        });
    }
    Ok(worktrees)
}

/// The checkout for `name`: a list number, a branch, or a directory name.
/// Unknown names become a new worktree next to the main one, on an existing
/// branch of that name or a new branch from HEAD.
pub fn open(dir: &Path, name: &str) -> Result<PathBuf> {
    if name.starts_with('-') {
        bail!("worktree name must not start with '-'");
    }
    let worktrees = list(dir)?;
    if let Ok(number) = name.parse::<usize>() {
        return number
            .checked_sub(1)
            .and_then(|index| worktrees.into_iter().nth(index))
            .map(|worktree| worktree.path)
            .with_context(|| format!("no worktree number {number}"));
    }
    let existing = worktrees.iter().find(|worktree| {
        worktree.branch == name
            || worktree
                .path
                .file_name()
                .is_some_and(|dir_name| dir_name == name)
    });
    if let Some(worktree) = existing {
        return Ok(worktree.path.clone());
    }
    let main = &worktrees.first().context("git listed no worktrees")?.path;
    let path = PathBuf::from(format!("{}-{}", main.display(), name.replace('/', "-")));
    let path_arg = path.to_string_lossy();
    let branch_exists = git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok();
    if branch_exists {
        git(dir, &["worktree", "add", &path_arg, name])?;
    } else {
        git(dir, &["worktree", "add", "-b", name, &path_arg])?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_by_number_branch_or_creates_a_sibling_checkout() {
        let parent = tempfile::tempdir().unwrap();
        let main = parent.path().join("repo");
        std::fs::create_dir(&main).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            git(&main, args).unwrap();
        }
        let main = main.canonicalize().unwrap();

        let created = open(&main, "feat/x").unwrap();
        assert_eq!(created, PathBuf::from(format!("{}-feat-x", main.display())));
        assert_eq!(
            git(&created, &["branch", "--show-current"]).unwrap(),
            "feat/x"
        );

        assert_eq!(open(&main, "feat/x").unwrap(), created);
        assert_eq!(open(&main, "1").unwrap(), main);
        assert!(open(&main, "9").is_err());
        let branches: Vec<String> = list(&main).unwrap().into_iter().map(|w| w.branch).collect();
        assert_eq!(branches, ["main", "feat/x"]);
    }
}
