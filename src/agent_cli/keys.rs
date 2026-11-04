//! Keys read from the terminal in raw mode: escape sequences, SGR mouse reports and
//! bracketed pastes decoded into `Key`s.

use rustix::termios::{self, OptionalActions, Termios};
use std::{
    io::{self, Write},
    time::Duration,
};

/// How long the rest of an escape sequence may take to arrive before Escape counts as a lone key.
pub(super) const ESCAPE_WAIT: Duration = Duration::from_millis(30);

#[derive(Debug, PartialEq)]
pub(super) enum Key {
    Text(String),
    Paste(String),
    Enter,
    NewLine,
    /// A new line in a dialog's text; at the prompt the full screen sends with it, or queues a
    /// follow-up during a turn.
    AltEnter,
    Backspace,
    Delete,
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    /// Puts the messages queued during a turn back in the editor; Up in a dialog's text; moves
    /// a model up in `/scoped-models`.
    AltUp,
    /// Down, but in `/scoped-models`, where it moves a model down.
    AltDown,
    /// Ctrl+N: Down, but in `/resume`, where it shows only the named sessions, as in Scramjet.
    CtrlN,
    PageUp,
    PageDown,
    LineStart,
    /// Ctrl+A: as Home (`LineStart`), but in `/scoped-models`, where it switches on all models.
    CtrlA,
    LineEnd,
    DeleteWordBack,
    DeleteWordForward,
    KillStart,
    KillEnd,
    /// Ctrl+Y: the text deleted last, put in again.
    Yank,
    /// Alt+Y, after Ctrl+Y: that text replaced with the one deleted before it.
    YankPop,
    /// Ctrl+-.
    Undo,
    /// Ctrl+] (`true`) or Ctrl+Alt+]: to the next or previous place of the character typed next.
    Jump(bool),
    /// Ctrl+L, Scramjet's key to open the model selector.
    SelectModel,
    /// Ctrl+Home and Ctrl+End: the conversation's start and end.
    Top,
    Bottom,
    Interrupt,
    EndOfInput,
    /// Ctrl+V, Scramjet's key to paste an image, or Alt+V, which still reaches the program in
    /// a terminal that takes Ctrl+V to paste text, as Windows Terminal does.
    PasteImage,
    ToggleThinking,
    /// Shift+Tab, `ESC [ Z`.
    BackTab,
    /// Ctrl+P, Scramjet's key to switch to the next model; `false`: Shift+Ctrl+P, to the one
    /// before, which only a terminal reporting modified keys tells apart.
    CycleModel(bool),
    External,
    /// Ctrl+O, Scramjet's key to expand tool output.
    Expand,
    /// Shift+Ctrl+O, which cycles `/tree`'s filters backwards; only a terminal reporting modified
    /// keys tells it apart.
    ExpandBack,
    /// Ctrl+R, Scramjet's key to rename a session in `/resume`.
    Rename,
    /// Ctrl+S, Scramjet's key to save the models chosen in `/scoped-models`.
    Save,
    /// Ctrl+X, Scramjet's key to switch off all models in `/scoped-models`.
    ClearAll,
    Suspend,
    /// A lone Escape.
    Escape,
    /// A mouse report, which only the full screen asks the terminal for.
    Mouse(Mouse),
    Idle,
    Ignored,
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

pub(super) fn read_key(source: &mut impl ByteSource, wait: Option<Duration>) -> io::Result<Key> {
    let Some(byte) = source.byte(wait)? else {
        return Ok(Key::Idle);
    };
    Ok(match byte {
        0x1b => escape(source)?,
        b'\r' => Key::Enter,
        b'\n' => Key::NewLine,
        b'\t' => Key::Text("\t".into()),
        0x7f | 8 => Key::Backspace,
        1 => Key::CtrlA,
        2 => Key::Left,
        3 => Key::Interrupt,
        4 => Key::EndOfInput,
        5 => Key::LineEnd,
        6 => Key::Right,
        7 => Key::External,
        11 => Key::KillEnd,
        12 => Key::SelectModel,
        14 => Key::CtrlN,
        15 => Key::Expand,
        16 => Key::CycleModel(true),
        18 => Key::Rename,
        19 => Key::Save,
        20 => Key::ToggleThinking,
        21 => Key::KillStart,
        22 => Key::PasteImage,
        23 => Key::DeleteWordBack,
        24 => Key::ClearAll,
        25 => Key::Yank,
        26 => Key::Suspend,
        29 => Key::Jump(true),
        // Ctrl+_, which terminals also send for Ctrl+-.
        31 => Key::Undo,
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
        _ => Key::Ignored,
    })
}

fn escape(source: &mut impl ByteSource) -> io::Result<Key> {
    let mut next = source.byte(Some(ESCAPE_WAIT))?;
    // Some terminals send Alt+key as an extra Escape before the key's own sequence.
    let mut alt = false;
    while next == Some(0x1b) {
        alt = true;
        next = source.byte(Some(ESCAPE_WAIT))?;
    }
    Ok(match next {
        None => Key::Escape,
        Some(b'[') => match csi(source)? {
            Key::Up if alt => Key::AltUp,
            Key::Down if alt => Key::AltDown,
            key => key,
        },
        Some(b'O') => match source.byte(Some(ESCAPE_WAIT))? {
            Some(b'A') => Key::Up,
            Some(b'B') => Key::Down,
            Some(b'C') => Key::Right,
            Some(b'D') => Key::Left,
            Some(b'H') => Key::LineStart,
            Some(b'F') => Key::LineEnd,
            _ => Key::Ignored,
        },
        Some(b'\r') => Key::AltEnter,
        Some(b'\n') => Key::NewLine,
        Some(0x7f | 8) => Key::DeleteWordBack,
        Some(b'b') => Key::WordLeft,
        Some(b'f') => Key::WordRight,
        Some(b'd') => Key::DeleteWordForward,
        Some(b'v') => Key::PasteImage,
        Some(b'y') => Key::YankPop,
        Some(29) => Key::Jump(false),
        Some(_) => Key::Ignored,
    })
}

fn csi(source: &mut impl ByteSource) -> io::Result<Key> {
    let mut parameters = Vec::new();
    let last = loop {
        let Some(byte) = source.byte(Some(ESCAPE_WAIT))? else {
            return Ok(Key::Ignored);
        };
        if (0x40..=0x7e).contains(&byte) {
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
    let modifier = parameters.split(';').nth(1);
    let word = matches!(modifier, Some("3" | "5"));
    Ok(match (last, parameters.as_ref()) {
        (b'A', "1;3") => Key::AltUp,
        (b'B', "1;3") => Key::AltDown,
        (b'A', _) => Key::Up,
        (b'B', _) => Key::Down,
        (b'C', _) if word => Key::WordRight,
        (b'D', _) if word => Key::WordLeft,
        (b'C', _) => Key::Right,
        (b'D', _) => Key::Left,
        (b'H', "1;5") | (b'^', "7") => Key::Top,
        (b'F', "1;5") | (b'^', "8") => Key::Bottom,
        (b'H', _) | (b'~', "1" | "7") => Key::LineStart,
        (b'F', _) | (b'~', "4" | "8") => Key::LineEnd,
        (b'~', "3") => Key::Delete,
        (b'~', "5") => Key::PageUp,
        (b'~', "6") => Key::PageDown,
        (b'~', "3;3" | "3;5") => Key::DeleteWordForward,
        (b'~', "200") => paste(source)?,
        (b'~', "27;2;13") | (b'u', "13;2") => Key::NewLine,
        (b'~', "27;3;13") | (b'u', "13;3") => Key::AltEnter,
        (b'u', "13") => Key::Enter,
        (b'Z', _) | (b'~', "27;2;9") | (b'u', "9;2") => Key::BackTab,
        (b'~', "27;5;112") | (b'u', "112;5") => Key::CycleModel(true),
        (b'~', "27;6;112" | "27;6;80") | (b'u', "112;6") => Key::CycleModel(false),
        (b'~', "27;5;108") | (b'u', "108;5") => Key::SelectModel,
        (b'~', "27;6;111" | "27;6;79") | (b'u', "111;6") => Key::ExpandBack,
        (b'~', "27;5;121") | (b'u', "121;5") => Key::Yank,
        (b'~', "27;3;121") | (b'u', "121;3") => Key::YankPop,
        (b'~', "27;5;45") | (b'u', "45;5") => Key::Undo,
        (b'~', "27;5;93") | (b'u', "93;5") => Key::Jump(true),
        (b'~', "27;7;93") | (b'u', "93;7") => Key::Jump(false),
        _ => Key::Ignored,
    })
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
    original: Termios,
}

impl RawMode {
    pub(super) fn enter() -> io::Result<Self> {
        let original = termios::tcgetattr(io::stdin())?;
        let mode = Self { original };
        mode.resume()?;
        Ok(mode)
    }

    /// Raw mode with bracketed paste on.
    pub(super) fn resume(&self) -> io::Result<()> {
        let mut raw = self.original.clone();
        raw.make_raw();
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &raw)?;
        eprint!("\x1b[?2004h");
        Ok(())
    }

    /// The terminal as the shell left it, for an external editor or the shell itself.
    pub(super) fn suspend(&self) {
        eprint!("\x1b[?2004l");
        let _ = io::stderr().flush();
        let _ = termios::tcsetattr(io::stdin(), OptionalActions::Now, &self.original);
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
    use std::collections::VecDeque;

    impl ByteSource for VecDeque<u8> {
        fn byte(&mut self, _wait: Option<Duration>) -> io::Result<Option<u8>> {
            Ok(self.pop_front())
        }
    }

    pub(in crate::agent_cli) fn keys(bytes: &[u8]) -> Vec<Key> {
        let mut source: VecDeque<u8> = bytes.iter().copied().collect();
        let mut keys = Vec::new();
        loop {
            match read_key(&mut source, None).unwrap() {
                Key::Idle => return keys,
                key => keys.push(key),
            }
        }
    }

    #[test]
    fn decodes_keys_terminals_send() {
        assert_eq!(
            keys("aé界\r\n\x1b\r\x1b[13;2u\x1b[27;2;13~\x1b[13;3u\x1b[27;3;13~".as_bytes()),
            [
                Key::Text("a".into()),
                Key::Text("é".into()),
                Key::Text("界".into()),
                Key::Enter,
                Key::NewLine,
                Key::AltEnter,
                Key::NewLine,
                Key::NewLine,
                Key::AltEnter,
                Key::AltEnter,
            ]
        );
        assert_eq!(
            keys(b"\x1b[1;3A\x1b\x1b[A\x1b[A\x1b[1;5A"),
            [Key::AltUp, Key::AltUp, Key::Up, Key::Up]
        );
        assert_eq!(
            keys(b"\x1b[D\x1bOC\x1b[1;5D\x1b[1;3C\x1bb\x1b[H\x1b[4~\x1b[3~\x1b[3;5~\x1b\x7f\x17"),
            [
                Key::Left,
                Key::Right,
                Key::WordLeft,
                Key::WordRight,
                Key::WordLeft,
                Key::LineStart,
                Key::LineEnd,
                Key::Delete,
                Key::DeleteWordForward,
                Key::DeleteWordBack,
                Key::DeleteWordBack,
            ]
        );
        assert_eq!(
            keys(b"\x03\x04\x14\x16\x1bv\x1b\x1b[D\x1b[99;5u\x1b[5~\x1b[6~\x1bx\x1b"),
            [
                Key::Interrupt,
                Key::EndOfInput,
                Key::ToggleThinking,
                Key::PasteImage,
                Key::PasteImage,
                Key::Left,
                Key::Ignored,
                Key::PageUp,
                Key::PageDown,
                Key::Ignored,
                Key::Escape
            ]
        );
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
            [Key::BackTab, Key::BackTab, Key::BackTab]
        );
    }

    #[test]
    fn ctrl_p_cycles_models_and_with_shift_back_where_the_terminal_tells_them_apart() {
        let forward = keys(b"\x10\x1b[112;5u\x1b[27;5;112~");
        assert_eq!(forward, [const { Key::CycleModel(true) }; 3]);
        let back = keys(b"\x1b[112;6u\x1b[27;6;112~\x1b[27;6;80~");
        assert_eq!(back, [const { Key::CycleModel(false) }; 3]);
    }

    #[test]
    fn ctrl_g_opens_the_external_editor_and_ctrl_z_suspends() {
        assert_eq!(
            keys(b"ab\x07\x1a"),
            [
                Key::Text("a".into()),
                Key::Text("b".into()),
                Key::External,
                Key::Suspend
            ]
        );
    }

    #[test]
    fn decodes_scramjets_kill_ring_undo_jump_and_scroll_keys() {
        assert_eq!(
            keys(b"\x19\x1by\x1f\x1d\x1b\x1d\x0c\x1b[1;5H\x1b[1;5F\x1b[7^\x1b[8^"),
            [
                Key::Yank,
                Key::YankPop,
                Key::Undo,
                Key::Jump(true),
                Key::Jump(false),
                Key::SelectModel,
                Key::Top,
                Key::Bottom,
                Key::Top,
                Key::Bottom,
            ]
        );
        assert_eq!(
            keys(b"\x1b[121;5u\x1b[121;3u\x1b[45;5u\x1b[93;5u\x1b[27;7;93~\x1b[108;5u\x1b[1H"),
            [
                Key::Yank,
                Key::YankPop,
                Key::Undo,
                Key::Jump(true),
                Key::Jump(false),
                Key::SelectModel,
                Key::LineStart,
            ]
        );
    }
}
