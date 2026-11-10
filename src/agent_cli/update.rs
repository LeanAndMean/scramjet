//! `scramjet-rs update self`: the newest release of `LeanAndMean/scramjet-rs` replaces this
//! program. While the repository is private the download goes through `gh`, run as a program,
//! so its token is never read here; the archive is checked against the release's `SHA256SUMS`
//! and the program in it must run and say the release's version before it is renamed into place.
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const REPOSITORY: &str = "LeanAndMean/scramjet-rs";
const PROGRAM: &str = if cfg!(windows) {
    "scramjet-rs.exe"
} else {
    "scramjet-rs"
};

pub(super) fn update_self() -> Result<(), String> {
    let program =
        std::env::current_exe().map_err(|error| format!("cannot find this program: {error}"))?;
    let gh = scramjet_rs::os::program("gh", None);
    println!(
        "{}",
        replace(
            &gh,
            &program,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH
        )?
    );
    Ok(())
}

/// The release archive of `program` for `version` on this platform, as `scripts/package.sh`
/// names scramjet-rs's.
pub(super) fn archive_name(program: &str, version: &str, arch: &str) -> String {
    format!("{program}-{version}-{arch}-{}.tar.gz", std::env::consts::OS)
}

fn replace(gh: &Path, program: &Path, current: &str, arch: &str) -> Result<String, String> {
    let view = ["release", "view", "--repo", REPOSITORY];
    let tag = run_gh(
        gh,
        REPOSITORY,
        view.iter()
            .chain(&["--json", "tagName", "--jq", ".tagName"]),
    )?;
    let tag = tag.trim();
    let latest = tag.strip_prefix('v').unwrap_or(tag);
    let (Some(newest), Some(running)) = (parsed(latest), parsed(current)) else {
        return Err(format!("the newest release's tag {tag:?} is not a version"));
    };
    if newest <= running {
        return Ok(format!(
            "scramjet-rs {current} is up to date (newest release {tag})"
        ));
    }
    let name = archive_name("scramjet-rs", latest, arch);
    let archive = download(gh, REPOSITORY, tag, &name)?;
    let bytes = unpacked(&archive, PROGRAM)
        .ok_or_else(|| format!("{name} holds no {PROGRAM}; nothing was replaced"))?;
    let new = program.with_file_name(format!(".{PROGRAM}.new-{}", std::process::id()));
    let result = write_program(&new, &bytes)
        .map_err(|error| format!("cannot write {}: {error}", new.display()))
        .and_then(|()| check_version(&new, latest))
        .and_then(|()| swap(&new, program));
    if result.is_err() {
        let _ = fs::remove_file(&new);
    }
    result?;
    Ok(format!(
        "Updated scramjet-rs: {current}..{latest} ({}); What's new opens at its next start",
        program.display()
    ))
}

/// The archive `name` of `repository`'s release `tag`, downloaded through `gh` and checked
/// against the release's `SHA256SUMS`.
pub(super) fn download(
    gh: &Path,
    repository: &str,
    tag: &str,
    name: &str,
) -> Result<Vec<u8>, String> {
    let assets = [
        "release",
        "view",
        tag,
        "--repo",
        repository,
        "--json",
        "assets",
        "--jq",
        ".assets[].name",
    ];
    let assets = run_gh(gh, repository, assets)?;
    if !assets.lines().any(|asset| asset.trim() == name) {
        return Err(format!(
            "release {tag} of {repository} has no {name} for this platform"
        ));
    }
    let scratch = Scratch::new()?;
    let download = [
        OsStr::new("release"),
        OsStr::new("download"),
        OsStr::new(tag),
        OsStr::new("--repo"),
        OsStr::new(repository),
        OsStr::new("--pattern"),
        OsStr::new(name),
        OsStr::new("--pattern"),
        OsStr::new("SHA256SUMS"),
        OsStr::new("--dir"),
        scratch.0.as_os_str(),
    ];
    run_gh(gh, repository, download)?;
    let read = |file: &str| {
        fs::read(scratch.0.join(file))
            .map_err(|error| format!("{file} of release {tag} did not download: {error}"))
    };
    let sums = String::from_utf8_lossy(&read("SHA256SUMS")?).into_owned();
    let archive = read(name)?;
    let expected = sums
        .lines()
        .find_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim_start().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
        })
        .ok_or_else(|| format!("SHA256SUMS of release {tag} does not list {name}"))?;
    let actual: String = Sha256::digest(&archive)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(format!(
            "{name} does not match its checksum in SHA256SUMS; nothing was replaced"
        ));
    }
    Ok(archive)
}

fn run_gh<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(
    gh: &Path,
    repository: &str,
    args: I,
) -> Result<String, String> {
    let args: Vec<S> = args.into_iter().collect();
    let what = args
        .iter()
        .take(2)
        .map(|arg| arg.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let output = Command::new(gh)
        .args(&args)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output();
    match output {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(format!(
            "downloads from {repository}'s releases go through GitHub's CLI while it is private: \
             install `gh` and run `gh auth login` with an account that can read {repository}"
        )),
        Err(error) => Err(format!("gh: {error}")),
        Ok(output) if !output.status.success() => Err(format!(
            "gh {what} failed; `gh auth status` shows whether it is logged in to an account that \
             can read {repository}"
        )),
        Ok(output) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
    }
}

fn parsed(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// The file `name` in a gzipped tar archive, at its top or in a directory.
pub(super) fn unpacked(archive: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut tar = Vec::new();
    GzDecoder::new(archive).read_to_end(&mut tar).ok()?;
    let mut rest = tar.as_slice();
    while rest.len() >= 512 && rest[..512].iter().any(|&byte| byte != 0) {
        let (header, after) = rest.split_at(512);
        let field = |range: std::ops::Range<usize>| {
            let bytes = &header[range];
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        };
        let size = u64::from_str_radix(field(124..136).trim(), 8).ok()? as usize;
        let path = match field(345..500) {
            prefix if prefix.is_empty() => field(0..100),
            prefix => format!("{prefix}/{}", field(0..100)),
        };
        let regular = matches!(header[156], b'0' | 0);
        let data = after.get(..size)?;
        if regular && path.rsplit('/').next() == Some(name) {
            return Some(data.to_vec());
        }
        rest = after.get(size.div_ceil(512) * 512..)?;
    }
    None
}

pub(super) fn write_program(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn check_version(new: &Path, latest: &str) -> Result<(), String> {
    // A process another thread forks while the file is open for writing keeps it busy until it
    // execs.
    let mut tries = 0;
    let output = loop {
        let ran = Command::new(new)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match ran {
            Err(error) if cfg!(unix) && error.raw_os_error() == Some(26) && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            ran => {
                break ran.map_err(|error| {
                    format!("the downloaded program does not run here: {error}")
                })?;
            }
        }
    };
    let said = String::from_utf8_lossy(&output.stdout);
    match output.status.success() && said.trim() == latest {
        true => Ok(()),
        false => Err(format!(
            "the downloaded program says {:?} rather than {latest}; nothing was replaced",
            said.trim()
        )),
    }
}

/// Renames `new` over `program`; Windows keeps a running `.exe` from being replaced or removed,
/// not from being renamed, so it is moved aside first to a free `.old` name. Those left by
/// earlier updates are removed here once no session still runs them.
fn swap(new: &Path, program: &Path) -> Result<(), String> {
    let failed = |error: io::Error| format!("cannot replace {}: {error}", program.display());
    if cfg!(windows) {
        let name = program.file_name().unwrap_or_default().to_string_lossy();
        let aside = |n: usize| match n {
            0 => program.with_file_name(format!("{name}.old")),
            n => program.with_file_name(format!("{name}.old-{n}")),
        };
        for n in 0..100 {
            let _ = fs::remove_file(aside(n));
        }
        let old = (0..100)
            .map(aside)
            .find(|old| fs::symlink_metadata(old).is_err())
            .ok_or_else(|| {
                format!(
                    "cannot replace {}: 100 earlier copies still run",
                    program.display()
                )
            })?;
        fs::rename(program, &old).map_err(failed)?;
        if let Err(error) = fs::rename(new, program) {
            let _ = fs::rename(&old, program);
            return Err(failed(error));
        }
        return Ok(());
    }
    fs::rename(new, program).map_err(failed)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        static MADE: AtomicUsize = AtomicUsize::new(0);
        let made = MADE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("scramjet-rs-update-{}-{made}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path)
            .map_err(|error| format!("cannot make {}: {error}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Release {
        directory: PathBuf,
        gh: PathBuf,
        program: PathBuf,
    }

    impl Drop for Release {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    /// A scratch directory holding the program to replace, a release of `tag` whose program
    /// prints `says`, its archive made by `tar`, and a `gh` serving it.
    fn release(test: &str, tag: &str, says: &str, sums: impl Fn(&str) -> String) -> Release {
        let directory = std::env::temp_dir().join(format!(
            "scramjet-rs-update-test-{test}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let (stage, assets) = (directory.join("stage"), directory.join("assets"));
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let executable = |path: &Path, text: &str| {
            fs::write(path, text).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        };
        executable(&stage.join(PROGRAM), &format!("#!/bin/sh\necho {says}\n"));
        let name = archive_name("scramjet-rs", tag.trim_start_matches('v'), "x86_64");
        let made = Command::new("tar")
            .arg("-czf")
            .arg(assets.join(&name))
            .arg("-C")
            .arg(&stage)
            .arg(PROGRAM)
            .status()
            .unwrap();
        assert!(made.success());
        let hash: String = Sha256::digest(fs::read(assets.join(&name)).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        fs::write(
            assets.join("SHA256SUMS"),
            sums(&format!("{hash}  {name}\n")),
        )
        .unwrap();
        let gh = directory.join("gh");
        let log = directory.join("calls");
        executable(
            &gh,
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$*\" in\n\
                 *assets*) ls '{assets}' ;;\n\
                 'release view'*) echo {tag} ;;\n\
                 'release download'*) while [ $# -gt 0 ]; do [ \"$1\" = --dir ] && cp '{assets}'/* \"$2\"; shift; done ;;\n\
                 esac\n",
                log = log.display(),
                assets = assets.display(),
            ),
        );
        let program = directory.join(PROGRAM);
        fs::write(&program, "old").unwrap();
        Release {
            directory,
            gh,
            program,
        }
    }

    fn calls(release: &Release) -> String {
        fs::read_to_string(release.directory.join("calls")).unwrap_or_default()
    }

    fn leftovers(release: &Release) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&release.directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_newer_release_replaces_the_program_after_its_checks() {
        let release = release("newer", "v0.10.0", "0.10.0", str::to_owned);
        let said = replace(&release.gh, &release.program, "0.9.3", "x86_64").unwrap();
        assert!(
            said.starts_with("Updated scramjet-rs: 0.9.3..0.10.0 ("),
            "{said}"
        );
        assert_eq!(
            fs::read_to_string(&release.program).unwrap(),
            "#!/bin/sh\necho 0.10.0\n"
        );
        let mode = fs::metadata(&release.program).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        let calls = calls(&release);
        assert!(
            calls.starts_with(
                "release view --repo LeanAndMean/scramjet-rs --json tagName --jq .tagName\n\
             release view v0.10.0 --repo LeanAndMean/scramjet-rs --json assets --jq \
             .assets[].name\n\
             release download v0.10.0 --repo LeanAndMean/scramjet-rs --pattern \
             scramjet-rs-0.10.0-x86_64-"
            ),
            "{calls}"
        );
        assert_eq!(
            leftovers(&release),
            ["assets", "calls", "gh", PROGRAM, "stage"]
        );
    }

    #[test]
    fn the_same_or_an_older_release_downloads_nothing() {
        for (test, running) in [("same", "0.10.0"), ("older", "0.11.0")] {
            let release = release(test, "v0.10.0", "0.10.0", str::to_owned);
            let said = replace(&release.gh, &release.program, running, "x86_64").unwrap();
            assert_eq!(
                said,
                format!("scramjet-rs {running} is up to date (newest release v0.10.0)")
            );
            assert_eq!(fs::read_to_string(&release.program).unwrap(), "old");
            assert!(!calls(&release).contains("download"));
        }
    }

    #[test]
    fn a_wrong_checksum_or_version_replaces_nothing() {
        let wrong_hash = |line: &str| format!("{}{}", "0".repeat(64), &line[64..]);
        let other_file = |line: &str| line.replace("scramjet-rs-", "other-");
        type Sums<'a> = &'a dyn Fn(&str) -> String;
        let cases: [(&str, &str, Sums, &str); 3] = [
            (
                "hash",
                "0.10.0",
                &wrong_hash,
                "does not match its checksum in SHA256SUMS",
            ),
            (
                "listed",
                "0.10.0",
                &other_file,
                "SHA256SUMS of release v0.10.0 does not list",
            ),
            (
                "says",
                "0.9.0",
                &str::to_owned,
                "the downloaded program says \"0.9.0\" rather than 0.10.0",
            ),
        ];
        for (test, says, sums, problem) in cases {
            let release = release(test, "v0.10.0", says, sums);
            let error = replace(&release.gh, &release.program, "0.9.3", "x86_64").unwrap_err();
            assert!(error.contains(problem), "{test}: {error}");
            assert_eq!(fs::read_to_string(&release.program).unwrap(), "old");
            assert_eq!(
                leftovers(&release),
                ["assets", "calls", "gh", PROGRAM, "stage"]
            );
        }
    }

    #[test]
    fn gh_missing_or_failing_says_what_to_do() {
        let program = Path::new("/nonexistent/scramjet-rs");
        let missing = replace(Path::new("/nonexistent/gh"), program, "0.1.0", "x86_64");
        assert!(
            missing
                .unwrap_err()
                .contains("install `gh` and run `gh auth login`")
        );
        let failing = replace(Path::new("/bin/false"), program, "0.1.0", "x86_64");
        assert_eq!(
            failing.unwrap_err(),
            "gh release view failed; `gh auth status` shows whether it is logged in to an \
             account that can read LeanAndMean/scramjet-rs"
        );
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(parsed("0.10.0") > parsed("0.9.12"));
        assert_eq!(parsed("1.2"), None);
        assert_eq!(parsed("1.2.3-rc1"), None);
        assert_eq!(parsed("1.2.3.4"), None);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// Each copy of `PING.EXE` started stands for a session still running that program.
    #[test]
    fn the_running_program_is_moved_aside_while_earlier_ones_still_run() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-rs-swap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let ping = Path::new(&std::env::var_os("SystemRoot").unwrap()).join(r"System32\PING.EXE");
        let program = directory.join(PROGRAM);
        let new = directory.join("new");
        let start = || {
            Command::new(&program)
                .args(["-n", "60", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        };
        let names = || {
            let mut names: Vec<String> = fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        fs::copy(&ping, &program).unwrap();
        let mut running = vec![start()];
        for _ in 0..2 {
            fs::copy(&ping, &new).unwrap();
            swap(&new, &program).unwrap();
            running.push(start());
        }
        assert_eq!(
            names(),
            [
                PROGRAM,
                &format!("{PROGRAM}.old"),
                &format!("{PROGRAM}.old-1")
            ]
        );
        running.remove(0).kill().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        fs::copy(&ping, &new).unwrap();
        swap(&new, &program).unwrap();
        assert_eq!(
            names(),
            [
                PROGRAM,
                &format!("{PROGRAM}.old"),
                &format!("{PROGRAM}.old-1")
            ]
        );
        for mut child in running {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(500));
        fs::copy(&ping, &new).unwrap();
        swap(&new, &program).unwrap();
        assert_eq!(names(), [PROGRAM, &format!("{PROGRAM}.old")]);
        let _ = fs::remove_dir_all(&directory);
    }
}
