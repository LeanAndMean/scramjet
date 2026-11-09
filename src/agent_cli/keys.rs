//! Keys read from the terminal in raw mode: escape sequences, SGR mouse reports, bracketed
//! pastes and the terminal's answers decoded into `Key`s, the keys pressed with their modifiers;
//! what each does is the keymap's (`keymap.rs`).

use super::keymap::{self, Binding};
use super::palette::Rgb;
use super::{background, clipboard, console};
use std::{
    io::{self, Write},
    time::Duration,
};

/// How long the rest of an escape sequence may take to arrive before Escape counts as a lone key.
pub(super) const ESCAPE_WAIT: Duration = Duration::from_millis(30);

#[derive(Debug, PartialEq)]
pub(super) enum Key {
    /// A character typed, never a control character.
    Text(String),
    Paste(String),
    /// Any other key, with the modifiers held; what it does is the keymap's (`Key::is`).
    Press(Press),
    /// A mouse report, which only the full screen asks the terminal for.
    Mouse(Mouse),
    /// One of the terminal's colors, as it answered the screen's question about it.
    Answer(background::Asked, Rgb),
    Idle,
    Ignored,
}

impl Key {
    /// The key as a press: a character typed as its key, an upper-case letter as Shift and the
    /// letter, so that `shift+l` is `L`, as in Scramjet.
    pub(super) fn press(&self) -> Option<Press> {
        match self {
            Key::Press(press) => Some(*press),
            Key::Text(text) => {
                let mut chars = text.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return None;
                };
                Some(match c.is_ascii_uppercase() {
                    true => Press::new(Code::Char(c.to_ascii_lowercase()), SHIFT),
                    false => Press::new(Code::Char(c), 0),
                })
            }
            _ => None,
        }
    }

    /// Whether the key is one of `binding`'s.
    pub(super) fn is(&self, binding: Binding) -> bool {
        self.press()
            .is_some_and(|press| keymap::bound(binding, press))
    }
}

/// A key on the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Code {
    /// A printable character's key: a letter in lower case, `' '` for Space.
    Char(char),
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
}

/// A key pressed with the modifiers held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Press {
    pub(super) code: Code,
    /// `SHIFT`, `ALT` and `CTRL`: the bits of xterm's modifier parameter, less one.
    pub(super) modifiers: u8,
}

pub(super) const SHIFT: u8 = 1;
pub(super) const ALT: u8 = 2;
pub(super) const CTRL: u8 = 4;

impl Press {
    pub(super) const fn new(code: Code, modifiers: u8) -> Self {
        Self { code, modifiers }
    }
}

/// An SGR mouse report (`ESC [ < button ; column ; row M`, `m` on release).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mouse {
    /// The button and modifier bits: 0 left, 2 right, 32 a move with the left button held, 64
    /// and 65 the wheel up and down.
    pub(super) button: u16,
    /// Where, from 0.
    pub(super) column: usize,
    pub(super) row: usize,
    pub(super) release: bool,
}

pub(super) trait ByteSource {
    /// The next input byte; `None` when `wait` elapses first. `wait: None` blocks.
    fn byte(&mut self, wait: Option<Duration>) -> io::Result<Option<u8>>;
}

/// Whether a lone `0x08` is Ctrl+Backspace, which other terminals send for Backspace: in Windows
/// Terminal, and in Windows' own consoles, which send Backspace as `0x7f` (conhost and Windows
/// Terminal 1.24's OpenConsole, 2026-11-09) and leave `WT_SESSION` unset when Windows Terminal
/// takes over a console started elsewhere. Not over ssh, where the terminal is another's.
pub(super) fn windows_terminal() -> bool {
    (cfg!(windows) || std::env::var_os("WT_SESSION").is_some_and(|value| !value.is_empty()))
        && !clipboard::remote()
}

/// The next key; `windows_terminal` reads a lone `0x08` as Ctrl+Backspace (`windows_terminal()`).
pub(super) fn read_key(
    source: &mut impl ByteSource,
    wait: Option<Duration>,
    windows_terminal: bool,
) -> io::Result<Key> {
    let Some(byte) = source.byte(wait)? else {
        return Ok(Key::Idle);
    };
    Ok(match byte {
        0x1b => escape(source)?,
        8 if windows_terminal => Key::Press(Press::new(Code::Backspace, CTRL)),
        0x20..=0x7e => Key::Text(char::from(byte).to_string()),
        0xc2..=0xf4 => {
            let length = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            let mut bytes = vec![byte];
            while bytes.len() < length {
                match source.byte(Some(ESCAPE_WAIT))? {
                    Some(next) => bytes.push(next),
                    None => return Ok(Key::Ignored),
                }
            }
            String::from_utf8(bytes).map_or(Key::Ignored, Key::Text)
        }
        _ => control(byte).map_or(Key::Ignored, Key::Press),
    })
}

/// The key that sends a control byte: Ctrl and a letter, but for the keys that send their own.
fn control(byte: u8) -> Option<Press> {
    let (code, modifiers) = match byte {
        b'\r' => (Code::Enter, 0),
        b'\t' => (Code::Tab, 0),
        0x7f | 8 => (Code::Backspace, 0),
        0 => (Code::Char(' '), CTRL),
        1..=26 => (Code::Char(char::from(b'a' + byte - 1)), CTRL),
        28 => (Code::Char('\\'), CTRL),
        29 => (Code::Char(']'), CTRL),
        // Ctrl+_, which terminals also send for Ctrl+-.
        31 => (Code::Char('-'), CTRL),
        _ => return None,
    };
    Some(Press::new(code, modifiers))
}

/// A printable character's key: a letter in lower case, Shift added for an upper-case one.
fn printable(c: char, modifiers: u8) -> Press {
    match c.is_ascii_uppercase() {
        true => Press::new(Code::Char(c.to_ascii_lowercase()), modifiers | SHIFT),
        false => Press::new(Code::Char(c), modifiers),
    }
}

fn escape(source: &mut impl ByteSource) -> io::Result<Key> {
    let mut next = source.byte(Some(ESCAPE_WAIT))?;
    // Some terminals send Alt+key as an extra Escape before the key's own sequence.
    let mut alt = 0;
    while next == Some(0x1b) {
        alt = ALT;
        next = source.byte(Some(ESCAPE_WAIT))?;
    }
    let key = match next {
        None => return Ok(Key::Press(Press::new(Code::Escape, 0))),
        Some(b'[') => csi(source)?,
        Some(b']') => osc(source)?,
        Some(b'O') => match source.byte(Some(ESCAPE_WAIT))? {
            Some(byte) => ss3(byte).map_or(Key::Ignored, |code| Key::Press(Press::new(code, 0))),
            None => Key::Ignored,
        },
        // Alt and a key, which the terminal sends as Escape before the key.
        Some(byte @ 0x20..=0x7e) => Key::Press(printable(char::from(byte), ALT)),
        Some(byte) => match control(byte) {
            Some(press) => Key::Press(Press::new(press.code, press.modifiers | ALT)),
            None => Key::Ignored,
        },
    };
    Ok(match key {
        Key::Press(press) => Key::Press(Press::new(press.code, press.modifiers | alt)),
        key => key,
    })
}

/// An OSC sequence, `ESC ]` and its text to BEL or `ESC \`, as terminals answer questions about
/// themselves; Alt+] when nothing follows `ESC ]`.
fn osc(source: &mut impl ByteSource) -> io::Result<Key> {
    const LONGEST: usize = 256;
    let mut text = Vec::new();
    let mut long = false;
    loop {
        match source.byte(Some(ESCAPE_WAIT))? {
            None if text.is_empty() && !long => return Ok(Key::Press(printable(']', ALT))),
            None => return Ok(Key::Ignored),
            Some(0x07) => break,
            Some(0x1b) => match source.byte(Some(ESCAPE_WAIT))? {
                Some(b'\\') => break,
                _ => return Ok(Key::Ignored),
            },
            Some(byte) if text.len() < LONGEST => text.push(byte),
            Some(_) => long = true,
        }
    }
    let text = String::from_utf8_lossy(&text);
    let answer = background::answer(&text).filter(|_| !long);
    Ok(answer.map_or(Key::Ignored, |(asked, rgb)| Key::Answer(asked, rgb)))
}

/// The key of `ESC O` and `byte`, as terminals send some keys in application mode.
fn ss3(byte: u8) -> Option<Code> {
    Some(match byte {
        b'A' => Code::Up,
        b'B' => Code::Down,
        b'C' => Code::Right,
        b'D' => Code::Left,
        b'H' => Code::Home,
        b'F' => Code::End,
        b'P'..=b'S' => Code::F(byte - b'P' + 1),
        _ => return None,
    })
}

fn csi(source: &mut impl ByteSource) -> io::Result<Key> {
    let mut parameters = Vec::new();
    let last = loop {
        let Some(byte) = source.byte(Some(ESCAPE_WAIT))? else {
            return Ok(Key::Ignored);
        };
        // rxvt ends Shift's keys with `$`, which is no final byte.
        if (0x40..=0x7e).contains(&byte) || byte == b'$' {
            break byte;
        }
        if parameters.len() < 32 {
            parameters.push(byte);
        }
    };
    let parameters = String::from_utf8_lossy(&parameters);
    if let Some(report) = parameters.strip_prefix('<') {
        return Ok(mouse(report, last).map_or(Key::Ignored, Key::Mouse));
    }
    // A report in the older X10 form, from a terminal without SGR reports: three bytes follow.
    if (last, parameters.as_ref()) == (b'M', "") {
        for _ in 0..3 {
            source.byte(Some(ESCAPE_WAIT))?;
        }
        return Ok(Key::Ignored);
    }
    // Each parameter's first part: the kitty protocol adds others after `:`.
    let mut numbers = (parameters.split(';')).map(|parameter| parameter.split(':').next());
    let first = numbers.next().flatten().unwrap_or("");
    let modifiers = match numbers.next().flatten().map(str::parse::<u8>) {
        Some(Ok(parameter)) => parameter.saturating_sub(1) & (SHIFT | ALT | CTRL),
        _ => 0,
    };
    let third = numbers.next().flatten();
    let code = match (last, first) {
        (b'A', _) => Code::Up,
        (b'B', _) => Code::Down,
        (b'C', _) => Code::Right,
        (b'D', _) => Code::Left,
        (b'H', _) => Code::Home,
        (b'F', _) => Code::End,
        (b'Z', _) => return Ok(Key::Press(Press::new(Code::Tab, modifiers | SHIFT))),
        (b'~', "200") => return paste(source),
        // xterm's modifyOtherKeys: `CSI 27 ; modifiers ; code ~`.
        (b'~', "27") => return Ok(codepoint(third, modifiers)),
        (b'u', _) => return Ok(codepoint(Some(first), modifiers)),
        // rxvt's: `^` with Ctrl, `$` with Shift, `@` with both.
        (b'~' | b'^' | b'$' | b'@', number) => {
            let Some(code) = tilde(number) else {
                return Ok(Key::Ignored);
            };
            let held = match last {
                b'^' => CTRL,
                b'$' => SHIFT,
                b'@' => CTRL | SHIFT,
                _ => 0,
            };
            return Ok(Key::Press(Press::new(code, modifiers | held)));
        }
        _ => return Ok(Key::Ignored),
    };
    Ok(Key::Press(Press::new(code, modifiers)))
}

/// The key of `CSI number ~`.
fn tilde(number: &str) -> Option<Code> {
    Some(match number {
        "1" | "7" => Code::Home,
        "2" => Code::Insert,
        "3" => Code::Delete,
        "4" | "8" => Code::End,
        "5" => Code::PageUp,
        "6" => Code::PageDown,
        "11" => Code::F(1),
        "12" => Code::F(2),
        "13" => Code::F(3),
        "14" => Code::F(4),
        "15" => Code::F(5),
        "17" => Code::F(6),
        "18" => Code::F(7),
        "19" => Code::F(8),
        "20" => Code::F(9),
        "21" => Code::F(10),
        "23" => Code::F(11),
        "24" => Code::F(12),
        _ => return None,
    })
}

/// The key of a character's code with `modifiers`, as the kitty protocol and modifyOtherKeys
/// report it; a printable character without Ctrl or Alt is text.
fn codepoint(code: Option<&str>, modifiers: u8) -> Key {
    let Some(c) = code
        .and_then(|code| code.parse().ok())
        .and_then(char::from_u32)
    else {
        return Key::Ignored;
    };
    let code = match c {
        '\r' => Code::Enter,
        '\t' => Code::Tab,
        '\x1b' => Code::Escape,
        '\x7f' | '\x08' => Code::Backspace,
        c if c.is_control() => return Key::Ignored,
        c if modifiers & !SHIFT == 0 && (modifiers == 0 || c.is_ascii_alphabetic()) => {
            let typed = match modifiers {
                0 => c,
                _ => c.to_ascii_uppercase(),
            };
            return Key::Text(typed.to_string());
        }
        c => return Key::Press(printable(c, modifiers)),
    };
    Key::Press(Press::new(code, modifiers))
}

fn mouse(report: &str, last: u8) -> Option<Mouse> {
    let mut numbers = report.split(';').map(str::parse::<usize>);
    let (Some(Ok(button)), Some(Ok(column)), Some(Ok(row)), None) = (
        numbers.next(),
        numbers.next(),
        numbers.next(),
        numbers.next(),
    ) else {
        return None;
    };
    Some(Mouse {
        button: u16::try_from(button).ok()?,
        column: column.checked_sub(1)?,
        row: row.checked_sub(1)?,
        release: match last {
            b'M' => false,
            b'm' => true,
            _ => return None,
        },
    })
}

fn paste(source: &mut impl ByteSource) -> io::Result<Key> {
    const END: &[u8] = b"\x1b[201~";
    let mut bytes = Vec::new();
    while !bytes.ends_with(END) {
        if let Some(byte) = source.byte(None)? {
            bytes.push(byte);
        }
    }
    bytes.truncate(bytes.len() - END.len());
    Ok(Key::Paste(clean(&String::from_utf8_lossy(&bytes))))
}

/// Pasted text keeps its line breaks and tabs; other control characters are dropped.
pub(super) fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

pub(super) struct RawMode {
    original: console::Mode,
}

impl RawMode {
    pub(super) fn enter() -> io::Result<Self> {
        let original = console::Mode::get().ok_or(io::ErrorKind::Unsupported)?;
        let mode = Self { original };
        mode.resume()?;
        Ok(mode)
    }

    /// Raw mode with bracketed paste on.
    pub(super) fn resume(&self) -> io::Result<()> {
        self.original.raw().set()?;
        eprint!("\x1b[?2004h");
        Ok(())
    }

    /// The terminal as the shell left it, for an external editor or the shell itself.
    pub(super) fn suspend(&self) {
        eprint!("\x1b[?2004l");
        let _ = io::stderr().flush();
        let _ = self.original.set();
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.suspend();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::agent_cli::background::Asked;
    use std::collections::VecDeque;

    impl ByteSource for VecDeque<u8> {
        fn byte(&mut self, _wait: Option<Duration>) -> io::Result<Option<u8>> {
            Ok(self.pop_front())
        }
    }

    pub(in crate::agent_cli) fn keys(bytes: &[u8]) -> Vec<Key> {
        read(bytes, false)
    }

    fn read(bytes: &[u8], windows_terminal: bool) -> Vec<Key> {
        let mut source: VecDeque<u8> = bytes.iter().copied().collect();
        let mut keys = Vec::new();
        loop {
            match read_key(&mut source, None, windows_terminal).unwrap() {
                Key::Idle => return keys,
                key => keys.push(key),
            }
        }
    }

    /// The key `name` names, as the keybindings file does (`ctrl+a`, `pageUp`).
    pub(in crate::agent_cli) fn key(name: &str) -> Key {
        Key::Press(keymap::parse(name).unwrap_or_else(|| panic!("{name} is no key")))
    }

    fn named(names: &[&str]) -> Vec<Key> {
        names.iter().map(|name| key(name)).collect()
    }

    #[test]
    fn the_terminals_answers_are_taken_and_never_typed() {
        let long = format!("\x1b]11;rgb:{}\x07a", "0".repeat(300));
        assert_eq!(
            keys(
                format!(
                    "\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\\x1b]11;#fafafa\x07\x1b]10;rgb:0/0/0\x07\
                     \x1b]4;3;rgb:c1c1/9c9c/0000\x1b\\\x1b]12;rgb:0/0/0\x07\x1b]11;?\x07{long}\x1b]"
                )
                .as_bytes()
            ),
            [
                Key::Answer(Asked::Background, (12, 12, 12)),
                Key::Answer(Asked::Background, (250, 250, 250)),
                Key::Answer(Asked::Foreground, (0, 0, 0)),
                Key::Answer(Asked::Palette(3), (193, 156, 0)),
                Key::Ignored,
                Key::Ignored,
                Key::Ignored,
                Key::Text("a".into()),
                key("alt+]"),
            ]
        );
        // Cut short, an answer types nothing of itself.
        assert_eq!(keys(b"\x1b]11;rgb:0c"), [Key::Ignored]);
    }

    #[test]
    fn decodes_keys_terminals_send() {
        assert_eq!(
            keys("aé界\r\n\x1b\r\x1b[13;2u\x1b[27;2;13~\x1b[13;3u\x1b[27;3;13~\x1b[13u".as_bytes()),
            [
                Key::Text("a".into()),
                Key::Text("é".into()),
                Key::Text("界".into()),
                key("enter"),
                key("ctrl+j"),
                key("alt+enter"),
                key("shift+enter"),
                key("shift+enter"),
                key("alt+enter"),
                key("alt+enter"),
                key("enter"),
            ]
        );
        assert_eq!(
            keys(b"\x1b[1;3A\x1b\x1b[A\x1b[A\x1bOA\x1b[1;5A\x1b[1;2B"),
            named(&["alt+up", "alt+up", "up", "up", "ctrl+up", "shift+down"])
        );
        assert_eq!(
            keys(b"\x1b[D\x1bOC\x1b[1;5D\x1b[1;3C\x1bb\x1b[H\x1b[4~\x1b[3~\x1b[3;5~\x1b\x7f\x17"),
            named(&[
                "left",
                "right",
                "ctrl+left",
                "alt+right",
                "alt+b",
                "home",
                "end",
                "delete",
                "ctrl+delete",
                "alt+backspace",
                "ctrl+w",
            ])
        );
        assert_eq!(
            keys(b"\x03\x04\x14\x16\x1bv\x1b\x1b[D\x1b[99;5u\x1b[5~\x1b[6~\x1bX\x08\t\x1b"),
            named(&[
                "ctrl+c",
                "ctrl+d",
                "ctrl+t",
                "ctrl+v",
                "alt+v",
                "alt+left",
                "ctrl+c",
                "pageUp",
                "pageDown",
                "alt+shift+x",
                "backspace",
                "tab",
                "escape",
            ])
        );
        assert_eq!(
            keys(b"\x1bOP\x1b[24~\x1b[2~\x1b[5;5~\x1b[3$\x1b[99;9u\x1b[9999999u\x1b[?1u\x1bO"),
            [
                key("f1"),
                key("f12"),
                key("insert"),
                key("ctrl+pageUp"),
                key("shift+delete"),
                Key::Text("c".into()),
                Key::Ignored,
                Key::Ignored,
                Key::Ignored,
            ]
        );
    }

    #[test]
    fn characters_with_shift_alone_are_text_and_upper_case_letters_are_shift_and_the_letter() {
        assert_eq!(
            keys(b"\x1b[27;2;76~\x1b[108;2u\x1b[97u\x1b[49;2u"),
            [
                Key::Text("L".into()),
                Key::Text("L".into()),
                Key::Text("a".into()),
                key("shift+1"),
            ]
        );
        assert_eq!(Key::Text("L".into()).press(), keymap::parse("shift+l"));
        assert_eq!(Key::Text("l".into()).press(), keymap::parse("l"));
        assert_eq!(Key::Text("!".into()).press(), keymap::parse("!"));
        assert_eq!(Key::Paste("L".into()).press(), None);
        assert_eq!(Key::Text("ab".into()).press(), None);
    }

    #[test]
    fn decodes_sgr_mouse_reports() {
        let mouse = |button, column, row, release| {
            Key::Mouse(Mouse {
                button,
                column,
                row,
                release,
            })
        };
        assert_eq!(
            keys(
                b"\x1b[<0;1;1M\x1b[<32;120;40M\x1b[<0;120;40m\x1b[<65;3;2M\x1b[<0;0;1M\x1b[<1;2M\x1b[M`!!a"
            ),
            [
                mouse(0, 0, 0, false),
                mouse(32, 119, 39, false),
                mouse(0, 119, 39, true),
                mouse(65, 2, 1, false),
                Key::Ignored,
                Key::Ignored,
                Key::Ignored,
                Key::Text("a".into()),
            ]
        );
    }

    #[test]
    fn shift_tab_is_back_tab() {
        assert_eq!(
            keys(b"\x1b[Z\x1b[27;2;9~\x1b[9;2u"),
            named(&["shift+tab"; 3])
        );
    }

    #[test]
    fn ctrl_p_with_shift_where_the_terminal_tells_them_apart() {
        assert_eq!(
            keys(b"\x10\x1b[112;5u\x1b[27;5;112~"),
            named(&["ctrl+p"; 3])
        );
        assert_eq!(
            keys(b"\x1b[112;6u\x1b[27;6;112~\x1b[27;6;80~"),
            named(&["ctrl+shift+p"; 3])
        );
    }

    #[test]
    fn decodes_scramjets_kill_ring_undo_jump_and_scroll_keys() {
        assert_eq!(
            keys(b"\x19\x1by\x1f\x1d\x1b\x1d\x0c\x1b[1;5H\x1b[1;5F\x1b[7^\x1b[8^"),
            named(&[
                "ctrl+y",
                "alt+y",
                "ctrl+-",
                "ctrl+]",
                "ctrl+alt+]",
                "ctrl+l",
                "ctrl+home",
                "ctrl+end",
                "ctrl+home",
                "ctrl+end",
            ])
        );
        assert_eq!(
            keys(b"\x1b[121;5u\x1b[121;3u\x1b[45;5u\x1b[93;5u\x1b[27;7;93~\x1b[108;5u\x1b[1H"),
            named(&[
                "ctrl+y",
                "alt+y",
                "ctrl+-",
                "ctrl+]",
                "ctrl+alt+]",
                "ctrl+l",
                "home",
            ])
        );
    }

    #[test]
    fn keys_mean_their_actions_by_default() {
        let cases = [
            (&b"\x01"[..], Binding::CursorLineStart),
            (b"\x01", Binding::SelectFirst),
            (b"\x01", Binding::ModelsEnableAll),
            (b"\x01", Binding::TreeFilterAll),
            (b"\x0e", Binding::CursorDown),
            (b"\x0e", Binding::SessionToggleNamedFilter),
            (b"\x1b\r", Binding::FollowUp),
            (b"\x1b[1;3A", Binding::Dequeue),
            (b"\x1b[1;3B", Binding::ModelsReorderDown),
            (b"\x1b[27;6;79~", Binding::TreeFilterCycleBackward),
            (b"\x1b[3;3~", Binding::DeleteWordForward),
            (b"L", Binding::TreeEditLabel),
            (b"\x1b", Binding::Interrupt),
            (b"\x1b", Binding::SelectCancel),
            (b"\x03", Binding::SelectCancel),
            (b"\x16", Binding::PasteImage),
        ];
        for (bytes, binding) in cases {
            let [key] = &keys(bytes)[..] else {
                panic!("{bytes:?}");
            };
            assert!(key.is(binding), "{bytes:?} {binding:?}");
        }
        assert!(!Key::Text("l".into()).is(Binding::TreeEditLabel));
        let [ctrl_backspace] = &read(b"\x08", true)[..] else {
            panic!("one key");
        };
        assert!(ctrl_backspace.is(Binding::DeleteWordBackward));
        assert!(ctrl_backspace.is(Binding::SessionDeleteNoninvasive));
        assert!(!ctrl_backspace.is(Binding::DeleteCharBackward));
        assert!(key("backspace").is(Binding::DeleteCharBackward));
        assert!(!key("ctrl+up").is(Binding::CursorUp));
        assert!(!Key::Idle.is(Binding::Interrupt));
    }
}
