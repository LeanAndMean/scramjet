//! A git package's programs from its releases, so that its users need no compiler. A
//! `models.json` in the package may say `"release": {"repository": "OWNER/REPO", "program":
//! "PATH"}`: `install` and `update` then take, from the GitHub release of the clone's newest
//! `vX.Y.Z` tag, `NAME-X.Y.Z-ARCH-OS.tar.gz` (NAME is PATH's file name), check it against the
//! release's `SHA256SUMS` and unpack its NAME (`NAME.exe` on Windows) to PATH, from the
//! `models.json`'s directory. The download goes through `gh`, as `update self`'s.
use super::{packages, update};
use scramjet_rs::agent::sets_in;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// In the clone's `.git`: one `TAG PATH` line per program downloaded.
const STAMP: &str = "scramjet-rs-programs";

struct Wanted {
    repository: String,
    /// As written in the `models.json`, from the package's top.
    shown: String,
    path: PathBuf,
}

/// What the package cloned in `directory` downloaded, and what it could not.
pub(super) fn fetch(directory: &Path) -> (Vec<String>, Vec<String>) {
    let gh = scramjet_rs::os::program("gh", None);
    fetch_with(&gh, directory, std::env::consts::ARCH)
}

fn fetch_with(gh: &Path, directory: &Path, arch: &str) -> (Vec<String>, Vec<String>) {
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    let wanted: Vec<Wanted> = sets_in(directory)
        .iter()
        .filter_map(|set| {
            wanted(set, directory)
                .map_err(|why| failed.push(why))
                .ok()?
        })
        .collect();
    if wanted.is_empty() {
        return (done, failed);
    }
    let tag = match packages::newest_tag(directory) {
        Ok(Some(tag)) => tag,
        Ok(None) => {
            for program in &wanted {
                failed.push(format!(
                    "{}: no vX.Y.Z tag in the clone's history, so no release to take it from; \
                     build it from the package's source",
                    program.shown
                ));
            }
            return (done, failed);
        }
        Err(why) => {
            failed.push(why);
            return (done, failed);
        }
    };
    let stamp = directory.join(".git").join(STAMP);
    let mut stamped: Vec<String> = fs::read_to_string(&stamp)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut changed = false;
    for program in &wanted {
        let line = format!("{tag} {}", program.shown);
        let file = with_extension(&program.path);
        if file.is_file() && stamped.contains(&line) {
            continue;
        }
        match download(gh, program, &tag, arch, &file) {
            Ok(()) => {
                done.push(format!(
                    "Downloaded {} from release {tag} of {}",
                    program.shown, program.repository
                ));
                let shown = format!(" {}", program.shown);
                stamped.retain(|known| !known.ends_with(&shown));
                stamped.push(line);
                changed = true;
            }
            Err(why) => failed.push(format!("{}: {why}", program.shown)),
        }
    }
    if changed && let Err(error) = fs::write(&stamp, stamped.join("\n") + "\n") {
        failed.push(format!("{}: {error}", stamp.display()));
    }
    (done, failed)
}

/// The program the `models.json` in `set` takes from a release, if it names one.
fn wanted(set: &Path, package: &Path) -> Result<Option<Wanted>, String> {
    let file = set.join("models.json");
    let Ok(text) = fs::read(&file) else {
        return Ok(None);
    };
    let shown_file = file.strip_prefix(package).unwrap_or(&file).display();
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return Ok(None);
    };
    let Some(release) = document.get("release") else {
        return Ok(None);
    };
    let wrong = || {
        format!(
            "{shown_file}: release must be {{\"repository\": \"OWNER/REPO\", \"program\": \
             \"PATH\"}}, PATH relative and inside the package"
        )
    };
    let text = |key: &str| release.get(key).and_then(|value| value.as_str());
    let (Some(repository), Some(program)) = (text("repository"), text("program")) else {
        return Err(wrong());
    };
    let part = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['-', '.'])
            && (part.chars()).all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    let named =
        matches!(repository.split_once('/'), Some((owner, name)) if part(owner) && part(name));
    let relative = Path::new(program);
    let inside = !program.is_empty()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !named || !inside {
        return Err(wrong());
    }
    let path = set.join(relative);
    Ok(Some(Wanted {
        repository: repository.to_owned(),
        shown: path
            .strip_prefix(package)
            .unwrap_or(&path)
            .display()
            .to_string(),
        path,
    }))
}

fn with_extension(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(scramjet_rs::os::program_file(&name))
}

fn download(gh: &Path, program: &Wanted, tag: &str, arch: &str, file: &Path) -> Result<(), String> {
    let name = program
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let archive_name = update::archive_name(&name, version, arch);
    let archive = update::download(gh, &program.repository, tag, &archive_name)?;
    let inner = scramjet_rs::os::program_file(&name);
    let bytes = update::unpacked(&archive, &inner)
        .ok_or_else(|| format!("{archive_name} holds no {inner}"))?;
    let parent = file.parent().unwrap_or(file);
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let new = file.with_file_name(format!(".{inner}.new-{}", std::process::id()));
    let _ = fs::remove_file(&new);
    let written = update::write_program(&new, &bytes)
        .and_then(|()| fs::rename(&new, file))
        .map_err(|error| format!("cannot write {}: {error}", file.display()));
    if written.is_err() {
        let _ = fs::remove_file(&new);
    }
    written
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{os::unix::fs::PermissionsExt, process::Command};

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sh(directory: &Path, script: &str) {
        let status = Command::new("sh")
            .args(["-c", script])
            .current_dir(directory)
            .status()
            .unwrap();
        assert!(status.success(), "{script}");
    }

    /// A clone holding a `models.json` with `release`, tagged `v0.2.2`, a release of it whose
    /// archive holds a program printing `hello`, and a `gh` serving it; `sums` makes its
    /// `SHA256SUMS` from the right one.
    fn package(test: &str, release: &str, sums: fn(&str) -> String) -> (Scratch, PathBuf) {
        let scratch = std::env::temp_dir().join(format!(
            "scramjet-rs-programs-{test}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&scratch);
        let (clone, stage, assets) = (
            scratch.join("clone"),
            scratch.join("stage"),
            scratch.join("assets"),
        );
        for directory in [&clone, &stage, &assets] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(
            clone.join("models.json"),
            format!(r#"{{"providersCommand": ["bin/tool", "providers"], "release": {release}}}"#),
        )
        .unwrap();
        sh(
            &clone,
            "git init -q && git add . && git -c user.name=t -c user.email=t@t commit -qm one \
             && git tag v0.2.2 && git -c user.name=t -c user.email=t@t commit -qm two \
             --allow-empty",
        );
        fs::write(stage.join("tool"), "#!/bin/sh\necho hello\n").unwrap();
        let name = update::archive_name("tool", "0.2.2", "x86_64");
        sh(
            &stage,
            &format!("tar -czf '{}/{name}' tool", assets.display()),
        );
        let hash: String = Sha256::digest(fs::read(assets.join(&name)).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        fs::write(
            assets.join("SHA256SUMS"),
            sums(&format!("{hash}  {name}\n")),
        )
        .unwrap();
        let gh = scratch.join("gh");
        fs::write(
            &gh,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{calls}'\ncase \"$*\" in\n\
                 *assets*) ls '{assets}' ;;\n\
                 'release download'*) while [ $# -gt 0 ]; do [ \"$1\" = --dir ] && cp '{assets}'/* \"$2\"; shift; done ;;\n\
                 esac\n",
                calls = scratch.join("calls").display(),
                assets = assets.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        (Scratch(scratch), clone)
    }

    const RELEASE: &str = r#"{"repository": "owner/tool", "program": "bin/tool"}"#;

    fn calls(scratch: &Scratch) -> String {
        fs::read_to_string(scratch.0.join("calls")).unwrap_or_default()
    }

    #[test]
    fn the_newest_tags_release_brings_the_program_once() {
        let (scratch, clone) = package("once", RELEASE, str::to_owned);
        let gh = scratch.0.join("gh");
        let (done, failed) = fetch_with(&gh, &clone, "x86_64");
        assert_eq!(
            (done, failed),
            (
                vec!["Downloaded bin/tool from release v0.2.2 of owner/tool".to_owned()],
                vec![]
            )
        );
        let program = clone.join("bin/tool");
        let output = Command::new(&program).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hello\n");
        assert_eq!(
            fs::metadata(&program).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_to_string(clone.join(".git").join(STAMP)).unwrap(),
            "v0.2.2 bin/tool\n"
        );
        assert!(calls(&scratch).contains(&format!(
            "release download v0.2.2 --repo owner/tool --pattern tool-0.2.2-x86_64-{}.tar.gz",
            std::env::consts::OS
        )));

        fs::remove_file(scratch.0.join("calls")).unwrap();
        assert_eq!(fetch_with(&gh, &clone, "x86_64"), (vec![], vec![]));
        assert_eq!(calls(&scratch), "");

        sh(&clone, "git tag v0.3.0");
        let (done, failed) = fetch_with(&gh, &clone, "x86_64");
        assert_eq!(
            failed,
            ["bin/tool: release v0.3.0 of owner/tool has no \
             tool-0.3.0-x86_64-linux.tar.gz for this platform"
                .replace("linux", std::env::consts::OS)]
        );
        assert!(done.is_empty());
        assert!(program.is_file());
    }

    #[test]
    fn a_wrong_checksum_another_platform_or_no_tag_brings_nothing() {
        let wrong_hash = |line: &str| format!("{}{}", "0".repeat(64), &line[64..]);
        let (scratch, clone) = package("hash", RELEASE, wrong_hash);
        let gh = scratch.0.join("gh");
        let (_, failed) = fetch_with(&gh, &clone, "x86_64");
        assert_eq!(failed.len(), 1);
        assert!(
            failed[0].contains("does not match its checksum"),
            "{failed:?}"
        );
        let (_, failed) = fetch_with(&gh, &clone, "aarch64");
        assert!(
            failed[0].contains("has no tool-0.2.2-aarch64-"),
            "{failed:?}"
        );
        sh(&clone, "git tag -d v0.2.2 >/dev/null");
        let (_, failed) = fetch_with(&gh, &clone, "x86_64");
        assert_eq!(
            failed,
            [
                "bin/tool: no vX.Y.Z tag in the clone's history, so no release to take it from; \
              build it from the package's source"
            ]
        );
        assert!(!clone.join("bin").exists());
        assert!(!clone.join(".git").join(STAMP).exists());
    }

    #[test]
    fn a_release_entry_must_name_a_repository_and_a_path_inside() {
        for release in [
            r#"{"repository": "owner/tool", "program": "../tool"}"#,
            r#"{"repository": "owner/tool", "program": "/bin/tool"}"#,
            r#"{"repository": "owner/tool", "program": ""}"#,
            r#"{"repository": "--repo=x/y", "program": "tool"}"#,
            r#"{"repository": "owner", "program": "tool"}"#,
            r#""owner/tool""#,
        ] {
            let (scratch, clone) = package("wrong", release, str::to_owned);
            let (done, failed) = fetch_with(&scratch.0.join("gh"), &clone, "x86_64");
            assert!(done.is_empty(), "{release}");
            assert_eq!(
                failed,
                [
                    "models.json: release must be {\"repository\": \"OWNER/REPO\", \"program\": \
                  \"PATH\"}, PATH relative and inside the package"
                ],
                "{release}"
            );
            assert_eq!(calls(&scratch), "", "{release}");
        }
    }
}
