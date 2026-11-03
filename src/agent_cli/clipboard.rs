//! The desktop's clipboard as Scramjet uses it. Copying: with the platform's tool (`pbcopy`;
//! `wl-copy` under Wayland, `xclip` or `xsel` under X11), else, and also in a remote session,
//! through the terminal with OSC 52. Pasting: with PowerShell under WSL, `pbpaste`, `wl-paste`,
//! `xclip` or `xsel`; never in a remote session, whose clipboard is the terminal's.

use std::{
    env,
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// How long a clipboard tool may take, as Scramjet's.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest OSC 52 payload sent, as Scramjet's: longer ones can upset a terminal.
const OSC52_MAX: usize = 100_000;
/// The most text a paste takes, as Scramjet's.
const PASTE_MAX: usize = 10 * 1024 * 1024;
/// Scramjet's PowerShell command that writes the Windows clipboard's text as UTF-8.
const POWERSHELL_READ: &str = "Add-Type -AssemblyName System.Windows.Forms; \
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); \
[Console]::Write([System.Windows.Forms.Clipboard]::GetText())";

/// Copies `text`. `Ok(Some(sequence))`: write `sequence` to the terminal, which copies it.
pub(super) fn copy(text: &str) -> Result<Option<String>, String> {
    let copied = by_tool(text);
    if copied && !remote() {
        return Ok(None);
    }
    match osc52(text) {
        Some(sequence) => Ok(Some(sequence)),
        None if copied => Ok(None),
        None => Err("Failed to copy to clipboard".to_owned()),
    }
}

/// The text on the clipboard, read with the first of the platform's tools that succeeds.
pub(super) fn paste() -> Result<String, String> {
    if remote() {
        return Err("Use your terminal's Paste command in remote sessions".to_owned());
    }
    (readers(set).into_iter())
        .find_map(|(program, arguments)| read(program, arguments))
        .ok_or_else(|| "Cannot read clipboard text; use your terminal's Paste command".to_owned())
}

fn remote() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"]
        .into_iter()
        .any(set)
}

fn set(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// The tools that read the clipboard, in Scramjet's order, given which variables are `set`.
/// Under WSL only Windows' clipboard is read, which WSLg's Wayland one need not follow.
fn readers(set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static [&'static str])> {
    if set("WSL_DISTRO_NAME") || set("WSL_INTEROP") {
        let arguments = &[
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-Command",
            POWERSHELL_READ,
        ];
        return vec![("powershell.exe", arguments)];
    }
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
        let read = (&mut stdout)
            .take(PASTE_MAX as u64 + 1)
            .read_to_end(&mut bytes);
        let _ = done.send(read.map(|_| bytes));
    });
    let deadline = Instant::now() + TIMEOUT;
    // Read apart, so that a tool that keeps writing cannot hold the paste past its time.
    let bytes = match output.recv_timeout(TIMEOUT) {
        Ok(Ok(bytes)) if bytes.len() <= PASTE_MAX => Some(bytes),
        _ => None,
    };
    let Some(bytes) = bytes else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    finished(&mut child, deadline).then(|| String::from_utf8_lossy(&bytes).into_owned())
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
        assert_eq!(
            names(&["WSL_INTEROP", "WAYLAND_DISPLAY", "DISPLAY"]),
            ["powershell.exe"]
        );
        assert_eq!(names(&["WSL_DISTRO_NAME"]), ["powershell.exe"]);
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
