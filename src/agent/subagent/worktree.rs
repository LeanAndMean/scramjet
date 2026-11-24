//! A git worktree of a child's own, as Claude Code's `isolation: "worktree"`: made on a new branch
//! from the repository's `HEAD`, removed after the child if it changed nothing, else left for the
//! parent with its path and branch in the result.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) const ISOLATION: &str = "worktree";

pub(super) struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// The repository's top-level directory, whose `.git` holds the worktree's metadata.
    pub top: PathBuf,
    base: String,
}

/// `STATE/worktrees/<project>`, holding a project's children's worktrees.
pub(in crate::agent) fn directory(state: &Path, top: &Path) -> PathBuf {
    state
        .join("worktrees")
        .join(super::super::session::slug(&top.to_string_lossy()))
}

fn git(directory: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git: {error}"))?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()),
        false => Err(format!(
            "git {}: {}",
            arguments.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

impl Worktree {
    /// A worktree of the repository holding `cwd`, under `state`'s, and where `cwd` is in it.
    pub(super) fn add(cwd: &Path, state: &Path) -> Result<(Self, PathBuf), String> {
        let why = |error: String| format!("isolation \"worktree\" needs a git repository: {error}");
        let top = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"]).map_err(why)?);
        let top = std::fs::canonicalize(&top).unwrap_or(top);
        let base = git(&top, &["rev-parse", "--verify", "-q", "HEAD"])
            .map_err(|_| "isolation \"worktree\" needs a commit to start from".to_owned())?;
        let mut bytes = [0u8; 6];
        getrandom::getrandom(&mut bytes).map_err(|error| error.to_string())?;
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = format!("agent-{name}");
        let parent = directory(state, &top);
        std::fs::create_dir_all(&parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
        let path = parent.join(&name);
        let branch = format!("worktree-{name}");
        let shown = path.to_string_lossy();
        git(
            &top,
            &["worktree", "add", "-q", "-b", &branch, &shown, &base],
        )?;
        let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
        let inside = match cwd.strip_prefix(&top) {
            Ok(relative) => path.join(relative),
            Err(_) => path.clone(),
        };
        Ok((
            Self {
                path,
                branch,
                top,
                base,
            },
            inside,
        ))
    }

    /// Removes the worktree and its branch if the child changed nothing; else what the parent
    /// is told of it.
    pub(super) fn finish(&self) -> Option<String> {
        let path = self.path.display();
        let changed = git(
            &self.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map(|status| !status.is_empty());
        let head = git(&self.path, &["rev-parse", "HEAD"]);
        let commits = match &head {
            Ok(head) if *head != self.base => git(
                &self.path,
                &["rev-list", "--count", &format!("{}..HEAD", self.base)],
            )
            .ok(),
            _ => None,
        };
        if let (Ok(false), Ok(_), None) = (&changed, &head, &commits) {
            let shown = self.path.to_string_lossy();
            let removed = git(&self.top, &["worktree", "remove", "--force", &shown])
                .and_then(|_| git(&self.top, &["branch", "-D", &self.branch]));
            return removed
                .err()
                .map(|error| format!("Its worktree {path} is unchanged but was left: {error}"));
        }
        let what = match (changed, commits) {
            (Ok(true), Some(commits)) => format!("{commits} commits and uncommitted changes"),
            (_, Some(commits)) => format!("{commits} commits"),
            (Ok(true), None) => "uncommitted changes".to_owned(),
            (_, None) => "changes that could not be read".to_owned(),
        };
        let base = &self.base[..self.base.len().min(12)];
        Some(format!(
            "Worktree kept with {what}: {path}, branch {} from {base}. Once its changes are \
merged or dropped: git worktree remove --force {path} && git branch -D {}",
            self.branch, self.branch
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn repository(root: &Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "a\n").unwrap();
        for arguments in [
            &["init", "-q"][..],
            &["add", "."],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "a",
            ],
        ] {
            git(root, arguments).unwrap();
        }
    }

    #[test]
    fn an_unchanged_worktree_goes_and_a_changed_one_stays_with_its_branch() {
        let base = std::env::temp_dir().join(format!("scramjet-worktree-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(base.clone());
        let (project, state) = (base.join("project"), base.join("state"));
        repository(&project);
        let (unchanged, cwd) = Worktree::add(&project.join("src"), &state).unwrap();
        assert_eq!(cwd, unchanged.path.join("src"));
        assert!(cwd.join("a.txt").exists());
        assert!(unchanged.path.starts_with(state.join("worktrees")));
        assert_eq!(unchanged.finish(), None);
        assert!(!unchanged.path.exists());
        let branches = git(&project, &["branch", "--list", &unchanged.branch]).unwrap();
        assert!(branches.is_empty(), "{branches}");

        let (changed, cwd) = Worktree::add(&project, &state).unwrap();
        std::fs::write(cwd.join("new.txt"), "new\n").unwrap();
        let note = changed.finish().unwrap();
        assert!(
            note.starts_with("Worktree kept with uncommitted changes: "),
            "{note}"
        );
        assert!(note.contains(&changed.branch), "{note}");
        assert!(changed.path.join("new.txt").exists());
        assert!(!project.join("new.txt").exists());

        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let error = Worktree::add(&outside, &state).err().unwrap();
        assert!(
            error.starts_with("isolation \"worktree\" needs a git repository"),
            "{error}"
        );
    }
}
