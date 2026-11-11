//! The desktop's clipboard as Scramjet uses it. Copying: with `pbcopy` on macOS, else with a
//! PowerShell kept running on Windows and under WSL (`powershell.rs`) outside a remote session,
//! then `wl-copy` under Wayland, then the X server itself (`x11.rs`), `xclip` or `xsel`; else, and
//! also in a remote session, through the terminal with OSC 52. Pasting: with that PowerShell,
//! `pbpaste` on macOS, else from the X server itself, then with `wl-paste`, `xclip` or `xsel`;
//! never in a remote session, whose clipboard is the terminal's. When no route reads it, the first
//! failure names the key that pastes through the terminal itself.
//! An image is pasted as Scramjet pastes it, as the path of a file it is saved to; on macOS as PNG
//! through `osascript`, as Claude Code reads it.

use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{self, Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

mod powershell;
#[cfg(unix)]
mod x11;
/// No X server is looked for on Windows.
#[cfg(not(unix))]
mod x11 {
    pub(super) const SERVE: &str = "__x11-selection-owner";
    const NONE: &str = "no X11 clipboard on Windows";
    pub(super) fn copy(_text: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    pub(super) fn serve(_selection: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    pub(super) fn text(_max: usize) -> Result<Option<String>, String> {
        Err(NONE.into())
    }
    pub(super) fn image(_max: usize) -> Result<Option<(Vec<u8>, &'static str)>, String> {
        Err(NONE.into())
    }
}

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

/// Whether a failed paste has named the terminal's own paste key in this run.
static KEY_NAMED: AtomicBool = AtomicBool::new(false);

/// Runs the command line's `command` if it is the clipboard's own: scramjet-rs started again to
/// own a copy (`x11::copy`).
pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    match (command, arguments) {
        (x11::SERVE, [selection]) => Some(x11::serve(selection)),
        _ => None,
    }
}

/// Copies `text`. `Ok(Some(sequence))`: write `sequence` to the terminal, which copies it; never
/// without a `terminal` to write it to.
pub(super) fn copy(text: &str, terminal: bool) -> Result<Option<String>, String> {
    let copied = directly(text);
    if copied && !(terminal && remote()) {
        return Ok(None);
    }
    match osc52(text).filter(|_| terminal) {
        Some(sequence) => Ok(Some(sequence)),
        None if copied => Ok(None),
        None => Err("Failed to copy to clipboard".to_owned()),
    }
}

/// On Windows and under WSL, starts what reads and writes Windows' clipboard, for the first paste or copy not to
/// wait for it.
pub(super) fn warm() {
    if windows(set) && !remote() {
        powershell::warm();
    }
}

/// The text on the clipboard: on Windows and under WSL Windows', else read with the first of the platform's
/// tools that succeeds.
pub(super) fn paste() -> Result<String, String> {
    let failed = |why: String| unreadable(why, &KEY_NAMED);
    if remote() {
        return Err(failed(
            "Remote sessions paste only through the terminal".to_owned(),
        ));
    }
    if windows(set) {
        return match powershell::read("text", PASTE_MAX) {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) => Err(failed(format!("Cannot read clipboard text ({error})"))),
        };
    }
    let built_in = x11(set).then(|| x11::text(PASTE_MAX));
    if let Some(Ok(Some(text))) = built_in {
        return Ok(text);
    }
    let mut tools = readers(set).into_iter();
    if let Some(text) = tools.find_map(|(program, arguments)| read(program, arguments)) {
        return Ok(text);
    }
    Err(match built_in {
        Some(Ok(_)) => "No text on the clipboard".to_owned(),
        Some(Err(error)) => failed(format!("Cannot read clipboard text ({error})")),
        None => failed("Cannot read clipboard text".to_owned()),
    })
}

/// `why` no route read the clipboard's text, followed the first time (`named`) by the key that
/// pastes through the terminal itself.
fn unreadable(why: String, named: &AtomicBool) -> String {
    if named.swap(true, Ordering::Relaxed) {
        return why;
    }
    let var = |name: &str| env::var(name).unwrap_or_default();
    format!("{why}; {}", paste_key(var, remote()))
}

/// Which key pastes through the terminal itself, as the terminal the variables (`var`) name binds
/// it by default, else as most do; a `remote` session's terminal may be a Mac's.
fn paste_key(var: impl Fn(&str) -> String, remote: bool) -> &'static str {
    if var("LC_TERMINAL") == "iTerm2" || var("TERM_PROGRAM") == "iTerm.app" {
        "iTerm2 pastes with Command+V"
    } else if var("TERM_PROGRAM") == "Apple_Terminal" {
        "Apple's Terminal pastes with Command+V"
    } else if !var("WT_SESSION").is_empty() {
        "Windows Terminal pastes with Ctrl+Shift+V"
    } else if !var("XTERM_VERSION").is_empty() {
        "xterm pastes only selected text, with Shift+Insert"
    } else if cfg!(target_os = "macos") && !remote {
        "paste with Command+V"
    } else if remote {
        "most terminals paste with Ctrl+Shift+V, a Mac's with Command+V"
    } else {
        "most terminals paste with Ctrl+Shift+V"
    }
}

/// Saves the clipboard's image to a new file only the user can read, for its path to be pasted.
pub(super) fn paste_image() -> Result<String, String> {
    if remote() {
        return Err("Images cannot be pasted from the clipboard in remote sessions".to_owned());
    }
    if cfg!(target_os = "macos") {
        let script = MAC_IMAGE.iter().flat_map(|line| ["-e", line]);
        let arguments: Vec<&str> = script.collect();
        return saved_by("osascript", &arguments, &env::temp_dir().to_string_lossy())?
            .ok_or_else(|| "No image on the clipboard".to_owned());
    }
    let (bytes, extension) = image(set)?.ok_or("No image on the clipboard")?;
    save(&bytes, extension, &env::temp_dir().to_string_lossy())
}

/// The AppleScript that writes the clipboard's image as PNG, as Claude Code reads it, into the file
/// its argument names; it says `none` when the clipboard has no image.
const MAC_IMAGE: [&str; 11] = [
    "on run argv",
    "try",
    "set png to the clipboard as «class PNGf»",
    "on error",
    "return \"none\"",
    "end try",
    "set fp to open for access (POSIX file (item 1 of argv)) with write permission",
    "write png to fp",
    "close access fp",
    "return \"saved\"",
    "end run",
];

/// The path of a new PNG file in `directory` that `program`, given `arguments` and that path,
/// wrote the clipboard's image into, saying `saved`; `None` when it says `none`.
fn saved_by(program: &str, arguments: &[&str], directory: &str) -> Result<Option<String>, String> {
    let path = save(&[], "png", directory)?;
    let said = output(program, &[arguments, &[path.as_str()]].concat(), PASTE_MAX);
    let size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
    let said = said.map(|said| String::from_utf8_lossy(&said).trim().to_owned());
    let saved = match said.as_deref() {
        Some("saved") if size == 0 => Ok(None),
        Some("saved") if size > IMAGE_MAX as u64 => Err(format!(
            "The clipboard's image is over {} MB",
            IMAGE_MAX >> 20
        )),
        Some("saved") => return Ok(Some(path)),
        Some("none") => Ok(None),
        _ => Err(format!(
            "Cannot read the clipboard's image ({program} failed)"
        )),
    };
    let _ = fs::remove_file(&path);
    saved
}

/// The clipboard's image, if any, and its extension, given which variables are `set`: under WSL
/// (and on Windows) Windows' as PNG, as for text; else from the X server, then with `wl-paste` under Wayland, then
/// `xclip` under X11, in a type offered among `IMAGE_TYPES`.
fn image(set: impl Fn(&str) -> bool) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    let nonempty = |read: Option<(Vec<u8>, _)>| read.filter(|(bytes, _)| !bytes.is_empty());
    if windows(&set) {
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
    let xclip = || {
        let (kind, extension) =
            listed("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"])?;
        let bytes = output(
            "xclip",
            &["-selection", "clipboard", "-t", kind, "-o"],
            IMAGE_MAX,
        )?;
        Some((bytes, extension))
    };
    let built_in = x11(&set).then(|| x11::image(IMAGE_MAX).map(nonempty));
    if let Some(Ok(Some(image))) = built_in {
        return Ok(Some(image));
    }
    let tools = nonempty(set("WAYLAND_DISPLAY").then(wayland).flatten())
        .or_else(|| nonempty(set("DISPLAY").then(xclip).flatten()));
    match (tools, built_in) {
        (None, Some(Err(error))) => Err(format!("Cannot read the clipboard's image ({error})")),
        (tools, _) => Ok(tools),
    }
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

/// Writes `bytes` to a new `scramjet-clipboard-…` file in `directory`; never to one that
/// exists, which in a shared `/tmp` may be another user's.
fn save(bytes: &[u8], extension: &str, directory: &str) -> Result<String, String> {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let name = format!("scramjet-clipboard-{}-{stamp:x}.{extension}", process::id());
    let path = Path::new(directory)
        .join(name)
        .to_string_lossy()
        .into_owned();
    let failed = |error| format!("Cannot save the image to {path}: {error}");
    let mut file = scramjet_rs::os::private_file(OpenOptions::new().write(true).create_new(true))
        .open(&path)
        .map_err(failed)?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(&path);
        return Err(failed(error));
    }
    Ok(path)
}

pub(super) fn remote() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"]
        .into_iter()
        .any(set)
}

fn set(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Whether the clipboard is Windows', used through PowerShell: on Windows, and under WSL.
fn windows(set: impl Fn(&str) -> bool) -> bool {
    cfg!(windows) || set("WSL_DISTRO_NAME") || set("WSL_INTEROP")
}

/// Whether the clipboard is read from an X server: on Linux, outside WSL, with a `DISPLAY`.
fn x11(set: impl Fn(&str) -> bool) -> bool {
    !cfg!(target_os = "macos") && !windows(&set) && set("DISPLAY")
}

/// The tools that read the clipboard outside Windows and WSL, in Scramjet's order, given which
/// variables are `set`.
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

/// Copies `text` to the desktop's clipboard, not through the terminal; whether it did.
fn directly(text: &str) -> bool {
    if cfg!(target_os = "macos") {
        return give("pbcopy", &[], text);
    }
    (windows(set) && !remote() && powershell::write(text).is_ok())
        || (set("WAYLAND_DISPLAY") && give("wl-copy", &[], text))
        || x11(set) && x11::copy(text).is_ok()
        || set("DISPLAY")
            && (give("xclip", &["-selection", "clipboard"], text)
                || give("xsel", &["--clipboard", "--input"], text))
}

/// Gives `text` to `program`; whether it took it and succeeded within the time allowed.
fn give(program: &str, arguments: &[&str], text: &str) -> bool {
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

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_clipboard_is_read_with_the_platforms_tools_in_scramjets_order() {
        let names = |variables: &[&str]| -> Vec<&str> {
            let set = |name: &str| variables.contains(&name);
            readers(set)
                .into_iter()
                .map(|(program, _)| program)
                .collect()
        };
        let windows = |variables: &[&str]| windows(|name: &str| variables.contains(&name));
        assert!(windows(&["WSL_INTEROP", "WAYLAND_DISPLAY", "DISPLAY"]));
        assert!(windows(&["WSL_DISTRO_NAME"]));
        assert_eq!(windows(&["WAYLAND_DISPLAY", "DISPLAY"]), cfg!(windows));
        if cfg!(any(target_os = "macos", windows)) {
            return;
        }
        let x11 = |variables: &[&str]| x11(|name: &str| variables.contains(&name));
        assert!(x11(&["DISPLAY"]) && x11(&["WAYLAND_DISPLAY", "DISPLAY"]));
        assert!(!x11(&["WAYLAND_DISPLAY"]) && !x11(&["WSL_INTEROP", "DISPLAY"]));
        assert_eq!(
            names(&["WAYLAND_DISPLAY", "DISPLAY"]),
            ["wl-paste", "xclip", "xsel"]
        );
        assert_eq!(names(&["DISPLAY"]), ["xclip", "xsel"]);
        assert!(names(&[]).is_empty());
    }

    #[test]
    fn the_first_unreadable_paste_names_the_terminals_own_paste_key() {
        let key = |variables: &[(&str, &str)], remote: bool| {
            let var = |name: &str| {
                let value = variables.iter().find(|(set, _)| *set == name);
                value.map_or_else(String::new, |(_, value)| (*value).to_owned())
            };
            paste_key(var, remote)
        };
        let windows = [("WT_SESSION", "0b1c"), ("TERM_PROGRAM", "tmux")];
        assert_eq!(
            key(&windows, false),
            "Windows Terminal pastes with Ctrl+Shift+V"
        );
        assert_eq!(
            key(&[("LC_TERMINAL", "iTerm2")], true),
            "iTerm2 pastes with Command+V"
        );
        assert_eq!(
            key(&[("TERM_PROGRAM", "Apple_Terminal")], false),
            "Apple's Terminal pastes with Command+V"
        );
        assert_eq!(
            key(&[("XTERM_VERSION", "XTerm(390)")], false),
            "xterm pastes only selected text, with Shift+Insert"
        );
        assert_eq!(
            key(&[], true),
            "most terminals paste with Ctrl+Shift+V, a Mac's with Command+V"
        );
        let local = match cfg!(target_os = "macos") {
            true => "paste with Command+V",
            false => "most terminals paste with Ctrl+Shift+V",
        };
        assert_eq!(key(&[("TERM", "xterm-256color")], false), local);

        let named = AtomicBool::new(false);
        let first = unreadable("Cannot read clipboard text".to_owned(), &named);
        let (why, hint) = first.split_once("; ").unwrap();
        assert_eq!(why, "Cannot read clipboard text");
        assert!(hint.contains(" with "), "{hint}");
        let later = unreadable("Cannot read clipboard text (ended)".to_owned(), &named);
        assert_eq!(later, "Cannot read clipboard text (ended)");
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
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
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
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
        let directory = env::temp_dir().join(format!("scramjet-clipboard-test-{}", process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let _scratch = Scratch(directory.clone());
        let directory = directory.to_string_lossy().into_owned();
        let first = save(
            b"one",
            "png",
            &format!("{directory}{}", std::path::MAIN_SEPARATOR),
        )
        .unwrap();
        let second = save(b"two", "jpg", &directory).unwrap();
        let read = |path: &str| std::fs::read(path).unwrap();
        assert_eq!(
            (read(&first), read(&second)),
            (b"one".to_vec(), b"two".to_vec())
        );
        let named = Path::new(&directory).join("scramjet-clipboard-");
        assert!(first.starts_with(&*named.to_string_lossy()), "{first}");
        assert!(first.ends_with(".png") && second.ends_with(".jpg"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(
            save(b"three", "png", &directory)
                .unwrap_err()
                .starts_with("Cannot save the image to ")
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn a_macs_image_is_kept_only_when_the_script_saved_one() {
        let directory = env::temp_dir().join(format!("scramjet-clipboard-mac-{}", process::id()));
        fs::create_dir_all(&directory).unwrap();
        let _scratch = Scratch(directory.clone());
        let directory = directory.to_string_lossy().into_owned();
        let run = |script: &str| saved_by("sh", &["-c", script, "sh"], &directory);
        let path = run("printf '\\211PNG' > \"$1\"; echo saved")
            .unwrap()
            .unwrap();
        assert!(path.ends_with(".png"), "{path}");
        assert_eq!(fs::read(&path).unwrap(), b"\x89PNG");
        fs::remove_file(&path).unwrap();
        assert_eq!(run("echo none"), Ok(None));
        assert_eq!(run("echo saved"), Ok(None));
        assert_eq!(
            run("printf x > \"$1\"; exit 1"),
            Err("Cannot read the clipboard's image (sh failed)".to_owned())
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
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
