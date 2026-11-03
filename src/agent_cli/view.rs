//! The conversation in the alternate screen with every tool result in full, as Scramjet's
//! Ctrl+O expands its tool blocks, scrolled with the keys while the prompt waits underneath, or
//! during a turn, following the turn while its inline output waits.

use super::editor::{Key, RESIZE_CHECK, RawMode, Stdin, read_key, terminal_size};
use super::{clip, editor, job, style};
use std::io::{self, Write};
use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

const KEYS: &str = "↑↓ PgUp PgDn Home End scroll · Ctrl+T thinking · Esc close";
/// How often a view of a running turn looks for more of it.
const FOLLOW_CHECK: Duration = Duration::from_millis(100);

/// A running turn that the view follows.
pub(super) trait Turn {
    /// Changes whenever the turn has more to show.
    fn version(&self) -> usize;
    /// What the turn has shown so far, `width` columns wide.
    fn render(&self, width: usize) -> String;
    /// What the agent does now, as the status line says it.
    fn activity(&self) -> Option<String>;
}

pub(super) struct Live<'a> {
    pub(super) turn: &'a dyn Turn,
    /// Set once the agent waits for the user: the turn ended or asks something.
    pub(super) waiting: &'a AtomicBool,
}

impl Live<'_> {
    fn activity(&self) -> String {
        match self.waiting.load(Ordering::Relaxed) {
            true => "waiting for you".into(),
            false => self.turn.activity().unwrap_or_else(|| "… working".into()),
        }
    }
}

/// Shows the text `render(width)` returns for the terminal's width, from its end, until Escape,
/// `q`, Ctrl+O, Ctrl-C or Ctrl-D, taking keys from `typed` first. With `live`, the running
/// turn follows that text, rendered again as it grows, and the output the turn writes meanwhile
/// is held until the view closes. Ctrl+T flips `hide_thinking` and renders it again. Returns the
/// keys typed after the one that closed it.
pub(super) fn show(
    render: impl Fn(usize) -> String,
    live: Option<Live>,
    hide_thinking: &AtomicBool,
    typed: Vec<u8>,
) -> io::Result<Vec<u8>> {
    // Dropped in reverse: the screen, the terminal mode, then the held output is written.
    let mut hold = live.as_ref().map(|_| Hold::start());
    let raw = RawMode::enter()?;
    let mut screen = Screen::enter();
    let mut source = Stdin::starting_with(typed);
    let (mut width, mut height) = terminal_size();
    let turn_lines = |width| match &live {
        Some(live) => screen_lines(&live.turn.render(width)),
        None => Vec::new(),
    };
    let version = || live.as_ref().map(|live| live.turn.version());
    let activity = || live.as_ref().map(Live::activity);
    let mut shown = version();
    let mut text = Text {
        before: screen_lines(&render(width)),
        turn: turn_lines(width),
    };
    let mut doing = activity();
    let wait = if live.is_some() {
        FOLLOW_CHECK
    } else {
        RESIZE_CHECK
    };
    let mut top = usize::MAX;
    let mut note: Option<&str> = None;
    let mut redraw = true;
    loop {
        let rows = height.saturating_sub(1).max(1);
        let last = text.len().saturating_sub(rows);
        top = top.min(last);
        if redraw && !source.pending() {
            draw(&text, top, rows, width, note.take(), doing.as_deref())?;
            redraw = false;
        }
        let key = match read_key(&mut source, Some(wait)) {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(Vec::new()),
            result => result?,
        };
        let page = rows.saturating_sub(1).max(1);
        top = match key {
            Key::Idle if job::continued() => top,
            Key::Idle => {
                let size = terminal_size();
                let now = version();
                if size != (width, height) {
                    let keep = position(top, last, text.len());
                    (width, height, shown) = (size.0, size.1, now);
                    text.before = screen_lines(&render(width));
                    text.turn = turn_lines(width);
                    doing = activity();
                    keep(text.len(), height.saturating_sub(1).max(1))
                } else if now != shown {
                    // At the end, the view stays there as the turn grows.
                    let end = top >= last;
                    shown = now;
                    text.turn = turn_lines(width);
                    doing = activity();
                    if end { usize::MAX } else { top }
                } else if activity() != doing {
                    doing = activity();
                    top
                } else {
                    continue;
                }
            }
            Key::Suspend => {
                drop(screen);
                raw.suspend();
                // The shell writes under the turn's output as far as it went.
                drop(hold.take());
                job::suspend();
                job::continued();
                raw.resume()?;
                hold = live.as_ref().map(|_| Hold::start());
                screen = Screen::enter();
                top
            }
            Key::ToggleThinking => {
                let hidden = !hide_thinking.fetch_xor(true, Ordering::Relaxed);
                note = Some(editor::thinking_note(hidden));
                let keep = position(top, last, text.len());
                shown = version();
                text.before = screen_lines(&render(width));
                text.turn = turn_lines(width);
                keep(text.len(), rows)
            }
            Key::Escape | Key::Interrupt | Key::EndOfInput | Key::Expand => {
                return Ok(source.rest());
            }
            Key::Text(text) if text == "q" => return Ok(source.rest()),
            Key::Up => top.saturating_sub(1),
            Key::Text(text) if text == "k" => top.saturating_sub(1),
            Key::Down => top + 1,
            Key::Text(text) if text == "j" => top + 1,
            Key::PageUp => top.saturating_sub(page),
            Key::Text(text) if text == "b" => top.saturating_sub(page),
            Key::PageDown => top + page,
            Key::Text(text) if text == " " => top + page,
            Key::LineStart => 0,
            Key::Text(text) if text == "g" => 0,
            Key::LineEnd => last,
            Key::Text(text) if text == "G" => last,
            _ => continue,
        };
        redraw = true;
    }
}

/// The lines shown: the conversation, then a blank line and the running turn's.
struct Text {
    before: Vec<String>,
    turn: Vec<String>,
}

impl Text {
    fn gap(&self) -> usize {
        usize::from(!self.before.is_empty() && !self.turn.is_empty())
    }
    fn len(&self) -> usize {
        self.before.len() + self.gap() + self.turn.len()
    }
    fn get(&self, index: usize) -> Option<&str> {
        match index.checked_sub(self.before.len() + self.gap()) {
            Some(index) => self.turn.get(index).map(String::as_str),
            None => Some(self.before.get(index).map_or("", String::as_str)),
        }
    }
}

/// Output for the inline screen, kept here while the view shows during a turn: whether it goes
/// to stdout, and the text.
static HELD: Mutex<Option<Vec<(bool, String)>>> = Mutex::new(None);

/// Writes inline output to stdout, where replies go, or stderr, unless the view holds it.
pub(super) fn print(stdout: bool, text: &str) {
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    match &mut *held {
        Some(held) => held.push((stdout, text.to_owned())),
        None => write(stdout, text),
    }
}

fn write(stdout: bool, text: &str) {
    let _ = match stdout {
        true => {
            let mut out = io::stdout().lock();
            out.write_all(text.as_bytes()).and_then(|()| out.flush())
        }
        false => io::stderr().lock().write_all(text.as_bytes()),
    };
}

/// Holds inline output until dropped, then writes it.
struct Hold;

impl Hold {
    fn start() -> Self {
        *HELD.lock().unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
        Self
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        // Written under the lock, so output printed meanwhile comes after it.
        let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        for (stdout, text) in held.take().into_iter().flatten() {
            write(stdout, &text);
        }
    }
}

/// Where to scroll to once the text is rendered again with `count` lines and `rows` shown: the
/// end if it was there, else as far into the text as before.
fn position(top: usize, last: usize, count: usize) -> impl Fn(usize, usize) -> usize {
    move |new_count, rows| match top >= last {
        true => new_count.saturating_sub(rows),
        false => top * new_count / count.max(1),
    }
}

/// The alternate screen, with the cursor hidden and lines cut at the edge rather than wrapped,
/// until dropped.
struct Screen;

impl Screen {
    fn enter() -> Self {
        eprint!("\x1b[?1049h\x1b[?25l\x1b[?7l");
        Self
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        eprint!("\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = io::stderr().flush();
    }
}

fn draw(
    text: &Text,
    top: usize,
    rows: usize,
    width: usize,
    note: Option<&str>,
    activity: Option<&str>,
) -> io::Result<()> {
    let mut frame = String::from("\x1b[H");
    for row in 0..rows {
        if let Some(line) = text.get(top + row) {
            frame += line;
        }
        frame += "\x1b[K\r\n";
    }
    let mut status = match text.len() {
        0 => "No messages yet".to_owned(),
        count => format!("{}–{} of {count}", top + 1, (top + rows).min(count)),
    };
    if let Some(activity) = activity {
        status += &format!(" · {activity}");
    }
    status += &format!(" · {}", note.unwrap_or(KEYS));
    let status = clip(&status, width.saturating_sub(1));
    frame += &format!("{}{status}{}\x1b[K", style::MUTED, style::RESET);
    let mut stderr = io::stderr().lock();
    stderr.write_all(frame.as_bytes())?;
    stderr.flush()
}

/// The lines of rendered text, each ending its styles and starting with those still on from the
/// lines before, so that any of them can be drawn first. Only style sequences are kept of the
/// escape sequences, and no other control characters.
fn screen_lines(text: &str) -> Vec<String> {
    let text = text.trim_end_matches('\n');
    if text.is_empty() {
        return Vec::new();
    }
    let mut active = String::new();
    text.split('\n')
        .map(|line| {
            let mut shown = active.clone();
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\x1b' if chars.peek() == Some(&'[') => {
                        chars.next();
                        let mut sequence = String::from("\x1b[");
                        for c in chars.by_ref() {
                            sequence.push(c);
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                        if sequence.ends_with('m') {
                            if sequence == style::RESET || sequence == "\x1b[m" {
                                active.clear();
                            } else {
                                active += &sequence;
                            }
                            shown += &sequence;
                        }
                    }
                    '\t' => shown += "    ",
                    c if c.is_control() => {}
                    c => shown.push(c),
                }
            }
            if !active.is_empty() {
                shown += style::RESET;
            }
            shown
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_the_styles_left_on_and_drop_other_sequences() {
        let text = "\x1b[2mdim\nstill dim\x1b[0m\nplain \x1b[1mbold\x1b[0m\n\x1b[2Jclear\x07\n\n";
        assert_eq!(
            screen_lines(text),
            [
                "\x1b[2mdim\x1b[0m",
                "\x1b[2mstill dim\x1b[0m",
                "plain \x1b[1mbold\x1b[0m",
                "clear",
            ]
        );
        assert!(screen_lines("\n\n").is_empty());
    }

    #[test]
    fn a_running_turn_follows_the_conversation_after_a_blank_line() {
        let lines = |texts: &[&str]| texts.iter().map(|text| text.to_string()).collect();
        let text = Text {
            before: lines(&["> one", "", "reply"]),
            turn: lines(&["> two", "more"]),
        };
        let shown: Vec<_> = (0..7).map(|index| text.get(index)).collect();
        assert_eq!(
            shown,
            [
                Some("> one"),
                Some(""),
                Some("reply"),
                Some(""),
                Some("> two"),
                Some("more"),
                None
            ]
        );
        assert_eq!(text.len(), 6);
        let text = Text {
            before: Vec::new(),
            turn: lines(&["> two"]),
        };
        assert_eq!(
            (text.len(), text.get(0), text.get(1)),
            (1, Some("> two"), None)
        );
    }

    #[test]
    fn a_rendering_again_keeps_the_end_or_the_place() {
        assert_eq!(position(80, 80, 100)(150, 20), 130);
        assert_eq!(position(40, 80, 100)(200, 20), 80);
        assert_eq!(position(0, 80, 100)(200, 20), 0);
    }
}
