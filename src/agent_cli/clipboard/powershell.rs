//! Windows' clipboard, on Windows and under WSL, read and written by one PowerShell process kept
//! for the next paste or copy, as Scramjet's reader is: starting PowerShell takes about 0.6 s, a
//! read in it about 80 ms, a copy 30-200 ms. Copying here rather than with OSC 52 also works
//! inside tmux, whose default `set-clipboard external` drops a program's OSC 52, and for any
//! length. Scramjet reads it so on Windows too, and copies there with `clip`.
//! Its script, a plain `-Command`, only calls `System.Windows.Forms.Clipboard`: no code compiled
//! with `Add-Type` (whose DLLs in Windows' `%TEMP%` alarmed endpoint security; Scramjet's #642)
//! and no `-EncodedCommand`. WSLg's own X11 clipboard answers in 2 ms but can stop following
//! Windows' after hours (microsoft/wslg#1202), which would paste old text without an error.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, PoisonError, mpsc},
    time::Duration,
};

/// Answers each `text` or `image` line with the clipboard's text as UTF-8 or its image as PNG
/// (none: nothing), sent as a line with the number of bytes and then the bytes, and each
/// `copy BASE64` line, once the UTF-8 text it carries is on the clipboard, with nothing; `-1` when
/// the clipboard cannot be used, as when another program holds it. Its first answer, empty, says
/// that it is ready. `SetText` leaves the text there after PowerShell ends.
const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
Add-Type -AssemblyName System.Drawing; \
$out = [Console]::OpenStandardOutput(); \
function Send([byte[]] $bytes) { \
$head = [Text.Encoding]::ASCII.GetBytes([string]$bytes.Length + [char]10); \
$out.Write($head, 0, $head.Length); $out.Write($bytes, 0, $bytes.Length); $out.Flush() }; \
Send @(); \
while ($null -ne ($request = [Console]::In.ReadLine())) { try { \
if ($request -eq 'text') { Send ([Text.Encoding]::UTF8.GetBytes([System.Windows.Forms.Clipboard]::GetText())) } \
elseif ($request -eq 'image') { $image = [System.Windows.Forms.Clipboard]::GetImage(); \
if ($image) { $png = New-Object IO.MemoryStream; \
$image.Save($png, [System.Drawing.Imaging.ImageFormat]::Png); $image.Dispose(); Send ($png.ToArray()) } \
else { Send @() } } \
elseif ($request.StartsWith('copy ')) { \
[System.Windows.Forms.Clipboard]::SetText([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($request.Substring(5)))); \
Send @() } \
else { break } \
} catch { $failed = [Text.Encoding]::ASCII.GetBytes('-1' + [char]10); \
$out.Write($failed, 0, $failed.Length); $out.Flush() } }";

static READER: Mutex<Option<Reader>> = Mutex::new(None);

/// Starts the reader, so that the first paste need not wait for PowerShell.
pub(super) fn warm() {
    let mut reader = READER.lock().unwrap_or_else(PoisonError::into_inner);
    if reader.is_none() {
        *reader = start().ok();
    }
}

/// The clipboard's text or image, as `request` (`text` or `image`) asks, if it takes at most
/// `max` bytes.
pub(super) fn read(request: &str, max: usize) -> Result<Vec<u8>, String> {
    let mut reader = READER.lock().unwrap_or_else(PoisonError::into_inner);
    let bytes = read_with(&mut reader, start, request, super::TIMEOUT)?;
    match bytes.len() <= max {
        true => Ok(bytes),
        false => Err("the clipboard holds too much".to_owned()),
    }
}

/// Puts `text` on the clipboard.
pub(super) fn write(text: &str) -> Result<(), String> {
    let request = format!("copy {}", super::base64(text.as_bytes()));
    let mut reader = READER.lock().unwrap_or_else(PoisonError::into_inner);
    read_with(&mut reader, start, &request, super::TIMEOUT).map(drop)
}

fn start() -> io::Result<Reader> {
    let arguments = [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-Command",
        SCRIPT,
    ];
    Reader::start("powershell.exe", &arguments, super::IMAGE_MAX)
}

/// Asks `slot`'s reader, started with `start` when there is none; a reader found stopped, as
/// one ended since the last paste, is started again once.
fn read_with(
    slot: &mut Option<Reader>,
    start: impl Fn() -> io::Result<Reader>,
    request: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    loop {
        let started = slot.is_none();
        let reader = match slot {
            Some(reader) => reader,
            None => {
                slot.insert(start().map_err(|error| format!("cannot start PowerShell: {error}"))?)
            }
        };
        let failure = match reader.ask(request, timeout) {
            Ok(bytes) => return Ok(bytes),
            Err(failure) => failure,
        };
        if failure != Failure::Failed {
            *slot = None;
        }
        if started || !matches!(failure, Failure::Stopped(_)) {
            return Err(failure.to_string());
        }
    }
}

/// A running reader.
struct Reader {
    child: Child,
    stdin: ChildStdin,
    answers: mpsc::Receiver<Answer>,
    ready: bool,
}

/// An answer read: the bytes, `None` for `-1`, or why the reader cannot go on.
type Answer = Result<Option<Vec<u8>>, String>;

#[derive(Debug, PartialEq)]
enum Failure {
    /// The clipboard could not be read this time.
    Failed,
    TimedOut,
    Stopped(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Failure::Failed => formatter.write_str("the clipboard could not be read"),
            Failure::TimedOut => formatter.write_str("PowerShell did not answer in time"),
            Failure::Stopped(error) => write!(formatter, "PowerShell stopped: {error}"),
        }
    }
}

impl Reader {
    /// Starts `program` in a process group of its own, out of reach of the terminal's Ctrl-C
    /// while an external editor or `!` command has it; it ends when its input closes, as when
    /// scramjet-rs exits.
    fn start(program: &str, arguments: &[&str], max: usize) -> io::Result<Self> {
        let mut child = scramjet_rs::os::own_group(&mut Command::new(program))
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("no pipes"));
        };
        let (send, answers) = mpsc::channel();
        let reading = std::thread::Builder::new()
            .name("clipboard reader".into())
            .spawn(move || read_answers(stdout, max, &send));
        if let Err(error) = reading {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Self {
            child,
            stdin,
            answers,
            ready: false,
        })
    }

    fn ask(&mut self, request: &str, timeout: Duration) -> Result<Vec<u8>, Failure> {
        if !self.ready {
            match self.answer(timeout)? {
                Some(bytes) if bytes.is_empty() => self.ready = true,
                _ => return Err(Failure::Stopped("no greeting".to_owned())),
            }
        }
        writeln!(self.stdin, "{request}").map_err(|error| Failure::Stopped(error.to_string()))?;
        self.answer(timeout)?.ok_or(Failure::Failed)
    }

    fn answer(&self, timeout: Duration) -> Result<Option<Vec<u8>>, Failure> {
        match self.answers.recv_timeout(timeout) {
            Ok(answer) => answer.map_err(Failure::Stopped),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Failure::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Failure::Stopped("ended".to_owned())),
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads the answers to `send` until the reader ends or writes what it should not, such as an
/// answer of more than `max` bytes.
fn read_answers(stdout: ChildStdout, max: usize, send: &mpsc::Sender<Answer>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let mut head = Vec::new();
        let answer = match (&mut stdout).take(24).read_until(b'\n', &mut head) {
            Ok(0) => Err("ended".to_owned()),
            Ok(_) => match std::str::from_utf8(&head).map(str::trim_end) {
                Ok("-1") => Ok(None),
                Ok(length) => match length.parse::<usize>() {
                    Ok(length) if length <= max => {
                        let mut bytes = vec![0; length];
                        (stdout.read_exact(&mut bytes).map(|()| Some(bytes)))
                            .map_err(|error| error.to_string())
                    }
                    Ok(_) => Err("the clipboard holds too much".to_owned()),
                    Err(_) => Err(format!(
                        "unexpected output {:?}",
                        String::from_utf8_lossy(&head)
                    )),
                },
                Err(_) => Err("unexpected output".to_owned()),
            },
            Err(error) => Err(error.to_string()),
        };
        let stop = answer.is_err();
        if send.send(answer).is_err() || stop {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader answering as the script does: `hello` as text, bytes with a NUL and line breaks
    /// as the image, `-1` to `fail`; `stop` ends it, and `wait` answers nothing.
    fn scripted() -> io::Result<Reader> {
        let script = "printf '0\\n'; while read -r request; do case $request in \
            text) printf '5\\nhello' ;; image) printf '5\\n\\000\\n\\211P\\r' ;; \
            fail) printf -- '-1\\n' ;; big) printf '11\\n' ;; wait) exec sleep 5 ;; *) exit ;; \
            esac; done";
        Reader::start("sh", &["-c", script], 10)
    }

    const QUICK: Duration = Duration::from_millis(300);

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn a_reader_answers_each_request_with_the_bytes_it_sent() {
        let mut reader = scripted().unwrap();
        assert_eq!(reader.ask("text", QUICK).unwrap(), b"hello");
        assert_eq!(reader.ask("image", QUICK).unwrap(), b"\0\n\x89P\r");
        assert_eq!(reader.ask("fail", QUICK), Err(Failure::Failed));
        assert_eq!(reader.ask("text", QUICK).unwrap(), b"hello");
        assert_eq!(
            reader.ask("big", QUICK),
            Err(Failure::Stopped("the clipboard holds too much".to_owned()))
        );
        let mut reader = scripted().unwrap();
        assert_eq!(reader.ask("wait", QUICK), Err(Failure::TimedOut));
        let mut reader = scripted().unwrap();
        assert!(matches!(
            reader.ask("stop", QUICK),
            Err(Failure::Stopped(_))
        ));
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn a_reader_that_stopped_is_started_again_once() {
        let mut slot = Some(scripted().unwrap());
        assert!(slot.as_mut().unwrap().ask("stop", QUICK).is_err());
        assert_eq!(
            read_with(&mut slot, scripted, "text", QUICK).unwrap(),
            b"hello"
        );
        assert!(slot.is_some());
        let mut slot = None;
        let broken = || Reader::start("sh", &["-c", "printf 'no\\n'"], 10);
        let error = read_with(&mut slot, broken, "text", QUICK).unwrap_err();
        assert!(
            error.starts_with("PowerShell stopped: unexpected output"),
            "{error}"
        );
        assert!(slot.is_none());
        let missing = || Reader::start("scramjet-rs-no-such-tool", &[], 10);
        let error = read_with(&mut slot, missing, "text", QUICK).unwrap_err();
        assert!(error.starts_with("cannot start PowerShell: "), "{error}");
        let error = read_with(&mut slot, scripted, "fail", QUICK).unwrap_err();
        assert_eq!(error, "the clipboard could not be read");
        assert!(slot.is_some());
        let error = read_with(&mut slot, scripted, "wait", QUICK).unwrap_err();
        assert_eq!(error, "PowerShell did not answer in time");
        assert!(slot.is_none());
    }
}
