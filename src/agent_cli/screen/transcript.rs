//! The conversation the screen shows, in parts that each render on their own.

use crate::agent_cli::record::{self, Event};
use crate::agent_cli::selection::{self, Line};
use crate::agent_cli::settings::MarkdownReads;
use crate::agent_cli::terminal::Terminal;
use scramjet_rs::agent::StreamProgress;
use std::sync::{Arc, atomic::AtomicBool};

/// How the conversation is rendered.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Look {
    pub(super) width: usize,
    pub(super) hide_thinking: bool,
    /// Tool results are shown whole.
    pub(super) whole: bool,
    pub(super) markdown_reads: MarkdownReads,
}

/// A line of the conversation by its part, so that it stays in view as parts before it or it
/// render to more or fewer lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Place {
    part: usize,
    line: usize,
}

/// The conversation shown, in parts that each render on their own: a turn, or what came
/// between turns. Each part is rendered again only when it or the look changes.
#[derive(Default)]
pub(super) struct Transcript {
    parts: Vec<Part>,
}

#[derive(Default)]
struct Part {
    events: Vec<Event>,
    version: usize,
    lines: Vec<Line>,
    rendered: Option<(Look, usize)>,
}

impl Transcript {
    pub(super) fn push(&mut self, event: Event) {
        if self.parts.is_empty() {
            self.parts.push(Part::default());
        }
        let part = self.parts.last_mut().expect("pushed above");
        part.version += 1;
        add(&mut part.events, event);
    }

    /// Adds `event` to call `index` of the calls running at the same time, shown last.
    pub(super) fn push_call(&mut self, index: usize, event: Event) {
        match self.calls_mut().and_then(|calls| calls.get_mut(index)) {
            Some(events) => add(events, event),
            None => return self.push(event),
        }
        self.parts.last_mut().expect("has the calls").version += 1;
    }

    /// The calls running at the same time, when they are what was shown last.
    pub(super) fn calls(&self) -> Option<&[Vec<Event>]> {
        match self.parts.last()?.events.last()? {
            Event::Calls(calls) => Some(calls),
            _ => None,
        }
    }

    fn calls_mut(&mut self) -> Option<&mut Vec<Vec<Event>>> {
        match self.parts.last_mut()?.events.last_mut()? {
            Event::Calls(calls) => Some(calls),
            _ => None,
        }
    }

    /// Starts a new part, unless the last one is empty.
    pub(super) fn split(&mut self) {
        if self.parts.last().is_none_or(|part| !part.events.is_empty()) {
            self.parts.push(Part::default());
        }
    }

    pub(super) fn render(&mut self, look: Look, hide_thinking: &Arc<AtomicBool>) {
        for part in &mut self.parts {
            if part.rendered != Some((look, part.version)) {
                part.lines = render(&part.events, look, hide_thinking.clone());
                part.rendered = Some((look, part.version));
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        self.parts.iter().map(|part| part.lines.len()).sum()
    }

    /// The part that line `line` is in, and where in it.
    pub(super) fn locate(&self, mut line: usize) -> Place {
        for (part, lines) in self.parts.iter().map(|part| part.lines.len()).enumerate() {
            if line < lines {
                return Place { part, line };
            }
            line -= lines;
        }
        Place {
            part: self.parts.len(),
            line,
        }
    }

    /// The line at `place`, as near as the part's lines now reach.
    pub(super) fn line(&self, place: Place) -> usize {
        let before: usize = (self.parts.iter().take(place.part))
            .map(|part| part.lines.len())
            .sum();
        let within = match self.parts.get(place.part) {
            Some(part) => place.line.min(part.lines.len().saturating_sub(1)),
            None => place.line,
        };
        before + within
    }

    /// `count` lines from `top`, as far as there are.
    pub(super) fn window(&self, top: usize, count: usize) -> Vec<Line> {
        let lines = self.parts.iter().flat_map(|part| &part.lines);
        lines.skip(top).take(count).cloned().collect()
    }

    /// Line `line` as drawn, empty past the end.
    pub(super) fn shown(&self, line: usize) -> String {
        let line = self.window(line, 1).pop();
        line.map(|line| line.shown).unwrap_or_default()
    }
}

/// Adds `event` to `events`, keeping streamed text whole, and only a running tool's latest
/// output and children.
fn add(events: &mut Vec<Event>, event: Event) {
    match (events.last_mut(), event) {
        (Some(Event::ToolOutput(last)), Event::ToolOutput(output)) => *last = output,
        (Some(Event::Subagents(last)), Event::Subagents(shown)) => *last = shown,
        (
            Some(Event::Progress(StreamProgress::Text(last))),
            Event::Progress(StreamProgress::Text(delta)),
        )
        | (
            Some(Event::Progress(StreamProgress::Reasoning(last))),
            Event::Progress(StreamProgress::Reasoning(delta)),
        ) => last.push_str(&delta),
        (_, event) => events.push(event),
    }
}

/// The lines of `events` as `Terminal` renders them at the look's width.
fn render(events: &[Event], look: Look, hide_thinking: Arc<AtomicBool>) -> Vec<Line> {
    let mut ui = Terminal::capture_shown(look.width, hide_thinking, look.whole);
    ui.markdown_reads = look.markdown_reads;
    for event in events {
        record::replay(event, &mut ui);
    }
    record::running(events.last(), &mut ui);
    let mut lines = selection::lines(&ui.captured());
    // Each block ends with a blank line, which separates it from the next part.
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::text::plain;
    use std::time::{Duration, Instant};

    /// Rendered `width` columns wide, the tool results `whole` or cut.
    fn look(width: usize, whole: bool) -> Look {
        Look {
            width,
            hide_thinking: false,
            whole,
            markdown_reads: MarkdownReads::Rendered,
        }
    }

    fn text(part: &Part) -> Vec<&str> {
        part.events
            .iter()
            .filter_map(|event| match event {
                Event::Progress(StreamProgress::Text(text)) | Event::ToolOutput(text) => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streamed_text_and_running_output_are_kept_whole() {
        let mut transcript = Transcript::default();
        transcript.push(Event::Progress(StreamProgress::Text("Hel".into())));
        transcript.push(Event::Progress(StreamProgress::Text("lo".into())));
        transcript.push(Event::ToolRunning(Instant::now()));
        transcript.push(Event::ToolOutput("1".into()));
        transcript.push(Event::ToolOutput("1\n2".into()));
        assert_eq!(text(&transcript.parts[0]), ["Hello", "1\n2"]);
        transcript.split();
        transcript.split();
        assert_eq!(transcript.parts.len(), 2);
    }

    #[test]
    fn calls_running_together_each_show_in_their_place_as_they_run() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::UserPrompt("both".into()));
        transcript.push(Event::Calls(vec![Vec::new(), Vec::new(), Vec::new()]));
        let ago = |seconds| {
            Instant::now()
                .checked_sub(Duration::from_secs(seconds))
                .unwrap()
        };
        for (index, command) in ["seq 1 2", "seq 3 4"].into_iter().enumerate() {
            transcript.push_call(index, Event::ToolCall("bash".into(), command.into(), None));
            transcript.push_call(index, Event::ToolRunning(ago(5)));
        }
        transcript.push_call(0, Event::ToolOutput("1".into()));
        transcript.push_call(1, Event::ToolOutput("3".into()));
        transcript.push_call(0, Event::ToolOutput("1\n2".into()));
        let shown = |transcript: &mut Transcript| -> Vec<String> {
            transcript.render(look(40, false), &hide);
            let lines = transcript.window(0, 100).into_iter();
            lines
                .map(|line| plain(&line.shown).trim().to_owned())
                .collect()
        };
        // The third call has not started: it shows nothing yet.
        let running = ["● bash seq 1 2", "1", "2", "", "● bash seq 3 4", "3", ""];
        assert_eq!(shown(&mut transcript)[2..], running);
        let result = Event::ToolResult("bash".into(), "1\n2\n".into(), None, false, ago(3));
        transcript.push_call(0, result);
        let ended = shown(&mut transcript);
        // It took what it took when it ended, however late it renders.
        assert_eq!(ended[2..6], ["● bash seq 1 2", "1", "2", "Took 2.0s"]);
        assert_eq!(ended[ended.len() - 4..], ["", "● bash seq 3 4", "3", ""]);
        // What follows the calls comes after them all.
        transcript.push(Event::Assistant("Done.".into()));
        assert_eq!(shown(&mut transcript)[ended.len() - 1..], ["Done.", ""]);
    }

    #[test]
    fn parts_render_again_only_when_they_or_the_look_change() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::UserPrompt("count".into()));
        transcript.push(Event::ToolCall("bash".into(), "seq 1 9".into(), None));
        let output: Vec<_> = (1..=9).map(|n| n.to_string()).collect();
        transcript.push(Event::ToolResult(
            "bash".into(),
            output.join("\n"),
            None,
            false,
            Instant::now(),
        ));
        transcript.split();
        transcript.push(Event::Text("✓ done".into()));
        transcript.render(look(40, false), &hide);
        let window = |transcript: &Transcript, top, count| -> Vec<String> {
            let lines = transcript.window(top, count).into_iter();
            lines.map(|line| line.shown).collect()
        };
        let cut = window(&transcript, 0, 100);
        assert!(
            cut.iter()
                .any(|line| line.contains("earlier lines, Ctrl+O to expand")),
            "{cut:?}"
        );
        assert_eq!(cut[cut.len() - 2..], ["✓ done", ""]);
        transcript.parts[0].lines.clear();
        transcript.render(look(40, false), &hide);
        assert!(transcript.parts[0].lines.is_empty());
        transcript.render(look(40, true), &hide);
        let whole = window(&transcript, 0, 100);
        assert!(whole.len() > cut.len() && !whole.iter().any(|line| line.contains("earlier")));
        assert_eq!(transcript.len(), whole.len());
        assert_eq!(window(&transcript, 2, 1), whole[2..3]);
        // A line of the part after the result stays in view as the result grows.
        let done = Place { part: 1, line: 0 };
        assert_eq!(transcript.locate(whole.len() - 2), done);
        assert_eq!(transcript.line(done), whole.len() - 2);
        transcript.render(look(40, false), &hide);
        assert_eq!(transcript.line(done), cut.len() - 2);
        assert_eq!(window(&transcript, transcript.line(done), 1), ["✓ done"]);
    }

    #[test]
    fn a_copy_holds_what_was_written_rather_than_how_it_wrapped() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        let prompt = "please run the tests,  then say what fails";
        transcript.push(Event::UserPrompt(prompt.into()));
        let reply = "Run `cargo test --offline --no-fail-fast` to see:\n\n- every failing test with its output";
        transcript.push(Event::Progress(StreamProgress::Text(reply.into())));
        transcript.push(Event::Assistant(reply.into()));
        let command = "cargo test --offline --no-fail-fast";
        transcript.push(Event::ToolCall("bash".into(), command.into(), None));
        let output = "test a ... ok\ntest agent::tests::compaction ... FAILED";
        transcript.push(Event::ToolResult(
            "bash".into(),
            output.into(),
            None,
            true,
            Instant::now(),
        ));
        transcript.render(look(24, true), &hide);
        let lines = transcript.window(0, 100);
        assert!(lines.len() > 16, "{lines:?}");
        assert_eq!(
            selection::text(&lines[..lines.len() - 1], 0, usize::MAX),
            format!(
                "{prompt}\n\nRun cargo test --offline --no-fail-fast to see:\n\n\
                - every failing test with its output\n\n● bash {command}\n{output}"
            )
        );
        // Cut previews are copied as shown.
        transcript.render(look(24, false), &hide);
        let lines = transcript.window(0, 100);
        let failed = lines.iter().position(|line| line.shown.contains("agent::"));
        let failed = failed.expect("the failed test's line");
        assert_eq!(
            selection::text(&lines[failed..=failed], 0, usize::MAX),
            "test agent::tests::co…"
        );
    }
}
