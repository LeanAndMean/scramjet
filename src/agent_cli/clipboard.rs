//! The desktop's clipboard as Scramjet uses it. Copying: with the platform's tool (`pbcopy`;
//! `wl-copy` under Wayland, `xclip` or `xsel` under X11), else, and also in a remote session,
//! through the terminal with OSC 52. Pasting: with a PowerShell kept running under WSL
//! (`powershell.rs`), `pbpaste`, `wl-paste`, `xclip` or `xsel`; never in a remote session, whose
//! clipboard is the terminal's. An image is pasted as Scramjet pastes it, as the path of a file
//! it is saved to.

use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    process::{self, Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

mod powershell;

/// How long a clipboard tool may take, as Scramjet's.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest OSC 52 payload sent, as Scramjet's: longer ones can upset a terminal.
const OSC52_MAX: usize = 100_000;
/// The most text a paste takes, as Scramjet's.
const PASTE_MAX: usize = 10 * 1024 * 1024;
/// The most image bytes a paste takes, as Scramjet's.
const IMAGE_MAX: usize = 50 * 1024 * 1024;
/// The image types taken, in Scramjet's order of preference, with the extension each is saved
/// with: those `read` sends to the model.
const IMAGE_TYPES: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
];

/// Copies `text`. `Ok(Some(sequence))`: write `sequence` to the terminal, which copies it; never
/// without a `terminal` to write it to.
pub(super) fn copy(text: &str, terminal: bool) -> Result<Option<String>, String> {
    let copied = by_tool(text);
    if copied && !(terminal && remote()) {
        return Ok(None);
    }
    match osc52(text).filter(|_| terminal) {
        Some(sequence) => Ok(Some(sequence)),
        None if copied => Ok(None),
        None => Err("Failed to copy to clipboard".to_owned()),
    }
}

/// Under WSL, starts what reads Windows' clipboard, for the first paste not to wait for it.
pub(super) fn warm() {
    if wsl(set) && !remote() {
        powershell::warm();
    }
}

/// The text on the clipboard: under WSL Windows', else read with the first of the platform's
/// tools that succeeds.
pub(super) fn paste() -> Result<String, String> {
    if remote() {
        return Err("Use your terminal's Paste command in remote sessions".to_owned());
    }
    if wsl(set) {
        return match powershell::read("text", PASTE_MAX) {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) => Err(format!(
                "Cannot read clipboard text ({error}); use your terminal's Paste command"
            )),
        };
    }
    (readers(set).into_iter())
        .find_map(|(program, arguments)| read(program, arguments))
        .ok_or_else(|| "Cannot read clipboard text; use your terminal's Paste command".to_owned())
}

/// Saves the clipboard's image to a new file only the user can read, for its path to be pasted.
pub(super) fn paste_image() -> Result<String, String> {
    if remote() {
        return Err("Images cannot be pasted from the clipboard in remote sessions".to_owned());
    }
    if cfg!(target_os = "macos") {
        return Err("Pasting images is not supported on macOS yet".to_owned());
    }
    let (bytes, extension) = image(set)?.ok_or("No image on the clipboard")?;
    save(&bytes, extension, &env::temp_dir().to_string_lossy())
}

/// The clipboard's image, if any, and its extension, given which variables are `set`: under WSL
/// Windows' as PNG, as for text; else with `wl-paste` under Wayland, then `xclip` under X11, in a
/// type the tool lists among `IMAGE_TYPES`.
fn image(set: impl Fn(&str) -> bool) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    let nonempty = |read: Option<(Vec<u8>, _)>| read.filter(|(bytes, _)| !bytes.is_empty());
    if wsl(&set) {
        return match powershell::read("image", IMAGE_MAX) {
            Ok(png) => Ok(nonempty(Some((png, "png")))),
            Err(error) => Err(format!("Cannot read the clipboard's image ({error})")),
        };
    }
    let listed = |program: &str, arguments: &[&str]| {
        preferred(&String::from_utf8_lossy(&output(
            program, arguments, PASTE_MAX,
        )?))
    };
    let wayland = || {
        let (kind, extension) = listed("wl-paste", &["--list-types"])?;
        let bytes = output("wl-paste", &["--type", kind, "--no-newline"], IMAGE_MAX)?;
        Some((bytes, extension))
    };
    let x11 = || {
        let (kind, extension) =
            listed("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])?;
        let bytes = output(
            "xclip",
            &["-selection", "clipboard", "-t", kind, "-o"],
            IMAGE_MAX,
        )?;
        Some((bytes, extension))
    };
    Ok(nonempty(set("WAYLAND_DISPLAY").then(wayland).flatten())
        .or_else(|| nonempty(set("DISPLAY").then(x11).flatten())))
}

/// The first of `IMAGE_TYPES` among the types a tool lists, one a line, ignoring parameters such
/// as `;charset=`.
fn preferred(listed: &str) -> Option<(&'static str, &'static str)> {
    let base = |line: &str| line.split(';').next().unwrap_or_default().trim().to_owned();
    let listed: Vec<String> = listed.lines().map(base).collect();
    IMAGE_TYPES
        .into_iter()
        .find(|(kind, _)| listed.iter().any(|line| line.eq_ignore_ascii_case(kind)))
}

/// Writes `bytes` to a new `scramjet-rs-clipboard-…` file in `directory`; never to one that
/// exists, which in a shared `/tmp` may be another user's.
fn save(bytes: &[u8], extension: &str, directory: &str) -> Result<String, String> {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let path = format!(
        "{}/scramjet-rs-clipboard-{}-{stamp:x}.{extension}",
        directory.trim_end_matches('/'),
        process::id()
    );
    let failed = |error| format!("Cannot save the image to {path}: {error}");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(failed)?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(&path);
        return Err(failed(error));
    }
    Ok(path)
}

fn remote() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"]
        .into_iter()
        .any(set)
}

fn set(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn wsl(set: impl Fn(&str) -> bool) -> bool {
    set("WSL_DISTRO_NAME") || set("WSL_INTEROP")
}

/// The tools that read the clipboard outside WSL, in Scramjet's order, given which variables are
/// `set`.
fn readers(set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        return vec![("pbpaste", &[])];
    }
    let mut readers: Vec<(&str, &[&str])> = Vec::new();
    if set("WAYLAND_DISPLAY") {
        readers.push(("wl-paste", &["--no-newline", "--type", "text"]));
    }
    if set("DISPLAY") {
        readers.push(("xclip", &["-selection", "clipboard", "-o"]));
        readers.push(("xsel", &["--clipboard", "--output"]));
    }
    readers
}

/// What `program` writes, if it succeeds within the time allowed and writes at most `PASTE_MAX`
/// bytes.
fn read(program: &str, arguments: &[&str]) -> Option<String> {
    output(program, arguments, PASTE_MAX).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// What `program` writes, if it succeeds within the time allowed and writes at most `max` bytes.
fn output(program: &str, arguments: &[&str], max: usize) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (done, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = (&mut stdout).take(max as u64 + 1).read_to_end(&mut bytes);
        let _ = done.send(read.map(|_| bytes));
    });
    let deadline = Instant::now() + TIMEOUT;
    // Read apart, so that a tool that keeps writing cannot hold the paste past its time.
    let bytes = match output.recv_timeout(TIMEOUT) {
        Ok(Ok(bytes)) if bytes.len() <= max => Some(bytes),
        _ => None,
    };
    let Some(bytes) = bytes else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    finished(&mut child, deadline).then_some(bytes)
}

fn by_tool(text: &str) -> bool {
    if cfg!(target_os = "macos") {
        return run("pbcopy", &[], text);
    }
    (set("WAYLAND_DISPLAY") && run("wl-copy", &[], text))
        || set("DISPLAY")
            && (run("xclip", &["-selection", "clipboard"], text)
                || run("xsel", &["--clipboard", "--input"], text))
}

/// Gives `text` to `program`; whether it took it and succeeded within the time allowed.
fn run(program: &str, arguments: &[&str], text: &str) -> bool {
    let child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    // Written apart, so that a tool that stops reading cannot hold the copy past its time.
    let writer = child.stdin.take().map(|mut stdin| {
        let text = text.to_owned();
        std::thread::spawn(move || stdin.write_all(text.as_bytes()).is_ok())
    });
    let succeeded = finished(&mut child, Instant::now() + TIMEOUT);
    let written = writer.is_some_and(|writer| writer.join().unwrap_or(false));
    succeeded && written
}

/// Whether `child` exits successfully before `deadline`; past it, it is killed.
fn finished(child: &mut Child, deadline: Instant) -> bool {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The OSC 52 sequence that copies `text`, unless it is too long to send.
fn osc52(text: &str) -> Option<String> {
    let encoded = base64(text.as_bytes());
    (encoded.len() <= OSC52_MAX).then(|| format!("\x1b]52;c;{encoded}\x07"))
}

fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let byte = |index: usize| u32::from(chunk.get(index).copied().unwrap_or(0));
        let bits = byte(0) << 16 | byte(1) << 8 | byte(2);
        for index in 0..4 {
            encoded.push(match index <= chunk.len() {
                true => char::from(DIGITS[(bits >> (18 - 6 * index) & 63) as usize]),
                false => '=',
            });
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clipboard_is_read_with_the_platforms_tools_in_scramjets_order() {
        let names = |variables: &[&str]| -> Vec<&str> {
            let set = |name: &str| variables.contains(&name);
            readers(set)
                .into_iter()
                .map(|(program, _)| program)
                .collect()
        };
        let wsl = |variables: &[&str]| wsl(|name: &str| variables.contains(&name));
        assert!(wsl(&["WSL_INTEROP", "WAYLAND_DISPLAY", "DISPLAY"]) && wsl(&["WSL_DISTRO_NAME"]));
        assert!(!wsl(&["WAYLAND_DISPLAY", "DISPLAY"]));
        if cfg!(target_os = "macos") {
            return;
        }
        assert_eq!(
            names(&["WAYLAND_DISPLAY", "DISPLAY"]),
            ["wl-paste", "xclip", "xsel"]
        );
        assert_eq!(names(&["DISPLAY"]), ["xclip", "xsel"]);
        assert!(names(&[]).is_empty());
    }

    #[test]
    fn a_paste_takes_what_the_tool_writes_when_it_succeeds() {
        let sh = |script: &str| read("sh", &["-c", script]);
        assert_eq!(
            sh("printf 'one\\r\\ntwo \\342\\200\\224'").as_deref(),
            Some("one\r\ntwo \u{2014}")
        );
        assert_eq!(sh("printf partial; exit 1"), None);
        assert_eq!(sh("head -c 10485761 /dev/zero"), None);
        assert_eq!(read("scramjet-rs-no-such-tool", &[]), None);
    }

    #[test]
    fn an_image_is_taken_in_the_first_type_read_sends_that_the_tool_lists() {
        let listed = "TARGETS\ntext/plain\nimage/bmp\nimage/gif\nIMAGE/JPEG; q=1\n";
        assert_eq!(preferred(listed), Some(("image/jpeg", "jpg")));
        assert_eq!(
            preferred("image/png\nimage/webp"),
            Some(("image/png", "png"))
        );
        assert_eq!(preferred("text/plain\nimage/bmp\nimage/tiff"), None);
        let png = output("sh", &["-c", "printf '\\211PNG\\r\\n\\000'"], IMAGE_MAX);
        assert_eq!(png.as_deref(), Some(&b"\x89PNG\r\n\0"[..]));
    }

    #[test]
    fn a_pasted_image_is_saved_to_a_new_file_only_the_user_reads() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            env::temp_dir().join(format!("scramjet-rs-clipboard-test-{}", process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let directory = directory.to_string_lossy().into_owned();
        let first = save(b"one", "png", &format!("{directory}/")).unwrap();
        let second = save(b"two", "jpg", &directory).unwrap();
        let read = |path: &str| std::fs::read(path).unwrap();
        assert_eq!(
            (read(&first), read(&second)),
            (b"one".to_vec(), b"two".to_vec())
        );
        assert!(first.starts_with(&format!("{directory}/scramjet-rs-clipboard-")));
        assert!(first.ends_with(".png") && second.ends_with(".jpg"));
        let mode = std::fs::metadata(&first).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(
            save(b"three", "png", &directory)
                .unwrap_err()
                .starts_with("Cannot save the image to ")
        );
    }

    #[test]
    fn osc52_carries_the_text_in_base64_up_to_scramjets_limit() {
        let encoded =
            ["", "f", "fo", "foo", "foob", "fooba", "foobar"].map(|text| base64(text.as_bytes()));
        assert_eq!(
            encoded,
            [
                "", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm9vYmFy"
            ]
        );
        assert_eq!(base64("界\n".as_bytes()), "55WMCg==");
        assert_eq!(osc52("hi").as_deref(), Some("\x1b]52;c;aGk=\x07"));
        assert!(osc52(&"a".repeat(75_000)).is_some());
        assert!(osc52(&"a".repeat(75_003)).is_none());
    }
}
