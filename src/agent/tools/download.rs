//! Downloads ripgrep and fd into the agent directory's `bin` when they are missing, as
//! Scramjet's tools manager does: the latest release from GitHub, unpacked with the system's
//! `tar` (or PowerShell on Windows). `SCRAMJET_OFFLINE` (or `PI_OFFLINE`) turns it off.

use super::search_program;
use crate::agent::client;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

const API: &str = "https://api.github.com";
const RELEASES: &str = "https://github.com";
const DOWNLOAD_BYTES: usize = 64 << 20;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const REDIRECTS: usize = 5;

/// One download at a time, so that a start and a tool call do not both fetch a program.
static DOWNLOADING: Mutex<()> = Mutex::new(());

/// What a tool needs: `rg` for `grep`, `fd` for `find`.
struct Program {
    shown: &'static str,
    repo: &'static str,
    binary: &'static str,
    tag_prefix: &'static str,
}

fn program(tool: &str) -> Program {
    match tool {
        "grep" => Program {
            shown: "ripgrep (rg)",
            repo: "BurntSushi/ripgrep",
            binary: "rg",
            tag_prefix: "",
        },
        _ => Program {
            shown: "fd",
            repo: "sharkdp/fd",
            binary: "fd",
            tag_prefix: "v",
        },
    }
}

/// The release archive of `binary` at `version` for `os` and `arch`, as Scramjet picks it.
fn asset(binary: &str, version: &str, os: &str, arch: &str) -> Option<String> {
    let arch = match arch {
        "x86_64" | "aarch64" => arch,
        _ => return None,
    };
    let target = match os {
        "macos" => format!("{arch}-apple-darwin.tar.gz"),
        "windows" => format!("{arch}-pc-windows-msvc.zip"),
        // ripgrep's x86_64 Linux build is static; fd's and ripgrep's aarch64 one need glibc.
        "linux" if binary == "rg" && arch == "x86_64" => {
            format!("{arch}-unknown-linux-musl.tar.gz")
        }
        "linux" => format!("{arch}-unknown-linux-gnu.tar.gz"),
        _ => return None,
    };
    Some(match binary {
        "rg" => format!("ripgrep-{version}-{target}"),
        _ => format!("fd-v{version}-{target}"),
    })
}

fn offline() -> bool {
    let value = std::env::var("SCRAMJET_OFFLINE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("PI_OFFLINE").ok())
        .unwrap_or_default()
        .to_lowercase();
    matches!(value.as_str(), "1" | "true" | "yes")
}

/// Downloads, at once, what `grep` and `find` need and neither the agent directory nor the
/// `PATH` has; what to tell of each.
pub fn download_missing() -> Vec<String> {
    let missing = ["grep", "find"].map(|tool| search_program(tool).is_none().then_some(tool));
    let downloads = missing
        .into_iter()
        .flatten()
        .map(|tool| (tool, std::thread::spawn(move || ensure(tool))));
    let downloads: Vec<_> = downloads.collect();
    let said = downloads.into_iter().filter_map(|(tool, download)| {
        let shown = program(tool).shown;
        Some(match download.join() {
            Ok(Ok((_, false))) => return None,
            Ok(Ok((path, true))) => format!(
                "Downloaded {shown}, which {tool} uses, to {}",
                super::shortened(&path.to_string_lossy())
            ),
            Ok(Err(error)) => format!("Warning: {error}"),
            Err(_) => format!("Warning: downloading {shown} failed"),
        })
    });
    said.collect()
}

/// What `tool` needs and cannot have here: its program, missing with downloads turned off.
pub fn unavailable(tool: &str) -> Option<&'static str> {
    let needs = matches!(tool, "grep" | "find");
    (needs && offline() && search_program(tool).is_none()).then(|| program(tool).shown)
}

/// The program `tool` runs, downloaded first when neither the agent directory nor the `PATH`
/// has it, and whether it was downloaded now.
pub(super) fn ensure(tool: &str) -> Result<(PathBuf, bool), String> {
    if let Some(found) = search_program(tool) {
        return Ok((found, false));
    }
    let program = program(tool);
    let missing = format!("{} is not installed", program.shown);
    if offline() {
        return Err(format!(
            "{missing}, and SCRAMJET_OFFLINE turns its download off"
        ));
    }
    let _one = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = search_program(tool) {
        return Ok((found, false));
    }
    let directory = crate::agent::agent_directory()
        .ok_or(format!(
            "{missing}, and HOME is not set to download it into"
        ))?
        .join("bin");
    let downloaded = download(&program, API, RELEASES, &directory);
    downloaded
        .map(|path| (path, true))
        .map_err(|error| format!("{missing}, and downloading it failed: {error}"))
}

/// Downloads `program`'s latest release from `releases`, as `api` names it, into `directory`.
fn download(
    program: &Program,
    api: &str,
    releases: &str,
    directory: &Path,
) -> Result<PathBuf, String> {
    let latest = format!("{api}/repos/{}/releases/latest", program.repo);
    let answer = crate::agent::login::blocking(get(&latest, true))?;
    let release: serde_json::Value = serde_json::from_slice(&answer)
        .map_err(|error| format!("GitHub's release is not JSON: {error}"))?;
    let tag = release["tag_name"]
        .as_str()
        .ok_or("GitHub's release has no tag")?;
    let mut version = tag.strip_prefix('v').unwrap_or(tag);
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    // fd's later releases have no x86_64 macOS build.
    if program.binary == "fd" && os == "macos" && arch == "x86_64" {
        version = "10.3.0";
    }
    let asset = asset(program.binary, version, os, arch)
        .ok_or(format!("no release is built for {os} on {arch}"))?;
    let url = format!(
        "{releases}/{}/releases/download/{}{version}/{asset}",
        program.repo, program.tag_prefix
    );
    let archive = crate::agent::login::blocking(get(&url, false))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let scratch = directory.join(format!(
        ".download-{}-{}-{nanos}",
        program.binary,
        std::process::id()
    ));
    let installed = unpack(&archive, &asset, program.binary, &scratch, directory);
    let _ = std::fs::remove_dir_all(&scratch);
    installed
}

/// `url`'s body, following redirects; `json`: GitHub's API, asked for JSON.
async fn get(url: &str, json: bool) -> Result<Vec<u8>, String> {
    let agent = format!("scramjet-rs/{}", env!("CARGO_PKG_VERSION"));
    let mut headers = vec![("user-agent", agent.as_str())];
    if json {
        headers.push(("accept", "application/vnd.github+json"));
    }
    let mut url = url.to_owned();
    for _ in 0..=REDIRECTS {
        let fetched = client::fetch_within(
            "GitHub",
            "GET",
            &url,
            &headers,
            None,
            DOWNLOAD_BYTES,
            DOWNLOAD_TIMEOUT,
        )
        .await?;
        match (fetched.status, fetched.location) {
            (200..=299, _) => return Ok(fetched.body),
            (301 | 302 | 303 | 307 | 308, Some(location)) => url = location,
            (status, _) => {
                let said = String::from_utf8_lossy(&fetched.body);
                let said: String = said.trim().chars().take(200).collect();
                return Err(format!("{url} answered HTTP {status}: {said}"));
            }
        }
    }
    Err(format!("{url} redirected more than {REDIRECTS} times"))
}

/// Unpacks `archive` (named `asset`) in `scratch` and moves its `binary` into `directory`.
fn unpack(
    archive: &[u8],
    asset: &str,
    binary: &str,
    scratch: &Path,
    directory: &Path,
) -> Result<PathBuf, String> {
    let unpacked = scratch.join("unpacked");
    std::fs::create_dir_all(&unpacked)
        .map_err(|error| format!("cannot create {}: {error}", unpacked.display()))?;
    let file = scratch.join(asset);
    std::fs::write(&file, archive)
        .map_err(|error| format!("cannot write {}: {error}", file.display()))?;
    extract(&file, &unpacked).map_err(|error| format!("cannot unpack {asset}: {error}"))?;
    let name = crate::os::program_file(binary);
    let found = find(&unpacked, &name).ok_or(format!("{asset} holds no {name}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&found, executable)
            .map_err(|error| format!("cannot make {name} executable: {error}"))?;
    }
    let installed = directory.join(&name);
    std::fs::rename(&found, &installed)
        .map_err(|error| format!("cannot move {name} into {}: {error}", directory.display()))?;
    Ok(installed)
}

fn extract(file: &Path, into: &Path) -> Result<(), String> {
    if cfg!(windows) {
        // Windows' own tar is bsdtar, which reads zip files; Git Bash's GNU tar does not.
        let tar = std::env::var_os("SystemRoot")
            .map(|root| Path::new(&root).join("System32").join("tar.exe"))
            .filter(|tar| tar.is_file())
            .unwrap_or_else(|| PathBuf::from("tar.exe"));
        let tarred = run(Command::new(tar).arg("xf").arg(file).arg("-C").arg(into));
        let Err(tar_failed) = tarred else {
            return Ok(());
        };
        let script = "& { param($archive, $destination) $ErrorActionPreference = 'Stop'; \
                      Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force }";
        let mut powershell = Command::new("powershell.exe");
        powershell.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
        ]);
        powershell.arg("-Command").arg(script).arg(file).arg(into);
        return run(&mut powershell).map_err(|failed| format!("{tar_failed}; {failed}"));
    }
    run(Command::new("tar").arg("xzf").arg(file).arg("-C").arg(into))
}

fn run(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("{program}: {} ({})", stderr.trim(), output.status))
}

/// The file named `name` under `directory`, at any depth.
fn find(directory: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![directory.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).ok()?.flatten() {
            let kind = entry.file_type().ok()?;
            if kind.is_file() && entry.file_name() == name {
                return Some(entry.path());
            }
            if kind.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_takes_the_release_scramjet_takes() {
        let cases = [
            (
                "rg",
                "linux",
                "x86_64",
                "ripgrep-15.1.0-x86_64-unknown-linux-musl.tar.gz",
            ),
            (
                "rg",
                "linux",
                "aarch64",
                "ripgrep-15.1.0-aarch64-unknown-linux-gnu.tar.gz",
            ),
            (
                "rg",
                "macos",
                "aarch64",
                "ripgrep-15.1.0-aarch64-apple-darwin.tar.gz",
            ),
            (
                "rg",
                "windows",
                "x86_64",
                "ripgrep-15.1.0-x86_64-pc-windows-msvc.zip",
            ),
            (
                "fd",
                "linux",
                "x86_64",
                "fd-v15.1.0-x86_64-unknown-linux-gnu.tar.gz",
            ),
            (
                "fd",
                "macos",
                "x86_64",
                "fd-v15.1.0-x86_64-apple-darwin.tar.gz",
            ),
            (
                "fd",
                "windows",
                "aarch64",
                "fd-v15.1.0-aarch64-pc-windows-msvc.zip",
            ),
        ];
        for (binary, os, arch, expected) in cases {
            assert_eq!(asset(binary, "15.1.0", os, arch).as_deref(), Some(expected));
        }
        assert_eq!(asset("rg", "15.1.0", "linux", "arm"), None);
        assert_eq!(asset("fd", "15.1.0", "freebsd", "x86_64"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_release_is_fetched_through_its_redirect_and_unpacked_into_bin() {
        let scratch = crate::agent::tests::ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-rs-download-{}", std::process::id())),
        );
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        let asset = asset("rg", "9.8.7", os, arch).unwrap();
        let folder = scratch
            .0
            .join("made")
            .join(asset.trim_end_matches(".tar.gz"));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("rg"), "#!/bin/sh\necho found\n").unwrap();
        std::fs::write(folder.join("README.md"), "").unwrap();
        let tarred = Command::new("tar")
            .arg("czf")
            .arg(scratch.0.join(&asset))
            .arg("-C")
            .arg(scratch.0.join("made"))
            .arg(".")
            .status();
        assert!(tarred.unwrap().success());
        let archive = std::fs::read(scratch.0.join(&asset)).unwrap();
        let base = std::sync::Arc::new(std::sync::OnceLock::<String>::new());
        let answers = {
            let base = base.clone();
            let mut answered = 0;
            move |_: &serde_json::Value| {
                answered += 1;
                let (head, body) = match answered {
                    1 => ("200 OK".to_owned(), br#"{"tag_name": "9.8.7"}"#.to_vec()),
                    2 => (
                        format!("302 Found\r\nLocation: {}/stored", base.get().unwrap()),
                        Vec::new(),
                    ),
                    _ => ("200 OK".to_owned(), archive.clone()),
                };
                let mut response = format!(
                    "HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend(body);
                response
            }
        };
        let (address, server) = crate::agent::tests::serve_answering(3, answers);
        base.set(format!("http://{address}")).unwrap();
        let bin = scratch.0.join("agent").join("bin");
        let installed = download(
            &program("grep"),
            base.get().unwrap(),
            base.get().unwrap(),
            &bin,
        );
        assert_eq!(installed, Ok(bin.join("rg")));
        let ran = Command::new(bin.join("rg")).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ran.stdout), "found\n");
        // Only the program is left: the archive and what else it held are gone.
        let left: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["rg"]);
        let requests = server.join().unwrap();
        let lines: Vec<&str> = requests
            .iter()
            .map(|(head, _)| head.lines().next().unwrap())
            .collect();
        let expected = format!(
            "get /burntsushi/ripgrep/releases/download/9.8.7/{} http/1.1",
            asset.to_lowercase()
        );
        assert_eq!(
            lines,
            [
                "get /repos/burntsushi/ripgrep/releases/latest http/1.1",
                &expected,
                "get /stored http/1.1"
            ]
        );
        assert!(requests[0].0.contains("user-agent: scramjet-rs/"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_download_says_why_and_leaves_nothing() {
        let scratch = crate::agent::tests::ScratchDir::new(std::env::temp_dir().join(format!(
            "scramjet-rs-download-failed-{}",
            std::process::id()
        )));
        let answer = |_: &serde_json::Value| {
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 14\r\n\r\nrate limited.\n".to_vec()
        };
        let (address, server) = crate::agent::tests::serve_answering(1, answer);
        let base = format!("http://{address}");
        let bin = scratch.0.join("bin");
        let failed = download(&program("find"), &base, &base, &bin).unwrap_err();
        assert_eq!(
            failed,
            format!("{base}/repos/sharkdp/fd/releases/latest answered HTTP 403: rate limited.")
        );
        assert!(!bin.exists());
        server.join().unwrap();
    }
}
