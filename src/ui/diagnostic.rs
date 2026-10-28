use super::{Event, Focus, Model};
use std::{fmt, io};

const _: () = assert!(std::mem::size_of::<Record>() <= 192);

const RECORDS: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Comparison {
    #[default]
    Other,
    Seed,
    MatchA,
    MatchAB,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Snapshot {
    selection: Option<((u16, u16), (u16, u16))>,
    editor_bytes: u16,
    caret: u16,
    copy_bytes: u32,
    window: bool,
    editor_focus: bool,
    dragging: bool,
    comparison: Comparison,
}

impl Snapshot {
    fn capture(model: &Model) -> Self {
        Self {
            selection: model.selection,
            editor_bytes: model.editor.len() as u16,
            caret: model.caret as u16,
            copy_bytes: model.copied.len() as u32,
            window: model.focused,
            editor_focus: model.focus == Focus::Editor,
            dragging: model.dragging,
            comparison: match model.editor.as_str() {
                "leftright" => Comparison::Seed,
                "lefthé\n界🙂right" => Comparison::MatchA,
                "lefthé\n界🙂SECOND\néright" => Comparison::MatchAB,
                _ => Comparison::Other,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Ready,
    Text,
    Paste,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Tab,
    Follow,
    Copy,
    Quit,
    Focus,
    Mouse,
    Unknown,
    Resize,
    Error,
    Handoff,
}

#[derive(Clone, Copy, Debug, Default)]
struct Metadata {
    kind: Kind,
    length: u16,
    button: u16,
    x: u16,
    y: u16,
    release: bool,
}

impl Metadata {
    fn event(event: &Event) -> Self {
        let mut metadata = Self::default();
        metadata.kind = match event {
            Event::Text(text) => {
                metadata.length = text.len() as u16;
                Kind::Text
            }
            Event::Paste(text) => {
                metadata.length = text.len() as u16;
                Kind::Paste
            }
            Event::Left => Kind::Left,
            Event::Right => Kind::Right,
            Event::Up => Kind::Up,
            Event::Down => Kind::Down,
            Event::Home => Kind::Home,
            Event::End => Kind::End,
            Event::PageUp => Kind::PageUp,
            Event::PageDown => Kind::PageDown,
            Event::Backspace => Kind::Backspace,
            Event::Delete => Kind::Delete,
            Event::Tab => Kind::Tab,
            Event::Follow => Kind::Follow,
            Event::Copy => Kind::Copy,
            Event::Quit => Kind::Quit,
            Event::Focus(_) => Kind::Focus,
            Event::Mouse {
                button,
                x,
                y,
                release,
            } => {
                metadata.button = *button;
                metadata.x = *x;
                metadata.y = *y;
                metadata.release = *release;
                Kind::Mouse
            }
            Event::Unknown
            | Event::CancelAll
            | Event::NextChild
            | Event::CancelChild
            | Event::Approve
            | Event::Deny
            | Event::ApprovalNext
            | Event::ApprovalPrevious
            | Event::HistoryPrevious
            | Event::Submit
            | Event::HistoryJourney => Kind::Unknown,
        };
        metadata
    }

    fn motion(self) -> bool {
        self.kind == Kind::Mouse && self.button & 32 != 0 && !self.release
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CopyOutcome {
    #[default]
    NotCopy,
    Noop,
    Ignored,
    Sent,
    Failed,
}

#[derive(Clone, Copy, Default)]
struct Painted {
    sequence: u64,
    width: u16,
    height: u16,
    copy_bytes: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Record {
    sequence: u64,
    count: u64,
    metadata: Metadata,
    before: Snapshot,
    after: Snapshot,
    painted: Painted,
    copy: CopyOutcome,
}

pub(super) struct Diagnostic {
    records: [Option<Record>; RECORDS],
    start: usize,
    length: usize,
    bytes_read: u64,
    bytes_fed: u64,
    events: u64,
    sequence: u64,
    overwritten: u64,
    saturated: bool,
    current: Snapshot,
    painted: Painted,
    last: Kind,
    sent_bytes: u64,
}

fn increment(counter: &mut u64, amount: u64, saturated: &mut bool) {
    match counter.checked_add(amount) {
        Some(value) => *counter = value,
        None => {
            *counter = u64::MAX;
            *saturated = true;
        }
    }
}

impl Diagnostic {
    pub(super) fn new(model: &Model) -> Self {
        Self {
            records: [None; RECORDS],
            start: 0,
            length: 0,
            bytes_read: 0,
            bytes_fed: 0,
            events: 0,
            sequence: 0,
            overwritten: 0,
            saturated: false,
            current: Snapshot::capture(model),
            painted: Painted::default(),
            last: Kind::Ready,
            sent_bytes: 0,
        }
    }

    pub(super) fn read(&mut self, bytes: usize) {
        increment(&mut self.bytes_read, bytes as u64, &mut self.saturated);
    }

    pub(super) fn fed(&mut self) {
        increment(&mut self.bytes_fed, 1, &mut self.saturated);
    }

    pub(super) fn before(&mut self, event: &Event, model: &Model) -> Record {
        increment(&mut self.events, 1, &mut self.saturated);
        Record {
            sequence: 0,
            count: 1,
            metadata: Metadata::event(event),
            before: Snapshot::capture(model),
            after: Snapshot::default(),
            painted: self.painted,
            copy: CopyOutcome::NotCopy,
        }
    }

    pub(super) fn after(&mut self, mut record: Record, model: &Model, copy: CopyOutcome) {
        record.after = Snapshot::capture(model);
        record.copy = copy;
        if copy == CopyOutcome::Sent {
            increment(
                &mut self.sent_bytes,
                record.before.copy_bytes as u64,
                &mut self.saturated,
            );
        }
        self.push(record);
    }

    fn push(&mut self, mut record: Record) {
        increment(&mut self.sequence, 1, &mut self.saturated);
        record.sequence = self.sequence;
        self.current = record.after;
        self.last = record.metadata.kind;
        if self.length != 0 && record.metadata.motion() {
            let index = (self.start + self.length - 1) % RECORDS;
            let previous = self.records[index].as_mut().unwrap();
            if previous.metadata.motion()
                && previous.sequence.checked_add(1) == Some(record.sequence)
            {
                increment(&mut previous.count, 1, &mut self.saturated);
                previous.sequence = record.sequence;
                previous.metadata = record.metadata;
                previous.after = record.after;
                previous.painted = record.painted;
                return;
            }
        }
        let index = (self.start + self.length) % RECORDS;
        self.records[index] = Some(record);
        if self.length == RECORDS {
            self.start = (self.start + 1) % RECORDS;
            increment(&mut self.overwritten, 1, &mut self.saturated);
        } else {
            self.length += 1;
        }
    }

    pub(super) fn resize(&mut self, model: &Model, width: u16, height: u16) -> Record {
        Record {
            sequence: 0,
            count: 1,
            metadata: Metadata {
                kind: Kind::Resize,
                x: width,
                y: height,
                ..Metadata::default()
            },
            before: Snapshot::capture(model),
            after: Snapshot::default(),
            painted: self.painted,
            copy: CopyOutcome::NotCopy,
        }
    }

    pub(super) fn displayed(&mut self, model: &Model) {
        increment(&mut self.painted.sequence, 1, &mut self.saturated);
        self.painted.width = model.display.width;
        self.painted.height = model.display.height;
        self.painted.copy_bytes = model.copied.len() as u32;
        self.current = Snapshot::capture(model);
    }

    pub(super) fn finish(&mut self, model: &Model, failed: bool) {
        for kind in [
            if failed { Kind::Error } else { Kind::Handoff },
            Kind::Handoff,
        ] {
            if kind == Kind::Handoff && self.last == Kind::Handoff {
                continue;
            }
            self.push(Record {
                sequence: 0,
                count: 1,
                metadata: Metadata {
                    kind,
                    ..Metadata::default()
                },
                before: self.current,
                after: Snapshot::capture(model),
                painted: self.painted,
                copy: CopyOutcome::NotCopy,
            });
        }
    }

    pub(super) fn footer(&self) -> Inline<192> {
        let mut text = Inline::new();
        let _ = fmt::write(
            &mut text,
            format_args!(
                "diag e={} {:?} w={} f={} d={} s={:?} p={} c={} sent={} ev={} sat={}",
                self.events,
                self.last,
                self.current.window as u8,
                self.current.editor_focus as u8,
                self.current.dragging as u8,
                self.current.selection,
                self.painted.sequence,
                self.painted.copy_bytes,
                self.sent_bytes,
                self.overwritten,
                self.saturated as u8
            ),
        );
        text
    }

    pub(super) fn paint_footer(
        &self,
        model: &Model,
        display: &super::Display,
        buffer: &mut super::Buffer,
    ) {
        if display.width < 10 || display.height < 8 {
            return;
        }
        let footer = self.footer();
        let mut status = Inline::<64>::new();
        let _ = fmt::write(
            &mut status,
            format_args!(
                "{} | ^Q quit",
                if display.exhausted {
                    "display capped"
                } else {
                    model.status
                }
            ),
        );
        for (row, text) in [
            (display.height - 2, footer.as_str()),
            (display.height - 1, status.as_str()),
        ] {
            for column in 0..display.width {
                buffer[(column, row)].set_symbol(" ");
            }
            buffer.set_stringn(
                0,
                row,
                text,
                display.width as usize,
                super::Style::default(),
            );
            if text.len() > display.width as usize {
                buffer[(display.width - 1, row)].set_symbol("~");
            }
        }
    }

    pub(super) fn report(
        &self,
        output: &mut impl io::Write,
        execution: &str,
        restored: bool,
        cursor_restored: bool,
    ) -> io::Result<()> {
        let mut summary = Inline::<2048>::new();
        let _ = fmt::write(
            &mut summary,
            format_args!(
                "UI-DIAGNOSTIC metadata-only; no native receipt/binding inference\nsummary read={} fed={} events={} records={} overwritten={} saturated={} frame={} size={}x{} committed={} sent={} window={} editor_focus={} dragging={} selection={:?} editor_bytes={} caret={} comparison={:?} execution={} restore={} cursor={} terminal_dropped=1\n",
                self.bytes_read,
                self.bytes_fed,
                self.events,
                self.length,
                self.overwritten,
                self.saturated as u8,
                self.painted.sequence,
                self.painted.width,
                self.painted.height,
                self.painted.copy_bytes,
                self.sent_bytes,
                self.current.window as u8,
                self.current.editor_focus as u8,
                self.current.dragging as u8,
                self.current.selection,
                self.current.editor_bytes,
                self.current.caret,
                self.current.comparison,
                execution,
                restored as u8,
                cursor_restored as u8
            ),
        );
        output.write_all(summary.as_str().as_bytes())?;
        for offset in 0..self.length {
            let record = self.records[(self.start + offset) % RECORDS].unwrap();
            let mut line = Inline::<256>::new();
            let _ = fmt::write(
                &mut line,
                format_args!(
                    "record seq={} n={} {:?} len={} b={} mods={} xy={},{} rel={} before={}/{}/{}/{:?}/{}/{} after={}/{}/{}/{:?}/{}/{} cmp={:?} paint={}:{}x{}:{} copy={:?}",
                    record.sequence,
                    record.count,
                    record.metadata.kind,
                    record.metadata.length,
                    record.metadata.button,
                    record.metadata.button & (4 | 8 | 16),
                    record.metadata.x,
                    record.metadata.y,
                    record.metadata.release as u8,
                    record.before.window as u8,
                    record.before.editor_focus as u8,
                    record.before.dragging as u8,
                    record.before.selection,
                    record.before.editor_bytes,
                    record.before.caret,
                    record.after.window as u8,
                    record.after.editor_focus as u8,
                    record.after.dragging as u8,
                    record.after.selection,
                    record.after.editor_bytes,
                    record.after.caret,
                    record.after.comparison,
                    record.painted.sequence,
                    record.painted.width,
                    record.painted.height,
                    record.painted.copy_bytes,
                    record.copy
                ),
            );
            line.end_line();
            output.write_all(line.as_str().as_bytes())?;
        }
        output.flush()
    }
}

pub(super) struct Inline<const N: usize> {
    bytes: [u8; N],
    length: usize,
    clipped: bool,
}

impl<const N: usize> Inline<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
            clipped: false,
        }
    }
    pub(super) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.length]).unwrap()
    }
    fn end_line(&mut self) {
        if self.length == N {
            let mut keep = N - 2;
            while !self.as_str().is_char_boundary(keep) {
                keep -= 1;
            }
            self.length = keep;
            self.bytes[self.length] = b'~';
            self.length += 1;
        }
        self.bytes[self.length] = b'\n';
        self.length += 1;
    }
}

impl<const N: usize> fmt::Write for Inline<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.clipped {
            return Err(fmt::Error);
        }
        let remaining = N - self.length;
        if text.len() <= remaining {
            self.bytes[self.length..self.length + text.len()].copy_from_slice(text.as_bytes());
            self.length += text.len();
            return Ok(());
        }
        let mut keep = remaining.saturating_sub(1).min(text.len());
        while !text.is_char_boundary(keep) {
            keep -= 1;
        }
        self.bytes[self.length..self.length + keep].copy_from_slice(&text.as_bytes()[..keep]);
        self.length += keep;
        if self.length == N {
            let mut keep = N - 1;
            while !self.as_str().is_char_boundary(keep) {
                keep -= 1;
            }
            self.length = keep;
        }
        self.bytes[self.length] = b'~';
        self.length += 1;
        self.clipped = true;
        Err(fmt::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Buffer, Decoder, Rect, dispatch, draw};
    use std::time::Instant;

    fn paint(model: &mut Model, observer: &mut Option<Diagnostic>) -> Buffer {
        let display = model.project(80, 24);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        model.paint(&display, &mut buffer);
        if let Some(observer) = observer {
            observer.paint_footer(model, &display, &mut buffer);
        }
        model.displayed(display);
        if let Some(observer) = observer {
            observer.displayed(model);
        }
        buffer
    }

    fn feed(
        model: &mut Model,
        observer: &mut Option<Diagnostic>,
        bytes: &[u8],
        output: &mut Vec<u8>,
    ) {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        if let Some(observer) = observer {
            observer.read(bytes.len());
        }
        for byte in bytes {
            if let Some(observer) = observer {
                observer.fed();
            }
            if let Some(event) = decoder.feed(*byte, now).unwrap() {
                assert!(!dispatch(model, observer, event, output).unwrap());
            }
        }
    }

    fn last(observer: &Option<Diagnostic>) -> Record {
        let observer = observer.as_ref().unwrap();
        observer.records[(observer.start + observer.length - 1) % RECORDS].unwrap()
    }

    #[test]
    fn decoder_dispatch_and_display_equivalence() {
        let mut observed = Model::new();
        let mut normal = Model::new();
        let mut observer = Some(Diagnostic::new(&observed));
        let mut off = None;
        let mut observed_copy = Vec::new();
        let mut normal_copy = Vec::new();
        paint(&mut observed, &mut observer);
        paint(&mut normal, &mut off);
        for round in 0..3 {
            let previous = observed.copied.clone();
            let drag = b"\x1b[<0;1;2M\x1b[<32;16;3M\x1b[<0;16;3m";
            for (model, diagnostic, output) in [
                (&mut observed, &mut observer, &mut observed_copy),
                (&mut normal, &mut off, &mut normal_copy),
            ] {
                feed(model, diagnostic, drag, output);
                assert_eq!(model.copied, previous);
                feed(model, diagnostic, b"\x1bOQ", output);
            }
            assert_eq!(
                last(&observer).copy,
                if previous.is_empty() {
                    CopyOutcome::Noop
                } else {
                    CopyOutcome::Sent
                }
            );
            let a = paint(&mut observed, &mut observer);
            let b = paint(&mut normal, &mut off);
            for y in 0..22 {
                for x in 0..80 {
                    assert_eq!(a[(x, y)], b[(x, y)]);
                }
            }
            assert_eq!(
                observed.cursor(&observed.display),
                normal.cursor(&normal.display)
            );
            assert_eq!(observed.copied, normal.copied);
            assert!(!observed.copied.is_empty());
            for (model, diagnostic, output) in [
                (&mut observed, &mut observer, &mut observed_copy),
                (&mut normal, &mut off, &mut normal_copy),
            ] {
                model.tick();
                feed(model, diagnostic, b"\x1bOQ", output);
            }
            assert_eq!(last(&observer).copy, CopyOutcome::Sent);
            if round == 1 {
                for (model, diagnostic, output) in [
                    (&mut observed, &mut observer, &mut observed_copy),
                    (&mut normal, &mut off, &mut normal_copy),
                ] {
                    feed(model, diagnostic, b"\x1b[O\x1bOQ\x1b[I", output);
                    paint(model, diagnostic);
                    assert!(model.copied.is_empty());
                }
            }
        }
        assert_eq!(observed_copy, normal_copy);
        assert_eq!(observed.editor, normal.editor);
        assert_eq!(observed.caret, 4);
        assert_eq!(observed.selection, normal.selection);
        assert_eq!(observed.status, normal.status);
        let events = observer.as_ref().unwrap().events;
        feed(&mut observed, &mut observer, b"", &mut observed_copy);
        paint(&mut observed, &mut observer);
        assert_eq!(observer.as_ref().unwrap().events, events);
        feed(
            &mut observed,
            &mut observer,
            b"\x1b[<4;1;2M",
            &mut observed_copy,
        );
        assert_eq!(last(&observer).metadata.button, 4);
        assert!(observed.dragging);
        assert_eq!(last(&observer).after.selection, observed.selection);
        let mut observed = Model::new();
        let mut normal = Model::new();
        let mut observer = Some(Diagnostic::new(&observed));
        let mut off = None;
        let mut a_output = Vec::new();
        let mut b_output = Vec::new();
        for bytes in [
            b"\x1b[<2;1;2M\x1b[<2;1;2m".as_slice(),
            b"\x16",
            b"\x1b[O\x1b[200~SYNTHETIC_IGNORED\x1b[201~\x1b[I",
            "\x1b[200~hé\n界🙂\x1b[201~".as_bytes(),
            "\x1b[200~SECOND\né\x1b[201~".as_bytes(),
            b"\x1b[200~\x03\x1b[201~",
        ] {
            feed(&mut observed, &mut observer, bytes, &mut a_output);
            feed(&mut normal, &mut off, bytes, &mut b_output);
            let a = paint(&mut observed, &mut observer);
            let b = paint(&mut normal, &mut off);
            assert_eq!(Snapshot::capture(&observed), Snapshot::capture(&normal));
            assert_eq!(observed.editor, normal.editor);
            assert_eq!(observed.status, normal.status);
            assert_eq!(
                observed.cursor(&observed.display),
                normal.cursor(&normal.display)
            );
            for y in 0..22 {
                for x in 0..80 {
                    assert_eq!(a[(x, y)], b[(x, y)]);
                }
            }
        }
        assert_eq!(a_output, b_output);
        assert_eq!(observed.editor, "lefthé\n界🙂SECOND\néright");
    }

    #[test]
    fn right_button_focus_unknown_and_two_atomic_pastes() {
        let mut model = Model::new();
        let mut observer = Some(Diagnostic::new(&model));
        let mut output = Vec::new();
        paint(&mut model, &mut observer);
        for (bytes, editor_focus) in [
            (b"\x1b[<2;1;2M\x1b[<2;1;2m".as_slice(), false),
            (b"\x1b[<2;1;20M\x1b[<2;1;20m".as_slice(), true),
        ] {
            feed(&mut model, &mut observer, bytes, &mut output);
            assert_eq!(model.editor, "leftright");
            assert_eq!(model.focus == Focus::Editor, editor_focus);
            assert_eq!(last(&observer).metadata.kind, Kind::Mouse);
        }
        feed(&mut model, &mut observer, b"\x16", &mut output);
        assert_eq!(last(&observer).metadata.kind, Kind::Unknown);
        feed(
            &mut model,
            &mut observer,
            b"\x1b[O\x1b[200~IGNORED\x1b[201~",
            &mut output,
        );
        assert_eq!(last(&observer).metadata.kind, Kind::Paste);
        assert!(!last(&observer).after.window);
        assert_eq!(last(&observer).before, last(&observer).after);
        assert_eq!(model.editor, "leftright");
        feed(&mut model, &mut observer, b"\x1b[I\t", &mut output);
        assert!(model.focus == Focus::Transcript);
        feed(
            &mut model,
            &mut observer,
            "\x1b[200~hé\n界🙂\x1b[201~".as_bytes(),
            &mut output,
        );
        assert_eq!(model.editor, "lefthé\n界🙂right");
        assert_eq!(model.caret, 15);
        assert_eq!(last(&observer).after.comparison, Comparison::MatchA);
        feed(
            &mut model,
            &mut observer,
            "\x1b[200~SECOND\né\x1b[201~".as_bytes(),
            &mut output,
        );
        assert_eq!(model.editor, "lefthé\n界🙂SECOND\néright");
        assert_eq!(model.caret, 24);
        assert_eq!(last(&observer).after.comparison, Comparison::MatchAB);
        let before = Snapshot::capture(&model);
        for bytes in [
            b"\x1b[200~\x03\x1b[201~".to_vec(),
            [b"\x1b[200~".as_slice(), &vec![b'x'; 8192], b"\x1b[201~"].concat(),
        ] {
            feed(&mut model, &mut observer, &bytes, &mut output);
            assert_eq!(Snapshot::capture(&model), before);
            assert_eq!(last(&observer).metadata.kind, Kind::Paste);
            assert_eq!(last(&observer).before, last(&observer).after);
        }
        let resize = observer.as_mut().unwrap().resize(&model, 10, 12);
        model.clear_selection();
        observer
            .as_mut()
            .unwrap()
            .after(resize, &model, CopyOutcome::NotCopy);
        assert!(last(&observer).after.selection.is_none());
        assert!(output.is_empty());
    }

    struct FaultWriter {
        bytes: Vec<u8>,
        fail_write: bool,
        fail_flush: bool,
    }
    impl io::Write for FaultWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::other("synthetic write failure"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::other("synthetic flush failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn copy_draw_and_report_failures_do_not_acknowledge_success() {
        let mut model = Model::new();
        let mut observer = Some(Diagnostic::new(&model));
        paint(&mut model, &mut observer);
        model.selection = Some(((0, 1), (7, 2)));
        paint(&mut model, &mut observer);
        for (fail_write, fail_flush) in [(true, false), (false, true)] {
            let mut writer = FaultWriter {
                bytes: Vec::new(),
                fail_write,
                fail_flush,
            };
            assert!(dispatch(&mut model, &mut observer, Event::Copy, &mut writer).is_err());
            assert_eq!(last(&observer).copy, CopyOutcome::Failed);
            assert_eq!(observer.as_ref().unwrap().sent_bytes, 0);
            assert_ne!(model.status, "copy sent; receipt unverified");
        }
        let writer = FaultWriter {
            bytes: Vec::new(),
            fail_write: true,
            fail_flush: false,
        };
        let mut terminal = super::super::Terminal::with_options(
            super::super::CrosstermBackend::new(writer),
            super::super::TerminalOptions {
                viewport: super::super::Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .unwrap();
        let committed = observer.as_ref().unwrap().painted.sequence;
        let copied = model.copied.clone();
        let display = model.project(80, 24);
        assert!(
            draw(
                &mut terminal,
                &mut model,
                &mut observer,
                display,
                None,
                None,
                None
            )
            .is_err()
        );
        assert_eq!(observer.as_ref().unwrap().painted.sequence, committed);
        assert_eq!(model.copied, copied);
        drop(terminal);
        for (fail_write, fail_flush) in [(true, false), (false, true)] {
            let mut writer = FaultWriter {
                bytes: Vec::new(),
                fail_write,
                fail_flush,
            };
            assert!(
                observer
                    .as_ref()
                    .unwrap()
                    .report(&mut writer, "error", false, false)
                    .is_err()
            );
        }
        let mut report = Vec::new();
        observer
            .as_ref()
            .unwrap()
            .report(&mut report, "error", false, false)
            .unwrap();
        assert!(
            std::str::from_utf8(&report)
                .unwrap()
                .contains("restore=0 cursor=0")
        );
    }

    #[test]
    fn fixed_records_coalescing_eviction_secrecy_and_saturation() {
        assert!(std::mem::size_of::<Record>() <= 192);
        let mut model = Model::new();
        let mut observer = Some(Diagnostic::new(&model));
        let mut output = Vec::new();
        paint(&mut model, &mut observer);
        for _ in 0..100 {
            feed(&mut model, &mut observer, b"\x1b[<32;2;2M", &mut output);
        }
        assert_eq!(observer.as_ref().unwrap().length, 1);
        assert_eq!(last(&observer).count, 100);
        observer.as_mut().unwrap().records[0]
            .as_mut()
            .unwrap()
            .count = u64::MAX;
        feed(&mut model, &mut observer, b"\x1b[<32;2;2M", &mut output);
        assert_eq!(last(&observer).count, u64::MAX);
        assert!(observer.as_ref().unwrap().saturated);
        for _ in 0..100 {
            feed(&mut model, &mut observer, b"\t", &mut output);
        }
        assert_eq!(observer.as_ref().unwrap().length, 32);
        assert_eq!(observer.as_ref().unwrap().overwritten, 69);
        model.focus = Focus::Editor;
        for payload in [
            b"SYNTHETIC_PRIVATE_TEXT".as_slice(),
            b"\x1b[200~SYNTHETIC_PRIVATE_PASTE\x1b[201~",
        ] {
            feed(&mut model, &mut observer, payload, &mut output);
        }
        let observer = observer.as_mut().unwrap();
        observer.events = u64::MAX;
        observer.sequence = u64::MAX;
        observer.overwritten = u64::MAX;
        observer.bytes_read = u64::MAX;
        observer.bytes_fed = u64::MAX;
        observer.sent_bytes = u64::MAX;
        observer.painted.sequence = u64::MAX;
        observer.read(1);
        observer.fed();
        observer.displayed(&model);
        let record = observer.before(&Event::Copy, &model);
        model.copied = "SYNTHETIC_PRIVATE_COPY".into();
        observer.after(record, &model, CopyOutcome::Sent);
        let record = observer.before(&Event::Copy, &model);
        observer.after(record, &model, CopyOutcome::Sent);
        assert!(observer.saturated);
        assert_eq!(observer.events, u64::MAX);
        assert_eq!(observer.painted.sequence, u64::MAX);
        assert!(observer.footer().as_str().len() <= 192);
        let mut report = Vec::new();
        observer.finish(&model, false);
        observer.report(&mut report, "ok", true, true).unwrap();
        assert!(report.len() <= 10240);
        let report = std::str::from_utf8(&report).unwrap();
        assert!(!report.contains("SYNTHETIC_PRIVATE"));
        assert!(!report.contains("leftright"));
        assert!(report.contains("saturated=1"));
        assert_eq!(
            report
                .lines()
                .filter(|line| line.starts_with("record "))
                .count(),
            32
        );
        assert!(
            report
                .lines()
                .filter(|line| line.starts_with("record "))
                .all(|line| line.len() < 256)
        );
        let mut line = Inline::<8>::new();
        let _ = fmt::write(&mut line, format_args!("{}", "界🙂界🙂"));
        assert!(line.as_str().ends_with('~'));
        line.end_line();
        assert!(line.as_str().ends_with("~\n"));
        let mut line = Inline::<6>::new();
        let _ = fmt::write(&mut line, format_args!("{}", "界界"));
        let _ = fmt::write(&mut line, format_args!("x"));
        line.end_line();
        assert_eq!(line.as_str(), "界~\n");
        let display = model.project(10, 12);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 12));
        observer.paint_footer(&model, &display, &mut buffer);
        assert_eq!(buffer[(9, 10)].symbol(), "~");
        println!(
            "diagnostic inline record={} observer={} report={} bytes",
            std::mem::size_of::<Record>(),
            std::mem::size_of::<Diagnostic>(),
            report.len()
        );
    }
}
