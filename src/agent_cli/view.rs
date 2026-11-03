//! The conversation in the alternate screen with every tool result in full, as Scramjet's
//! Ctrl+O expands its tool blocks, scrolled with the keys while the prompt waits underneath.

use super::editor::{Key, RESIZE_CHECK, RawMode, Stdin, read_key, terminal_size};
use super::{clip, editor, style};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

const KEYS: &str = "↑↓ PgUp PgDn Home End scroll · Ctrl+T thinking · Esc close";

/// Shows the text `render(width)` returns for the terminal's width, from its end, until Escape,
/// `q`, Ctrl+O, Ctrl-C or Ctrl-D, taking keys from `typed` first. Ctrl+T flips `hide_thinking`
/// and renders it again. Returns the keys typed after the one that closed it.
pub(super) fn show(
    render: impl Fn(usize) -> String,
    hide_thinking: &AtomicBool,
    typed: Vec<u8>,
) -> io::Result<Vec<u8>> {
    let raw = RawMode::enter()?;
    let mut screen = Screen::enter();
    let mut source = Stdin::starting_with(typed);
    let (mut width, mut height) = terminal_size();
    let mut lines = screen_lines(&render(width));
    let mut top = usize::MAX;
    let mut note: Option<&str> = None;
    let mut redraw = true;
    loop {
        let rows = height.saturating_sub(1).max(1);
        let last = lines.len().saturating_sub(rows);
        top = top.min(last);
        if redraw && !source.pending() {
            draw(&lines, top, rows, width, note.take())?;
            redraw = false;
        }
        let key = match read_key(&mut source, Some(RESIZE_CHECK)) {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(Vec::new()),
            result => result?,
        };
        let page = rows.saturating_sub(1).max(1);
        top = match key {
            Key::Idle if super::job::continued() => top,
            Key::Idle => {
                let size = terminal_size();
                if size == (width, height) {
                    continue;
                }
                let keep = position(top, last, lines.len());
                (width, height) = size;
                lines = screen_lines(&render(width));
                keep(lines.len(), height.saturating_sub(1).max(1))
            }
            Key::Suspend => {
                drop(screen);
                raw.suspend();
                super::job::suspend();
                super::job::continued();
                raw.resume()?;
                screen = Screen::enter();
                top
            }
            Key::ToggleThinking => {
                let hidden = !hide_thinking.fetch_xor(true, Ordering::Relaxed);
                note = Some(editor::thinking_note(hidden));
                let keep = position(top, last, lines.len());
                lines = screen_lines(&render(width));
                keep(lines.len(), rows)
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
    lines: &[String],
    top: usize,
    rows: usize,
    width: usize,
    note: Option<&str>,
) -> io::Result<()> {
    let mut frame = String::from("\x1b[H");
    for row in 0..rows {
        if let Some(line) = lines.get(top + row) {
            frame += line;
        }
        frame += "\x1b[K\r\n";
    }
    let shown = match lines.len() {
        0 => "No messages yet".to_owned(),
        count => format!("{}–{} of {count}", top + 1, (top + rows).min(count)),
    };
    let status = match note {
        Some(note) => format!("{shown} · {note}"),
        None => format!("{shown} · {KEYS}"),
    };
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
    fn a_rendering_again_keeps_the_end_or_the_place() {
        assert_eq!(position(80, 80, 100)(150, 20), 130);
        assert_eq!(position(40, 80, 100)(200, 20), 80);
        assert_eq!(position(0, 80, 100)(200, 20), 0);
    }
}
