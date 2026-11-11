//! Packages from git hosting, as Scramjet installs them: `git:github.com/user/repo[@ref]`,
//! `git:git@github.com:user/repo` and `https://`, `ssh://`, `git://` or `file://` URLs, cloned
//! by the `git` program, with the user's SSH configuration and credential helpers, into
//! `$XDG_DATA_HOME/scramjet/git/<host>/<path>`. A ref pins a package: `update` leaves it.
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// A package's git source.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Source {
    /// As the user and the settings wrote it.
    pub written: String,
    /// What `git clone` is given.
    repository: String,
    host: String,
    /// The repository's path on its host without `.git`, each part checked, so that its clone
    /// stays inside the directory of clones.
    path: String,
    /// The branch, tag or commit it is pinned to.
    pub reference: Option<String>,
}

impl Source {
    /// `text` as a git source, or why it is not a valid one; `None` when it is not meant as
    /// one, as a directory is not: without `git:`, only a URL is.
    pub fn parse(text: &str) -> Option<Result<Self, String>> {
        let written = text.trim();
        let (prefixed, url) = match written.strip_prefix("git:") {
            Some(rest) if !rest.starts_with("//") => (true, rest.trim()),
            _ => (false, written),
        };
        if !prefixed && !url.contains("://") {
            return None;
        }
        Some(Self::read(written, url).map_err(|why| format!("{written}: {why}")))
    }

    fn read(written: &str, url: &str) -> Result<Self, String> {
        const FORMS: &str =
            "not a repository: git:HOST/USER/REPO, git:git@HOST:USER/REPO or a git URL";
        let (repository, host, path, reference) = if let Some((scheme, rest)) =
            url.split_once("://")
        {
            if !["https", "http", "ssh", "git", "file"].contains(&&*scheme.to_ascii_lowercase()) {
                return Err("git URLs start https://, http://, ssh://, git:// or file://".into());
            }
            let (authority, rest) = rest.split_once('/').unwrap_or((rest, ""));
            let (path, reference) = split_reference(rest)?;
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            let host = host.split(':').next().unwrap_or_default();
            let host = match host.is_empty() && scheme.eq_ignore_ascii_case("file") {
                true => "localhost",
                false => host,
            };
            (
                format!("{scheme}://{authority}/{path}"),
                host,
                path,
                reference,
            )
        } else if let Some((host, rest)) = url.strip_prefix("git@").and_then(|r| r.split_once(':'))
        {
            let (path, reference) = split_reference(rest)?;
            (format!("git@{host}:{path}"), host, path, reference)
        } else {
            let (host, rest) = url.split_once('/').ok_or(FORMS)?;
            if !host.contains('.') && host != "localhost" {
                return Err(FORMS.into());
            }
            let (path, reference) = split_reference(rest)?;
            (format!("https://{host}/{path}"), host, path, reference)
        };
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<&str> = path.split('/').collect();
        let wrong = |part: &str| part.is_empty() || part == "." || part == "..";
        if wrong(host) || host.contains('/') || parts.len() < 2 || parts.iter().any(|p| wrong(p)) {
            return Err(FORMS.into());
        }
        Ok(Self {
            written: written.to_owned(),
            repository,
            host: host.to_owned(),
            path: path.to_owned(),
            reference,
        })
    }

    /// `git:<host>/<path>`: the repository whatever its form and ref, as Scramjet tells
    /// packages apart.
    pub fn identity(&self) -> String {
        format!("git:{}/{}", self.host, self.path)
    }

    /// Its clone under `root`, the directory of clones.
    pub fn clone_directory(&self, root: &Path) -> PathBuf {
        root.join(&self.host).join(&self.path)
    }
}

/// `path` and the ref after its first `@`.
fn split_reference(path: &str) -> Result<(&str, Option<String>), String> {
    let Some((path, reference)) = path.split_once('@') else {
        return Ok((path, None));
    };
    if reference.is_empty() || reference.starts_with('-') {
        return Err(format!("{reference:?} is not a branch, tag or commit"));
    }
    Ok((path, Some(reference.to_owned())))
}

/// Where packages are cloned: `$XDG_DATA_HOME/scramjet/git`, by default
/// `~/.local/share/scramjet/git`.
pub(super) fn clones() -> Option<PathBuf> {
    let data = scramjet_rs::os::base_directory("XDG_DATA_HOME", ".local/share")?;
    Some(data.join("scramjet").join("git"))
}

/// Clones `source` into `directory`, which must not exist, at its ref. Nothing of it is left
/// when that fails.
pub(super) fn clone(source: &Source, directory: &Path, root: &Path) -> Result<(), String> {
    if directory.exists() {
        return Err(format!("{} exists already", directory.display()));
    }
    let parent = directory.parent().unwrap_or(directory);
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let mut command = git(None);
    command.args(["clone", "--quiet", "--", &source.repository]);
    let cloned = run(command.arg(directory), "clone").and_then(|_| match &source.reference {
        Some(reference) => checkout(directory, reference),
        None => Ok(()),
    });
    if cloned.is_err() {
        if directory.exists() {
            let _ = delete(directory, root);
        }
        prune(parent, root);
    }
    cloned
}

/// Checks out `reference` in the clone in `directory`.
pub(super) fn checkout(directory: &Path, reference: &str) -> Result<(), String> {
    let mut command = git(Some(directory));
    command.args(["-c", "advice.detachedHead=false", "checkout", "--quiet"]);
    run(command.args([reference, "--"]), "checkout").map(drop)
}

/// Whether `directory` is a clone of its own, not only inside one, so git is not run on the
/// repository around it.
pub(super) fn is_clone(directory: &Path) -> bool {
    directory.join(".git").exists()
}

/// Pulls the clone in `directory` when it can be fast-forwarded, keeping changes made in it.
/// Its commit before and after.
pub(super) fn pull(directory: &Path) -> Result<(String, String), String> {
    if !is_clone(directory) {
        return Err(format!(
            "{} is not a clone; left as it is",
            directory.display()
        ));
    }
    let head = || {
        run(
            git(Some(directory)).args(["rev-parse", "--short", "HEAD"]),
            "rev-parse",
        )
    };
    let before = head()?;
    run(
        git(Some(directory)).args(["pull", "--quiet", "--ff-only"]),
        "pull",
    )?;
    Ok((before, head()?))
}

/// The clone's newest `vX.Y.Z` tag that its checked-out commit holds, if any.
pub(super) fn newest_tag(directory: &Path) -> Result<Option<String>, String> {
    let mut command = git(Some(directory));
    command.args(["tag", "--merged", "HEAD", "--list", "v[0-9]*"]);
    let tags = run(command.arg("--sort=-version:refname"), "tag")?;
    Ok(tags.lines().next().map(str::to_owned))
}

/// Deletes the clone in `directory`, then the directories it leaves empty, up to `root`, the
/// directory of clones. Only a git clone inside `root` is deleted.
pub(super) fn delete(directory: &Path, root: &Path) -> Result<(), String> {
    let shown = directory.display();
    let inside = match (fs::canonicalize(root), fs::canonicalize(directory)) {
        (_, Err(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        (Ok(root), Ok(clone)) => clone.starts_with(&root) && clone != root,
        _ => false,
    };
    if !inside || !is_clone(directory) {
        return Err(format!(
            "{shown} is not a clone in {}; left as it is",
            root.display()
        ));
    }
    fs::remove_dir_all(directory).map_err(|error| format!("{shown}: {error}"))?;
    prune(directory.parent().unwrap_or(root), root);
    Ok(())
}

/// Removes `directory` and its parents while they are empty and inside `root`.
fn prune(directory: &Path, root: &Path) {
    let mut directory = directory;
    while directory.starts_with(root) && directory != root && fs::remove_dir(directory).is_ok() {
        directory = directory.parent().unwrap_or(root);
    }
}

/// `git`, run in `directory`, with the terminal for prompts and messages.
fn git(directory: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    if let Some(directory) = directory {
        command.arg("-C").arg(directory);
    }
    command.stdin(Stdio::inherit()).stderr(Stdio::inherit());
    command
}

/// What `command` printed, trimmed; `git <what>` failing when it does.
fn run(command: &mut Command, what: &str) -> Result<String, String> {
    match command.stdout(Stdio::piped()).output() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err("git is not installed; packages from git need it".into())
        }
        Err(error) => Err(format!("git: {error}")),
        Ok(output) if !output.status.success() => Err(format!("git {what} failed")),
        Ok(output) => Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn parsed(text: &str) -> (String, String, String, Option<String>) {
        let source = Source::parse(text).unwrap().unwrap();
        assert_eq!(source.written, text.trim());
        (
            source.repository,
            source.host,
            source.path,
            source.reference,
        )
    }

    #[test]
    fn sources_are_read_as_scramjet_reads_them() {
        let owned = |r: &str, h: &str, p: &str, at: Option<&str>| {
            (r.into(), h.into(), p.into(), at.map(str::to_owned))
        };
        assert_eq!(
            parsed("git:github.com/LeanAndMean/scramjet"),
            owned(
                "https://github.com/LeanAndMean/scramjet",
                "github.com",
                "LeanAndMean/scramjet",
                None
            )
        );
        assert_eq!(
            parsed(" git:github.com/user/repo@v1 "),
            owned(
                "https://github.com/user/repo",
                "github.com",
                "user/repo",
                Some("v1")
            )
        );
        assert_eq!(
            parsed("git:git@github.com:user/repo.git@main"),
            owned(
                "git@github.com:user/repo.git",
                "github.com",
                "user/repo",
                Some("main")
            )
        );
        assert_eq!(
            parsed("https://github.com/user/repo.git"),
            owned(
                "https://github.com/user/repo.git",
                "github.com",
                "user/repo",
                None
            )
        );
        assert_eq!(
            parsed("ssh://git@host.example:2222/group/sub/repo@abc123"),
            owned(
                "ssh://git@host.example:2222/group/sub/repo",
                "host.example",
                "group/sub/repo",
                Some("abc123")
            )
        );
        assert_eq!(
            parsed("git://host.example/user/repo"),
            owned(
                "git://host.example/user/repo",
                "host.example",
                "user/repo",
                None
            )
        );
        assert_eq!(
            parsed("file:///srv/git/repo.git"),
            owned(
                "file:///srv/git/repo.git",
                "localhost",
                "srv/git/repo",
                None
            )
        );
        assert_eq!(
            Source::parse("git:github.com/user/repo@v1")
                .unwrap()
                .unwrap()
                .identity(),
            "git:github.com/user/repo"
        );
        assert_eq!(
            Source::parse("git:git@github.com:user/repo")
                .unwrap()
                .unwrap()
                .clone_directory(Path::new("/clones")),
            Path::new("/clones/github.com/user/repo")
        );
    }

    #[test]
    fn only_clones_inside_the_directory_of_clones_are_deleted() {
        let scratch =
            std::env::temp_dir().join(format!("scramjet-packages-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        let _guard = Scratch(scratch.clone());
        let root = scratch.join("git");
        let made = |path: &str| {
            let path = scratch.join(path);
            fs::create_dir_all(&path).unwrap();
            path
        };
        let (clone, kept) = (
            made("git/host/user/repo/.git"),
            made("git/host/user/kept/.git"),
        );
        delete(clone.parent().unwrap(), &root).unwrap();
        assert!(!scratch.join("git/host/user/repo").exists());
        assert!(kept.exists());
        delete(kept.parent().unwrap(), &root).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        let outside = made("outside/.git");
        made("git/host/user");
        let plain = made("git/host/user/plain");
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut refused = vec![plain.clone(), root.clone(), scratch.join("outside")];
        #[cfg(unix)]
        {
            let link = root.join("host/user/link");
            std::os::unix::fs::symlink(outside.parent().unwrap(), &link).unwrap();
            refused.push(link);
        }
        for directory in &refused {
            assert_eq!(
                delete(directory, &root),
                Err(format!(
                    "{} is not a clone in {}; left as it is",
                    directory.display(),
                    root.display()
                ))
            );
        }
        assert!(outside.exists() && plain.exists());
        assert_eq!(delete(&root.join("host/user/missing"), &root), Ok(()));
    }

    #[test]
    fn directories_are_not_sources_and_wrong_ones_say_why() {
        for directory in [
            "~/repos/scramjet",
            "./sets",
            "/srv/git/repo.git",
            "github.com/u/r",
        ] {
            assert_eq!(Source::parse(directory), None, "{directory}");
        }
        let forms = "not a repository: git:HOST/USER/REPO, git:git@HOST:USER/REPO or a git URL";
        for (text, why) in [
            ("git:user/repo", forms),
            ("git:github.com/repo", forms),
            ("git:github.com/user/../../etc", forms),
            ("https://github.com/user/./repo", forms),
            ("https://../user/repo", forms),
            ("file:///repo", forms),
            (
                "ftp://host/user/repo",
                "git URLs start https://, http://, ssh://, git:// or file://",
            ),
            (
                "git:github.com/user/repo@",
                "\"\" is not a branch, tag or commit",
            ),
            (
                "git:github.com/user/repo@--upload-pack=x",
                "\"--upload-pack=x\" is not a branch, tag or commit",
            ),
        ] {
            assert_eq!(
                Source::parse(text),
                Some(Err(format!("{text}: {why}"))),
                "{text}"
            );
        }
    }
}
