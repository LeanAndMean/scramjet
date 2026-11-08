//! The conversation the screen shows, in parts that each render on their own.

use crate::agent_cli::banner::Banner;
use crate::agent_cli::design::Design;
use crate::agent_cli::record::{self, Event};
use crate::agent_cli::selection::{self, Line};
use crate::agent_cli::settings::MarkdownReads;
use crate::agent_cli::terminal::Terminal;
use scramjet_rs::agent::{Purpose, StreamProgress};
use std::sync::{Arc, atomic::AtomicBool};

/// How the conversation is rendered.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Look {
    pub(super) width: usize,
    pub(super) hide_thinking: bool,
    /// Tool results are shown whole.
    pub(super) whole: bool,
    pub(super) markdown_reads: MarkdownReads,
    /// `style::generation()`: the theme's.
    pub(super) theme: usize,
    pub(super) design: Design,
    pub(super) banner: Banner,
}

/// A line of the conversation by its part, so that it stays in view as parts before it or it
/// render to more or fewer lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Place {
    part: usize,
    line: usize,
}

impl Place {
    /// The conversation's first line.
    pub(super) const START: Place = Place { part: 0, line: 0 };
}

/// The conversation shown, in parts that each render on their own: a turn, or what came
/// between turns. Each part is rendered again only when it or the look changes.
#[derive(Default)]
pub(super) struct Transcript {
    parts: Vec<Part>,
    /// Where the calls running at the same time are, by part and event: what the session says
    /// while they run, such as a command's result, shows under them.
    calls: Option<(usize, usize)>,
    /// Where the turn's latest reply has got to, by part and event: what the session says
    /// while it streams shows under it.
    reply: Option<(usize, usize)>,
    /// Shown after the parts until the session adds them: the `!` commands that ended during
    /// the turn, by their serial numbers.
    tail: (Vec<usize>, Part),
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
        let calls = matches!(event, Event::Calls(_));
        let streamed = matches!(
            event,
            Event::Progress(_) | Event::Retrying(..) | Event::Cooling(_) | Event::Assistant(_)
        );
        if streamed && let Some((part, at)) = self.reply {
            return self.insert(part, at, event);
        }
        let replying = matches!(event, Event::Waiting(Purpose::Reply));
        let index = self.parts.len() - 1;
        let part = &mut self.parts[index];
        part.version += 1;
        add(&mut part.events, event);
        if calls {
            self.calls = Some((index, part.events.len() - 1));
        }
        if replying {
            self.reply = Some((index, part.events.len() - 1));
        }
    }

    /// The turn ended: what follows is no reply's.
    pub(super) fn turn_ended(&mut self) {
        self.reply = None;
    }

    /// Adds `event` after event `at` of part `part`, the reply's latest.
    fn insert(&mut self, part: usize, at: usize, event: Event) {
        let events = &mut self.parts[part].events;
        let (before, after) = (events.len(), events.split_off(at + 1));
        add(events, event);
        let added = events.len() + after.len() - before;
        self.reply = Some((part, events.len() - 1));
        events.extend(after);
        self.parts[part].version += 1;
        if let Some((calls, index)) = &mut self.calls
            && *calls == part
            && *index > at
        {
            *index += added;
        }
    }

    /// Adds `event` to call `index` of the calls running at the same time, shown last.
    pub(super) fn push_call(&mut self, index: usize, event: Event) {
        match self.calls_mut().and_then(|calls| calls.get_mut(index)) {
            Some(events) => add(events, event),
            None => return self.push(event),
        }
        let (part, _) = self.calls.expect("has the calls");
        self.parts[part].version += 1;
    }

    /// The calls running at the same time last.
    pub(super) fn calls(&self) -> Option<&[Vec<Event>]> {
        let (part, event) = self.calls?;
        match self.parts.get(part)?.events.get(event)? {
            Event::Calls(calls) => Some(calls),
            _ => None,
        }
    }

    fn calls_mut(&mut self) -> Option<&mut Vec<Vec<Event>>> {
        let (part, event) = self.calls?;
        match self.parts.get_mut(part)?.events.get_mut(event)? {
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

    /// The serial numbers of the `!` commands `tail` shows.
    pub(super) fn tailed(&self) -> &[usize] {
        &self.tail.0
    }

    /// Shows `events` after the conversation: the `!` commands numbered `serials`.
    pub(super) fn tail(&mut self, serials: Vec<usize>, events: Vec<Event>) {
        self.tail.0 = serials;
        self.tail.1.events = events;
        self.tail.1.version += 1;
    }

    fn all(&self) -> impl Iterator<Item = &Part> {
        self.parts.iter().chain([&self.tail.1])
    }

    pub(super) fn render(&mut self, look: Look, hide_thinking: &Arc<AtomicBool>) {
        for part in self.parts.iter_mut().chain([&mut self.tail.1]) {
            if part.rendered != Some((look, part.version)) {
                part.lines = render(&part.events, look, hide_thinking.clone());
                part.rendered = Some((look, part.version));
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        self.all().map(|part| part.lines.len()).sum()
    }

    /// The part that line `line` is in, and where in it.
    pub(super) fn locate(&self, mut line: usize) -> Place {
        for (part, lines) in self.all().map(|part| part.lines.len()).enumerate() {
            if line < lines {
                return Place { part, line };
            }
            line -= lines;
        }
        Place {
            part: self.parts.len() + 1,
            line,
        }
    }

    /// The line at `place`, as near as the part's lines now reach.
    pub(super) fn line(&self, place: Place) -> usize {
        let before: usize = (self.all().take(place.part))
            .map(|part| part.lines.len())
            .sum();
        let within = match self.all().nth(place.part) {
            Some(part) => place.line.min(part.lines.len().saturating_sub(1)),
            None => place.line,
        };
        before + within
    }

    /// `count` lines from `top`, as far as there are.
    pub(super) fn window(&self, top: usize, count: usize) -> Vec<Line> {
        let lines = self.all().flat_map(|part| &part.lines);
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
    ui.design = look.design;
    ui.banner = look.banner;
    ui.outcomes = record::outcomes(events).into();
    for (index, event) in events.iter().enumerate() {
        record::replay(event, &mut ui);
        // Calls still running end their block before what was said under them.
        if let Event::Calls(calls) = event
            && index + 1 < events.len()
            && (calls.iter().rfind(|events| !events.is_empty()))
                .is_some_and(|events| !record::ended(events))
        {
            ui.blank_line();
        }
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
    use crate::agent_cli::design::Outcome;
    use crate::agent_cli::style::{self, Band};
    use crate::agent_cli::text::plain;
    use std::time::{Duration, Instant};

    /// Rendered `width` columns wide, the tool results `whole` or cut.
    fn look(width: usize, whole: bool) -> Look {
        Look {
            width,
            hide_thinking: false,
            whole,
            markdown_reads: MarkdownReads::Rendered,
            theme: 0,
            design: Design::Classic,
            banner: Banner::Plain,
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
    fn a_streaming_reply_goes_on_in_its_place_above_what_is_said_meanwhile() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::Waiting(Purpose::Reply));
        transcript.push(Event::Progress(StreamProgress::Text("First ".into())));
        transcript.push(Event::Text("Autopilot is on".into()));
        transcript.split();
        transcript.push(Event::Progress(StreamProgress::Text(
            "line.\n\nSecond.".into(),
        )));
        transcript.push(Event::Assistant("First line.\n\nSecond.".into()));
        transcript.render(look(40, false), &hide);
        let lines = transcript.window(0, 100).into_iter();
        let shown: Vec<_> = lines
            .map(|line| plain(&line.shown).trim().to_owned())
            .collect();
        assert_eq!(
            shown,
            ["First line.", "", "Second.", "", "Autopilot is on", ""]
        );
        assert_eq!(text(&transcript.parts[0]), ["First line.\n\nSecond."]);
        // The next reply starts under it all.
        transcript.push(Event::Waiting(Purpose::Reply));
        transcript.push(Event::Progress(StreamProgress::Text("Next.".into())));
        assert_eq!(text(&transcript.parts[1]), ["Next."]);
    }

    #[test]
    fn the_tail_stays_under_what_is_added_until_it_is_taken() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        let shown = |transcript: &mut Transcript| -> Vec<String> {
            transcript.render(look(40, false), &hide);
            let lines = transcript.window(0, 100).into_iter();
            lines
                .map(|line| plain(&line.shown).trim().to_owned())
                .collect()
        };
        transcript.push(Event::Text("Before".into()));
        transcript.tail(vec![1], vec![Event::Text("Ended".into())]);
        transcript.push(Event::Text("After".into()));
        let all = ["Before", "", "After", "", "Ended", ""];
        assert_eq!(shown(&mut transcript), all);
        let place = transcript.locate(4);
        assert_eq!(transcript.line(place), 4);
        assert_eq!(transcript.tailed(), [1]);
        // Taken into the conversation, it shows where it was.
        transcript.tail(Vec::new(), Vec::new());
        transcript.push(Event::Text("Ended".into()));
        assert_eq!(shown(&mut transcript), all);
    }

    #[test]
    fn running_calls_go_on_in_their_place_under_what_is_said_meanwhile() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::Calls(vec![Vec::new()]));
        transcript.push_call(0, Event::ToolCall("bash".into(), "make".into(), None));
        transcript.push_call(0, Event::ToolOutput("building".into()));
        transcript.push(Event::Text("Autopilot is on".into()));
        transcript.split();
        transcript.push_call(0, Event::ToolOutput("building\nbuilt".into()));
        transcript.render(look(40, false), &hide);
        let lines = transcript.window(0, 100).into_iter();
        let shown: Vec<_> = lines
            .map(|line| plain(&line.shown).trim().to_owned())
            .collect();
        assert_eq!(
            shown,
            [
                "● bash make",
                "building",
                "built",
                "",
                "Autopilot is on",
                ""
            ]
        );
        assert!(transcript.calls().is_some());
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
        // What follows the calls comes after them all, a running one's block ended first.
        transcript.push(Event::Assistant("Done.".into()));
        assert_eq!(shown(&mut transcript)[ended.len() - 1..], ["", "Done.", ""]);
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
        // Previews wrap long lines too, and a copy joins them again.
        transcript.render(look(24, false), &hide);
        let lines = transcript.window(0, 100);
        let first = lines
            .iter()
            .position(|line| plain(&line.shown) == "  test a ... ok");
        let first = first.unwrap_or_else(|| panic!("{lines:?}"));
        let end = lines
            .iter()
            .rposition(|line| !line.shown.is_empty())
            .unwrap();
        let rows = &lines[first..=end];
        assert!(rows.len() > 2, "{rows:?}");
        assert_eq!(selection::text(rows, 0, usize::MAX), output);
    }

    /// A turn that thinks, runs a command that fails after two seconds, and replies, then a
    /// prompt that leaves a call running; as each look shows it, 40 columns wide, its rows
    /// without styles and the bands they are on.
    fn shown_in(design: Design) -> Vec<(String, Option<Band>)> {
        let styled = styled_in(design).into_iter();
        styled.map(|(row, band)| (plain(&row), band)).collect()
    }

    fn styled_in(design: Design) -> Vec<(String, Option<Band>)> {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        let started = Instant::now();
        transcript.push(Event::UserPrompt("fix the test".into()));
        let thought = "Looking at it.\n\n";
        transcript.push(Event::Progress(StreamProgress::Reasoning(thought.into())));
        transcript.push(Event::ToolCall("bash".into(), "cargo test".into(), None));
        transcript.push(Event::ToolRunning(started));
        let output = "1 failed\n\nCommand exited with code 101";
        let ended = started + Duration::from_secs(2);
        let result = Event::ToolResult("bash".into(), output.into(), None, true, ended);
        transcript.push(result);
        transcript.push(Event::Progress(StreamProgress::Text("It fails.".into())));
        transcript.push(Event::Assistant("It fails.".into()));
        transcript.split();
        transcript.push(Event::UserPrompt("again".into()));
        transcript.push(Event::ToolCall("bash".into(), "make".into(), None));
        transcript.push(Event::ToolRunning(started));
        transcript.push(Event::ToolOutput("building".into()));
        let look = Look {
            design,
            ..look(40, false)
        };
        transcript.render(look, &hide);
        let lines = transcript.window(0, 100).into_iter();
        lines.map(|line| (line.shown, line.band)).collect()
    }

    fn rows(shown: &[(String, Option<Band>)]) -> Vec<&str> {
        shown.iter().map(|(row, _)| row.trim_end()).collect()
    }

    #[test]
    fn each_look_shows_prompts_reasoning_calls_and_replies_its_own_way() {
        let classic = shown_in(Design::Classic);
        assert_eq!(
            rows(&classic),
            [
                "> fix the test",
                "",
                "Looking at it.",
                "",
                "● bash cargo test",
                "  1 failed",
                "  Command exited with code 101",
                "  Took 2.0s",
                "",
                "It fails.",
                "",
                "> again",
                "",
                "● bash make",
                "  building",
                "",
            ]
        );
        assert!(classic.iter().all(|(_, band)| band.is_none()));
        assert_eq!(
            rows(&shown_in(Design::Rails)),
            [
                "▌ fix the test",
                "",
                "┊ Looking at it.",
                "",
                "✗ bash cargo test                   2.0s",
                "│ 1 failed",
                "│ Command exited with code 101",
                "",
                "It fails.",
                "",
                "▌ again",
                "",
                "○ bash make",
                "│ building",
                "",
            ]
        );
        assert_eq!(
            rows(&shown_in(Design::Margin)),
            [
                "   you fix the test",
                "",
                " think Looking at it.",
                "",
                "  bash cargo test                   2.0s",
                "     │ 1 failed",
                "     │ Command exited with code 101",
                "",
                " reply It fails.",
                "",
                "   you again",
                "",
                "  bash make",
                "     │ building",
                "",
            ]
        );
        // Panels put prompts on a band of their own, and each call on one for how it went.
        let panels = shown_in(Design::Panels);
        let (prompt, failed, running) =
            (Some(Band::Prompt), Some(Band::Failed), Some(Band::Running));
        assert_eq!(
            panels
                .iter()
                .map(|(row, band)| (row.trim_end(), *band))
                .collect::<Vec<_>>(),
            [
                ("", prompt),
                (" › fix the test", prompt),
                ("", prompt),
                ("", None),
                (" Looking at it.", None),
                ("", None),
                ("", failed),
                (" bash  cargo test                2.0s ✗", failed),
                ("", failed),
                (" 1 failed", failed),
                (" Command exited with code 101", failed),
                ("", failed),
                ("", None),
                (" It fails.", None),
                ("", None),
                ("", prompt),
                (" › again", prompt),
                ("", prompt),
                ("", None),
                ("", running),
                (" bash  make                           ○", running),
                ("", running),
                (" building", running),
                ("", running),
                ("", None),
            ]
        );
    }

    #[test]
    fn a_question_in_a_panel_has_one_pad_above_and_one_below_while_asked_and_answered() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        let call = Event::ToolCall("get_scramjet_user_input".into(), String::new(), None);
        transcript.push(call);
        transcript.push(Event::ToolRunning(Instant::now()));
        transcript.push(Event::Question("Delete it?".into()));
        let shown = |transcript: &mut Transcript| {
            let look = Look {
                design: Design::Panels,
                ..look(40, false)
            };
            transcript.render(look, &hide);
            let lines = transcript.window(0, 100).into_iter();
            let rows = lines.map(|line| (plain(&line.shown).trim_end().to_owned(), line.band));
            rows.filter(|(_, band)| band.is_some()).collect::<Vec<_>>()
        };
        let running = Some(Band::Running);
        assert_eq!(
            shown(&mut transcript),
            [
                ("".into(), running),
                (" get_scramjet_user_input              ○".into(), running),
                ("".into(), running),
                (" Delete it?".into(), running),
                ("".into(), running),
            ]
        );
        let answer = "Answer: Yes".into();
        let result = Event::ToolResult(
            "get_scramjet_user_input".into(),
            answer,
            None,
            false,
            Instant::now(),
        );
        transcript.push(result);
        let done = Some(Band::Done);
        assert_eq!(
            shown(&mut transcript),
            [
                ("".into(), done),
                (" get_scramjet_user_input              ✓".into(), done),
                ("".into(), done),
                (" Delete it?".into(), done),
                ("".into(), done),
                (" Answer: Yes".into(), done),
                ("".into(), done),
            ]
        );
    }

    #[test]
    fn a_failed_calls_output_is_red_but_on_its_panel() {
        let row = |design| {
            let styled = styled_in(design);
            let failed = styled
                .into_iter()
                .find(|(row, _)| plain(row).contains("1 failed"));
            failed.expect("the failed call's output").0
        };
        assert!(row(Design::Rails).contains(style::TOOL_FAILED.get()));
        let panels = row(Design::Panels);
        assert!(panels.contains(style::TOOL_OUTPUT.get()), "{panels:?}");
        assert!(!panels.contains(style::TOOL_FAILED.get()), "{panels:?}");
    }

    #[test]
    fn a_copy_leaves_out_what_a_look_puts_before_the_rows() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        let prompt = "please run the tests,  then say what fails";
        transcript.push(Event::UserPrompt(prompt.into()));
        transcript.push(Event::Progress(StreamProgress::Reasoning(
            "Run them all.".into(),
        )));
        let command = "cargo test --offline --no-fail-fast";
        transcript.push(Event::ToolCall("bash".into(), command.into(), None));
        let output = "test a ... ok\ntest agent::tests::compaction ... FAILED";
        let result = Event::ToolResult("bash".into(), output.into(), None, true, Instant::now());
        transcript.push(result);
        // As a resumed session shows a reply: whole, not streamed.
        let reply = "One fails: compaction, in the agent's tests.";
        transcript.push(Event::Assistant(reply.into()));
        for design in [Design::Rails, Design::Panels, Design::Margin] {
            let look = Look {
                design,
                ..look(24, true)
            };
            transcript.render(look, &hide);
            let lines = transcript.window(0, 100);
            assert_eq!(
                selection::text(&lines[..lines.len() - 1], 0, usize::MAX).trim(),
                format!("{prompt}\n\nRun them all.\n\n{command}\n{output}\n\n{reply}"),
                "{design:?}"
            );
        }
    }

    #[test]
    fn calls_are_found_in_the_order_they_show_with_how_they_went() {
        let started = Instant::now();
        let ended = |failed, seconds| {
            let at = started + Duration::from_secs(seconds);
            Event::ToolResult("bash".into(), String::new(), None, failed, at)
        };
        let call = || Event::ToolCall("bash".into(), "x".into(), None);
        let events = [
            call(),
            Event::ToolRunning(started),
            ended(true, 1),
            Event::Calls(vec![
                vec![call(), ended(false, 0)],
                Vec::new(),
                vec![call(), Event::ToolRunning(started)],
            ]),
            call(),
        ];
        assert_eq!(
            record::outcomes(&events),
            [
                (Outcome::Failed, Some(Duration::from_secs(1))),
                (Outcome::Done, None),
                (Outcome::Running, None),
                (Outcome::Running, None),
            ]
        );
    }
}
