//! `scramjet update self`: the newest release of the public `LeanAndMean/scramjet` replaces
//! this program, downloaded over HTTPS. The archive is checked against the release's
//! `SHA256SUMS` and the program in it must run and say the release's version before it is
//! renamed into place. A package's programs from a private repository's releases come through
//! `gh` (`download`), run as a program, so its token is never read here.
use flate2::read::GzDecoder;
use semver::Version;
use serde_json::Value;
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

const REPOSITORY: &str = "LeanAndMean/scramjet";
const RELEASES: &str = "https://api.github.com/repos/LeanAndMean/scramjet/releases?per_page=100";
const PROGRAM: &str = if cfg!(windows) {
    "scramjet.exe"
} else {
    "scramjet"
};

/// `SCRAMJET_RELEASES` names another release list, for tests and rehearsals.
pub(super) fn update_self() -> Result<(), String> {
    let program =
        std::env::current_exe().map_err(|error| format!("cannot find this program: {error}"))?;
    let releases = std::env::var("SCRAMJET_RELEASES").unwrap_or_else(|_| RELEASES.to_owned());
    println!(
        "{}",
        replace(
            &releases,
            &program,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH
        )?
    );
    Ok(())
}

/// The release archive of `program` for `version` on this platform, as `scripts/package.sh`
/// names scramjet's.
pub(super) fn archive_name(program: &str, version: &str, arch: &str) -> String {
    format!("{program}-{version}-{arch}-{}.tar.gz", std::env::consts::OS)
}

fn get(what: &str, url: &str, limit: usize, timeout: Duration) -> Result<Vec<u8>, String> {
    let agent = concat!("scramjet/", env!("CARGO_PKG_VERSION"));
    let headers = [
        ("User-Agent", agent),
        ("Accept", "application/vnd.github+json"),
    ];
    scramjet::agent::download(what, url, &headers, limit, timeout)
}

fn replace(releases: &str, program: &Path, current: &str, arch: &str) -> Result<String, String> {
    let running =
        Version::parse(current).map_err(|error| format!("this version {current:?}: {error}"))?;
    let what = format!("{REPOSITORY}'s release list");
    let listed = get(&what, releases, 16 << 20, Duration::from_secs(30))?;
    let listed: Value =
        serde_json::from_slice(&listed).map_err(|error| format!("{what} is not JSON: {error}"))?;
    let Some((newest, release)) = newest(&listed) else {
        return Ok(format!(
            "scramjet {current} is up to date ({REPOSITORY} has no release)"
        ));
    };
    let tag = release["tag_name"].as_str().unwrap_or_default();
    if newest.cmp_precedence(&running).is_le() {
        return Ok(format!(
            "scramjet {current} is up to date (newest release {tag})"
        ));
    }
    let latest = newest.to_string();
    let name = archive_name("scramjet", &latest, arch);
    let asset = |file: &str| {
        let assets = release["assets"].as_array().into_iter().flatten();
        let mut found = assets.filter(|asset| asset["name"] == file);
        found.find_map(|asset| asset["browser_download_url"].as_str())
    };
    let Some(url) = asset(&name) else {
        return Err(format!(
            "release {tag} of {REPOSITORY} has no {name} for this platform"
        ));
    };
    let sums = asset("SHA256SUMS")
        .ok_or_else(|| format!("release {tag} has no SHA256SUMS; nothing was replaced"))?;
    let sums = get(
        &format!("SHA256SUMS of release {tag}"),
        sums,
        1 << 20,
        Duration::from_secs(30),
    )?;
    let archive = get(
        &format!("{name} of release {tag}"),
        url,
        256 << 20,
        Duration::from_secs(600),
    )?;
    checked(&String::from_utf8_lossy(&sums), &archive, &name, tag)?;
    let bytes = unpacked(&archive, PROGRAM)
        .ok_or_else(|| format!("{name} holds no {PROGRAM}; nothing was replaced"))?;
    let new = program.with_file_name(format!(".{PROGRAM}.new-{}", std::process::id()));
    let result = write_program(&new, &bytes)
        .map_err(|error| format!("cannot write {}: {error}", new.display()))
        .and_then(|()| check_version(&new, &latest))
        .and_then(|()| swap(&new, program));
    if result.is_err() {
        let _ = fs::remove_file(&new);
    }
    result?;
    Ok(format!(
        "Updated scramjet: {current}..{latest} ({}); What's new opens at its next start",
        program.display()
    ))
}

/// The newest of the `listed` releases with its version, prereleases included: Scramjet 2's are
/// all prereleases until one is called stable.
fn newest(listed: &Value) -> Option<(Version, &Value)> {
    let releases = listed
        .as_array()?
        .iter()
        .filter(|release| release["draft"] != true);
    let versions = releases.filter_map(|release| {
        let tag = release["tag_name"].as_str()?;
        Some((
            Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?,
            release,
        ))
    });
    versions.max_by(|a, b| a.0.cmp_precedence(&b.0))
}

/// Fails unless `archive`'s SHA-256 is the one `sums` lists for `name`.
fn checked(sums: &str, archive: &[u8], name: &str, tag: &str) -> Result<(), String> {
    let expected = sums
        .lines()
        .find_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim_start().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
        })
        .ok_or_else(|| format!("SHA256SUMS of release {tag} does not list {name}"))?;
    let actual: String = Sha256::digest(archive)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    match actual == expected {
        true => Ok(()),
        false => Err(format!(
            "{name} does not match its checksum in SHA256SUMS; nothing was replaced"
        )),
    }
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
    checked(&sums, &archive, name, tag)?;
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
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    struct Release {
        directory: PathBuf,
        releases: String,
        program: PathBuf,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for Release {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    /// A scratch directory holding the program to replace, and a local server listing a draft,
    /// an older release and the release of `tag`, whose program prints `says` and whose assets
    /// redirect to where their files are, as GitHub's do.
    fn release(test: &str, tag: &str, says: &str, sums: impl Fn(&str) -> String) -> Release {
        let directory = std::env::temp_dir().join(format!(
            "scramjet-update-test-{test}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let (stage, assets) = (directory.join("stage"), directory.join("assets"));
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let staged = stage.join(PROGRAM);
        fs::write(&staged, format!("#!/bin/sh\necho {says}\n")).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
        let name = archive_name("scramjet", tag.trim_start_matches('v'), "x86_64");
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
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let asset = |file: &str| serde_json::json!({"name": file, "browser_download_url": format!("{base}/download/{tag}/{file}")});
        let listing = serde_json::json!([
            {"tag_name": "v99.0.0", "draft": true, "assets": [asset(&name)]},
            {"tag_name": tag, "prerelease": true, "assets": [asset(&name), asset("SHA256SUMS")]},
            {"tag_name": "v0.1.0", "assets": []},
        ]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (served, logged) = (assets.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
                let request = lines.next().unwrap().unwrap();
                while lines.next().is_some_and(|line| !line.unwrap().is_empty()) {}
                let path = request.split(' ').nth(1).unwrap().to_owned();
                logged.lock().unwrap().push(path.clone());
                let (status, header, body) = match path.split('/').collect::<Vec<_>>()[..] {
                    ["", "releases"] => (200, String::new(), listing.to_string().into_bytes()),
                    ["", "download", _, file] => {
                        (302, format!("Location: /assets/{file}\r\n"), vec![])
                    }
                    ["", "assets", file] => {
                        (200, String::new(), fs::read(served.join(file)).unwrap())
                    }
                    _ => (404, String::new(), vec![]),
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let program = directory.join(PROGRAM);
        fs::write(&program, "old").unwrap();
        Release {
            directory,
            releases: format!("{base}/releases"),
            program,
            requests,
        }
    }

    fn requests(release: &Release) -> Vec<String> {
        release.requests.lock().unwrap().clone()
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
        for (test, tag, running) in [
            ("newer", "v0.10.0", "0.9.3"),
            ("from-0.x", "v2.140.0", "0.140.0"),
            ("later", "v2.141.0", "2.140.0"),
            ("alpha", "v2.0.0-alpha.10", "2.0.0-alpha.9"),
        ] {
            let latest = &tag[1..];
            let release = release(test, tag, latest, str::to_owned);
            let said = replace(&release.releases, &release.program, running, "x86_64").unwrap();
            assert!(
                said.starts_with(&format!("Updated scramjet: {running}..{latest} (")),
                "{said}"
            );
            assert_eq!(
                fs::read_to_string(&release.program).unwrap(),
                format!("#!/bin/sh\necho {latest}\n")
            );
            let mode = fs::metadata(&release.program).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
            let name = archive_name("scramjet", latest, "x86_64");
            assert_eq!(
                requests(&release),
                [
                    "/releases".to_owned(),
                    format!("/download/{tag}/SHA256SUMS"),
                    "/assets/SHA256SUMS".to_owned(),
                    format!("/download/{tag}/{name}"),
                    format!("/assets/{name}"),
                ]
            );
            assert_eq!(leftovers(&release), ["assets", PROGRAM, "stage"]);
        }
    }

    #[test]
    fn the_same_or_an_older_release_downloads_nothing() {
        for (test, tag, running) in [
            ("same", "v2.140.0", "2.140.0"),
            ("older", "v2.140.0", "2.141.0"),
            ("final", "v2.0.0-alpha.3", "2.0.0"),
        ] {
            let release = release(test, tag, &tag[1..], str::to_owned);
            let said = replace(&release.releases, &release.program, running, "x86_64").unwrap();
            assert_eq!(
                said,
                format!("scramjet {running} is up to date (newest release {tag})")
            );
            assert_eq!(fs::read_to_string(&release.program).unwrap(), "old");
            assert_eq!(requests(&release), ["/releases"]);
        }
    }

    #[test]
    fn a_wrong_checksum_or_version_replaces_nothing() {
        let wrong_hash = |line: &str| format!("{}{}", "0".repeat(64), &line[64..]);
        let other_file = |line: &str| line.replace("scramjet-", "other-");
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
            let error =
                replace(&release.releases, &release.program, "0.9.3", "x86_64").unwrap_err();
            assert!(error.contains(problem), "{test}: {error}");
            assert_eq!(fs::read_to_string(&release.program).unwrap(), "old");
            assert_eq!(leftovers(&release), ["assets", PROGRAM, "stage"]);
        }
    }

    #[test]
    fn a_release_without_this_platform_or_an_unreachable_list_says_so() {
        let release = release("platform", "v0.10.0", "0.10.0", str::to_owned);
        let error = replace(&release.releases, &release.program, "0.9.3", "riscv64").unwrap_err();
        assert_eq!(
            error,
            format!(
                "release v0.10.0 of LeanAndMean/scramjet has no {} for this platform",
                archive_name("scramjet", "0.10.0", "riscv64")
            )
        );
        let gone = release.releases.replace("/releases", "/gone");
        let error = replace(&gone, &release.program, "0.9.3", "x86_64").unwrap_err();
        assert_eq!(
            error,
            "LeanAndMean/scramjet's release list answered HTTP 404"
        );
        assert_eq!(fs::read_to_string(&release.program).unwrap(), "old");
    }

    #[test]
    fn gh_missing_or_failing_says_what_to_do() {
        let missing = download(Path::new("/nonexistent/gh"), "o/r", "v1", "a.tar.gz");
        assert!(
            missing
                .unwrap_err()
                .contains("install `gh` and run `gh auth login`")
        );
        let failing = download(Path::new("/bin/false"), "o/r", "v1", "a.tar.gz");
        assert_eq!(
            failing.unwrap_err(),
            "gh release view failed; `gh auth status` shows whether it is logged in to an \
             account that can read o/r"
        );
    }

    #[test]
    fn the_newest_is_by_version_drafts_left_out() {
        let newest =
            |tags: serde_json::Value| newest(&tags).map(|(version, _)| version.to_string());
        let tags = serde_json::json!([
            {"tag_name": "v2.0.0-alpha.9"},
            {"tag_name": "v2.0.0-alpha.10", "prerelease": true},
            {"tag_name": "v0.117.0"},
            {"tag_name": "v3.0.0", "draft": true},
            {"tag_name": "not-a-version"},
        ]);
        assert_eq!(newest(tags).unwrap(), "2.0.0-alpha.10");
        let tags = serde_json::json!([{"tag_name": "v2.140.0"}, {"tag_name": "v2.99.1", "prerelease": true}]);
        assert_eq!(newest(tags).unwrap(), "2.140.0");
        assert_eq!(newest(serde_json::json!([])), None);
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
