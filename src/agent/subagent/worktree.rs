//! A git worktree of a child's own, as Claude Code's `isolation: "worktree"`: made on a new branch
//! from the repository's `HEAD`, removed after the child if it changed nothing, else left for the
//! parent with its path and branch in the result.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::agent::sandbox::Sandbox;

pub(super) const ISOLATION: &str = "worktree";

pub(super) struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// The repository's top-level directory, whose `.git` holds the worktree's metadata.
    pub top: PathBuf,
    /// The repository's git directory (`--git-common-dir`), where removing it writes.
    git_dir: PathBuf,
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
    /// A worktree of the repository holding `cwd`, under the project `root`'s in `state`, and
    /// where `cwd` is in it.
    pub(super) fn add(root: &Path, cwd: &Path, state: &Path) -> Result<(Self, PathBuf), String> {
        let why = |error: String| format!("isolation \"worktree\" needs a git repository: {error}");
        let top = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"]).map_err(why)?);
        let top = crate::os::canonicalize(&top).unwrap_or(top);
        let base = git(&top, &["rev-parse", "--verify", "-q", "HEAD"])
            .map_err(|_| "isolation \"worktree\" needs a commit to start from".to_owned())?;
        let git_dir = top.join(git(&top, &["rev-parse", "--git-common-dir"])?);
        let git_dir = crate::os::canonicalize(&git_dir).unwrap_or(git_dir);
        let mut bytes = [0u8; 6];
        getrandom::getrandom(&mut bytes).map_err(|error| error.to_string())?;
        let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = format!("agent-{name}");
        let parent = directory(
            state,
            &crate::os::canonicalize(root).unwrap_or(root.to_owned()),
        );
        std::fs::create_dir_all(&parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
        let parent = crate::os::canonicalize(&parent).unwrap_or(parent);
        let path = parent.join(&name);
        let branch = format!("worktree-{name}");
        let shown = path.to_string_lossy();
        git(
            &top,
            &["worktree", "add", "-q", "-b", &branch, &shown, &base],
        )?;
        let cwd = crate::os::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
        let worktree = Self {
            path,
            branch,
            top,
            git_dir,
            base,
        };
        let inside = match cwd.strip_prefix(&worktree.top) {
            Ok(relative) => worktree.path.join(relative),
            Err(_) => worktree.path.clone(),
        };
        // What HEAD lacks (an untracked directory) is made; what a symlink in HEAD leads out of
        // the worktree is refused, as the child may write there whatever the main sandbox.
        let inside = std::fs::create_dir_all(&inside)
            .and_then(|()| crate::os::canonicalize(&inside))
            .ok()
            .filter(|real| real.starts_with(&worktree.path));
        let Some(inside) = inside else {
            worktree.finish(None);
            return Err(format!(
                "isolation \"worktree\": {} is not a directory in the worktree",
                cwd.display()
            ));
        };
        Ok((worktree, inside))
    }

    /// Whether a submodule's directory, empty in a new worktree and unseen by `git status`
    /// (and by `git worktree remove`), has files.
    fn submodule_written(&self) -> bool {
        let Ok(staged) = git(&self.path, &["ls-files", "--stage", "-z"]) else {
            return true;
        };
        (staged.split('\0'))
            .filter_map(|entry| entry.strip_prefix("160000 "))
            .filter_map(|entry| entry.split_once('\t'))
            .any(|(_, path)| {
                std::fs::read_dir(self.path.join(path))
                    .is_ok_and(|mut entries| entries.next().is_some())
            })
    }

    /// Removes the worktree and its branch if the child changed nothing; else what the parent
    /// is told of it, with what of the removal the `main` agent's sandbox in its project keeps
    /// read-only.
    pub(super) fn finish(&self, main: Option<(&Sandbox, &Path)>) -> Option<String> {
        let path = self.path.display();
        let changed = git(
            &self.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map(|status| !status.is_empty() || self.submodule_written());
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
        let mut note = format!(
            "Worktree kept with {what}: {path}, branch {} from {base}. Once its changes are \
merged or dropped: git worktree remove --force {path} && git branch -D {}",
            self.branch, self.branch
        );
        let read_only: Vec<String> = (main.into_iter())
            .flat_map(|(sandbox, root)| {
                [&self.path, &self.git_dir]
                    .into_iter()
                    .filter(move |place| sandbox.check_write(root, place).is_err())
            })
            .map(|place| place.display().to_string())
            .collect();
        if !read_only.is_empty() {
            note.push_str(&format!(
                "\nYour sandbox keeps {} read-only, so that command fails in it: ask the user to \
make them writable (request_permissions), or leave the removal to them.",
                read_only.join(" and ")
            ));
        }
        Some(note)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn commit(root: &Path) {
        let commit = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "a",
        ];
        git(root, &commit).unwrap();
    }

    fn repository(root: &Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "a\n").unwrap();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["add", "."]).unwrap();
        commit(root);
    }

    #[test]
    fn an_unchanged_worktree_goes_and_a_changed_one_stays_with_its_branch() {
        let base = std::env::temp_dir().join(format!("scramjet-worktree-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(base.clone());
        let (project, state) = (base.join("project"), base.join("state"));
        repository(&project);
        let (unchanged, cwd) = Worktree::add(&project, &project.join("src"), &state).unwrap();
        assert_eq!(cwd, unchanged.path.join("src"));
        assert!(cwd.join("a.txt").exists());
        assert!(unchanged.path.starts_with(state.join("worktrees")));
        assert_eq!(unchanged.finish(None), None);
        assert!(!unchanged.path.exists());
        let branches = git(&project, &["branch", "--list", &unchanged.branch]).unwrap();
        assert!(branches.is_empty(), "{branches}");

        let (changed, cwd) = Worktree::add(&project, &project, &state).unwrap();
        std::fs::write(cwd.join("new.txt"), "new\n").unwrap();
        let note = changed.finish(None).unwrap();
        assert!(
            note.starts_with("Worktree kept with uncommitted changes: "),
            "{note}"
        );
        assert!(note.contains(&changed.branch), "{note}");
        assert!(!note.contains("Your sandbox"), "{note}");
        // A main agent in workspace-write can write neither the worktree nor the project's .git;
        // granted both, it can remove the worktree itself.
        let mut sandbox = Sandbox {
            mode: crate::agent::sandbox::SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
        };
        let note = changed.finish(Some((&sandbox, &project))).unwrap();
        let git_dir = crate::os::canonicalize(&project.join(".git")).unwrap();
        let read_only = format!(
            "\nYour sandbox keeps {} and {} read-only, so that command fails in it",
            changed.path.display(),
            git_dir.display()
        );
        assert!(note.contains(&read_only), "{note}");
        sandbox.writable = vec![changed.path.clone(), git_dir];
        let note = changed.finish(Some((&sandbox, &project))).unwrap();
        assert!(!note.contains("Your sandbox"), "{note}");
        assert!(changed.path.join("new.txt").exists());
        assert!(!project.join("new.txt").exists());

        // A directory HEAD lacks is made; one a symlink in HEAD leads out of is refused.
        std::fs::create_dir_all(project.join("untracked")).unwrap();
        let (made, cwd) = Worktree::add(&project, &project.join("untracked"), &state).unwrap();
        assert_eq!(cwd, made.path.join("untracked"));
        assert!(cwd.is_dir());
        assert_eq!(made.finish(None), None);
        std::fs::remove_dir(project.join("untracked")).unwrap();
        std::os::unix::fs::symlink(&base, project.join("out")).unwrap();
        git(&project, &["add", "out"]).unwrap();
        commit(&project);
        std::fs::remove_file(project.join("out")).unwrap();
        std::fs::create_dir(project.join("out")).unwrap();
        let error = Worktree::add(&project, &project.join("out"), &state)
            .err()
            .unwrap();
        assert!(
            error.ends_with("is not a directory in the worktree"),
            "{error}"
        );

        // Files in a submodule's directory, which git status does not see, keep the worktree.
        let module = base.join("module");
        repository(&module);
        let url = module.to_string_lossy();
        let add = [
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &url,
            "m",
        ];
        git(&project, &add).unwrap();
        commit(&project);
        let (untouched, _) = Worktree::add(&project, &project, &state).unwrap();
        assert_eq!(untouched.finish(None), None);
        let (written, cwd) = Worktree::add(&project, &project, &state).unwrap();
        std::fs::write(cwd.join("m/new.txt"), "new\n").unwrap();
        let note = written.finish(None).unwrap();
        assert!(
            note.starts_with("Worktree kept with uncommitted changes"),
            "{note}"
        );
        assert!(cwd.join("m/new.txt").exists());

        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let error = Worktree::add(&outside, &outside, &state).err().unwrap();
        assert!(
            error.starts_with("isolation \"worktree\" needs a git repository"),
            "{error}"
        );
    }
}
