//! Copying text to the desktop's clipboard as Scramjet does: with the platform's tool (`pbcopy`;
//! `wl-copy` under Wayland, `xclip` or `xsel` under X11), else, and also in a remote session,
//! through the terminal with OSC 52.

use std::{
    env,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How long a clipboard tool may take, as Scramjet's.
const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest OSC 52 payload sent, as Scramjet's: longer ones can upset a terminal.
const OSC52_MAX: usize = 100_000;

/// Copies `text`. `Ok(Some(sequence))`: write `sequence` to the terminal, which copies it.
pub(super) fn copy(text: &str) -> Result<Option<String>, String> {
    let remote = ["SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"]
        .into_iter()
        .any(set);
    let copied = by_tool(text);
    if copied && !remote {
        return Ok(None);
    }
    match osc52(text) {
        Some(sequence) => Ok(Some(sequence)),
        None if copied => Ok(None),
        None => Err("Failed to copy to clipboard".to_owned()),
    }
}

fn set(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
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
    let start = Instant::now();
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let written = writer.is_some_and(|writer| writer.join().unwrap_or(false));
    succeeded && written
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
