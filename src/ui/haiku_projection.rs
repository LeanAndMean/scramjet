use super::{
    Event, Focus, Line, Model,
    haiku_document::{Document, LEGEND},
};
use ratatui_core::{
    buffer::Buffer,
    style::{Color, Style},
};
use scramjet_rs::{HaikuHistory, HaikuProposal, HaikuRecord};
use std::{collections::VecDeque, sync::Arc};

#[derive(Default)]
pub(super) struct Presentation {
    history: Option<Arc<HaikuHistory>>,
    lines: VecDeque<Line>,
    selected: usize,
    active: bool,
    detail: Option<Result<Document, &'static str>>,
    proposal: Option<HaikuProposal>,
    approval: Option<Result<Document, &'static str>>,
    epoch: u64,
    displayed: Option<u64>,
    armed: bool,
    submission: Option<(u64, Document)>,
    submission_displayed: Option<u64>,
}
fn kind(record: &HaikuRecord) -> &'static str {
    match record {
        HaikuRecord::Header { .. } => "Header",
        HaikuRecord::Attempt { .. } => "Attempt",
        HaikuRecord::Selected { .. } => "Selected",
        HaikuRecord::Receipt { .. } => "Receipt",
        HaikuRecord::Started { .. } => "Started",
        HaikuRecord::Denied { .. } => "Denied",
        HaikuRecord::Authorized { .. } => "Authorized",
        HaikuRecord::NotDispatched { .. } => "NotDispatched",
        HaikuRecord::Dispatched { .. } => "Dispatched",
        HaikuRecord::Created { .. } => "Created",
        HaikuRecord::Process { .. } => "Process",
        HaikuRecord::Result { .. } => "Result",
        HaikuRecord::Finished => "Finished",
        HaikuRecord::Stopped { .. } => "Stopped",
    }
}
impl Presentation {
    pub fn pending(&self) -> bool {
        self.proposal.is_some()
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn ingress(&self) -> Option<u64> {
        self.displayed
    }
    pub fn disarm(&mut self) {
        self.displayed = None;
        self.armed = false;
    }
    pub fn observe(&mut self, projection: &crate::haiku_ui_demo::Projection, read_only: bool) {
        let snapshot = &projection.snapshot;
        if self
            .history
            .as_ref()
            .is_none_or(|h| !Arc::ptr_eq(h, &snapshot.history))
        {
            self.detail = None;
            self.lines.clear();
            for entry in snapshot.history.entries.iter().take(128) {
                let extra = match entry.record.as_ref() {
                    HaikuRecord::Receipt { attempt } => {
                        format!(" outcome={:?} usage={:?}", attempt.outcome, attempt.usage)
                    }
                    HaikuRecord::Process { result, .. } => format!(
                        " exit={:?} signal={:?} stdout_bytes={} stderr_bytes={} launcher_reaped={} namespace_dead={}",
                        result.exit_code,
                        result.signal,
                        result.stdout.len(),
                        result.stderr.len(),
                        result.launcher_reaped,
                        result.namespace_dead
                    ),
                    _ => String::new(),
                };
                self.lines.push_back(Line {
                    id: entry.sequence as u64,
                    hard: true,
                    text: format!(
                        "Record {} {} acknowledged={}{}; F5 complete JSON",
                        entry.sequence,
                        kind(&entry.record),
                        entry.acknowledged,
                        extra
                    ),
                });
            }
            while self.lines.len() > 70 {
                self.lines.pop_front();
            }
            self.selected = self
                .selected
                .min(snapshot.history.entries.len().saturating_sub(1));
            self.history = Some(snapshot.history.clone());
        }
        let proposal = if read_only {
            None
        } else {
            snapshot.proposal.clone()
        };
        let changed = match (&self.proposal, &proposal) {
            (None, None) => false,
            (Some(a), Some(b)) => a.intent() != b.intent(),
            _ => true,
        };
        if changed {
            self.approval = None;
            self.proposal = proposal;
            self.epoch = self
                .epoch
                .checked_add(1)
                .expect("bounded presentation epoch");
            self.approval = self.proposal.as_ref().map(|p| Document::new(p.intent()));
            self.disarm();
            self.active = false;
        }
    }
    fn load_detail(&mut self) {
        if self.detail.is_none()
            && let Some(entry) = self
                .history
                .as_ref()
                .and_then(|h| h.entries.get(self.selected))
        {
            #[derive(serde::Serialize)]
            struct Detail<'a> {
                sequence: usize,
                acknowledged: bool,
                record: &'a HaikuRecord,
            }
            self.detail = Some(Document::new(&Detail {
                sequence: entry.sequence,
                acknowledged: entry.acknowledged,
                record: &entry.record,
            }));
        }
    }
    pub fn event(
        &mut self,
        event: &Event,
        model: &Model,
        ingress: Option<u64>,
        width: u16,
        height: u16,
    ) -> Option<(HaikuProposal, bool)> {
        let suitable = model.focused
            && model.focus == Focus::Transcript
            && width >= 80
            && height >= 24
            && !model.display.exhausted;
        if !suitable {
            self.disarm();
        }
        if self.pending() {
            match event {
                Event::ApprovalNext | Event::ApprovalPrevious => {
                    if let Some(Ok(document)) = &mut self.approval {
                        if *event == Event::ApprovalNext {
                            document.next();
                        } else {
                            document.previous();
                        }
                    }
                    self.disarm();
                }
                Event::Approve
                    if suitable && self.armed && ingress.is_some() && ingress == self.displayed =>
                {
                    self.disarm();
                    return self.proposal.clone().map(|p| (p, true));
                }
                Event::Deny if suitable && ingress.is_some() && ingress == self.displayed => {
                    self.disarm();
                    return self.proposal.clone().map(|p| (p, false));
                }
                _ => {}
            }
        } else {
            match event {
                Event::NextChild if model.focused && model.focus == Focus::Transcript => {
                    self.active = !self.active;
                    if self.active {
                        self.load_detail();
                    }
                    self.disarm();
                }
                Event::HistoryJourney if self.active => {
                    if let Some(history) = &self.history {
                        self.selected = (self.selected + 1) % history.entries.len().max(1);
                    }
                    self.detail = None;
                    self.load_detail();
                }
                Event::CancelChild if self.active => {
                    self.selected = self.selected.saturating_sub(1);
                    self.detail = None;
                    self.load_detail();
                }
                Event::ApprovalNext | Event::ApprovalPrevious if self.active => {
                    if let Some(Ok(document)) = &mut self.detail {
                        if *event == Event::ApprovalNext {
                            document.next();
                        } else {
                            document.previous();
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }
    pub fn suppresses(&self, event: &Event) -> bool {
        (self.active || self.pending())
            && matches!(
                event,
                Event::Submit
                    | Event::Follow
                    | Event::Copy
                    | Event::Mouse { .. }
                    | Event::NextChild
                    | Event::CancelChild
                    | Event::HistoryJourney
                    | Event::ApprovalNext
                    | Event::ApprovalPrevious
                    | Event::Approve
                    | Event::Deny
            )
    }
    pub fn project(
        &mut self,
        projection: &crate::haiku_ui_demo::Projection,
        model: &mut Model,
        read_only: bool,
    ) {
        if !read_only && projection.offered && model.editor == scramjet_rs::haiku_prompt() {
            if self
                .submission
                .as_ref()
                .is_none_or(|(revision, _)| *revision != model.revision)
            {
                self.submission = None;
                self.submission = Document::new(&model.editor)
                    .ok()
                    .map(|d| (model.revision, d));
                self.submission_displayed = None;
            }
        } else {
            self.submission = None;
            self.submission_displayed = None;
        }
        model.status = "Fixed request: F1 submit; Tab approval/history; F5 detail; F12/F6 record; F9/F10 page; F4 Stop";
        model.lines = self.lines.clone();
        let snapshot = &projection.snapshot;
        let facts = [
            format!(
                "ONE FIXED SYNTHETIC REQUEST; submission is NOT effect approval; read_only={read_only}"
            ),
            format!(
                "phase={:?} request={:?} offered={} attempt_observed={} attempt_acknowledged={}",
                snapshot.phase,
                snapshot.request_id,
                projection.offered,
                snapshot.observed_attempt.is_some(),
                snapshot.attempt_acknowledged
            ),
            format!(
                "run_ended={} worker_terminated={} worker_joined={} runtime_joined={} ending_seen={} ending_acknowledged={}",
                snapshot.run_ended,
                snapshot.worker_terminated,
                snapshot.worker_joined,
                projection.runtime_joined,
                snapshot.history.ending_seen(),
                snapshot.history.finished()
            ),
            format!(
                "process_observed={} process_acknowledged={} history_records={}; F11 restores/settles when run ended",
                snapshot.observed_process.is_some(),
                snapshot.process_acknowledged,
                snapshot.history.entries.len()
            ),
        ];
        for (i, text) in facts.into_iter().enumerate() {
            model.lines.push_back(Line {
                id: 256 + i as u64,
                text,
                hard: true,
            });
        }
        for (i, error) in [
            projection.rejection,
            projection.error.as_deref(),
            snapshot.failure.as_deref(),
        ]
        .into_iter()
        .flatten()
        .enumerate()
        {
            for (j, text) in super::runtime_projection::excerpt(error)
                .pieces
                .into_iter()
                .enumerate()
            {
                model.lines.push_back(Line {
                    id: 270 + (i * 8 + j) as u64,
                    text,
                    hard: true,
                });
            }
        }
        while model.lines.len() > 80 {
            model.lines.pop_front();
        }
        model.runtime_summary = Some(
            if read_only {
                "haiku-ui: inert history; ending bytes do not prove acknowledgement"
            } else {
                "haiku-ui: retained; Stop/quit restores before host settlement"
            }
            .into(),
        );
    }
    pub fn submission_painted(&mut self, complete: bool) {
        self.submission_displayed = if complete {
            self.submission.as_ref().map(|(revision, _)| *revision)
        } else {
            None
        };
    }
    pub fn submission_visible(&self, revision: u64) -> bool {
        self.submission_displayed == Some(revision)
    }
    pub fn paint_submission(&self, model: &Model, buffer: &mut Buffer) -> bool {
        let Some((revision, document)) = &self.submission else {
            return false;
        };
        if self.pending()
            || self.active
            || model.focus != Focus::Editor
            || !model.focused
            || model.revision != *revision
            || buffer.area.width < 80
            || buffer.area.height < 24
            || model.display.exhausted
            || document.pages() != 1
        {
            return false;
        }
        for y in 0..14 {
            for x in 0..buffer.area.width {
                buffer[(x, y)].set_symbol(" ").set_style(Style::default());
            }
        }
        buffer.set_string(
            0,
            0,
            "COMPLETE FIXED REQUEST (ASCII JSON string); F1 submits, NOT approval",
            Style::default().fg(Color::Yellow),
        );
        for (line, chunk) in document.payload().chunks(64).enumerate() {
            buffer.set_string(
                0,
                line as u16 + 2,
                format!("{:06}:{}", line * 64, std::str::from_utf8(chunk).unwrap()),
                Style::default(),
            );
        }
        for (line, chunk) in LEGEND.as_bytes().chunks(76).enumerate() {
            buffer.set_string(
                0,
                11 + line as u16,
                std::str::from_utf8(chunk).unwrap(),
                Style::default(),
            );
        }
        true
    }
    pub fn paint(&self, model: &Model, buffer: &mut Buffer) -> bool {
        let document = if self.pending() {
            &self.approval
        } else if self.active {
            &self.detail
        } else {
            return false;
        };
        if model.focus != Focus::Transcript {
            return false;
        }
        let width = buffer.area.width;
        let height = buffer.area.height;
        for y in 0..height.min(17) {
            for x in 0..width {
                buffer[(x, y)].set_symbol(" ").set_style(Style::default());
            }
        }
        if !model.focused || width < 80 || height < 24 || model.display.exhausted {
            buffer.set_string(
                0,
                0,
                "Complete detail unavailable: focus, >=80x24, nonexhausted display; F4 Stop",
                Style::default(),
            );
            return false;
        }
        let Some(Ok(document)) = document else {
            buffer.set_string(
                0,
                0,
                "Presentation resource refusal: no approval; F8 deny or F4 Stop",
                Style::default().fg(Color::Yellow),
            );
            return self.pending();
        };
        buffer.set_string(
            0,
            0,
            format!(
                "{} page {}/{} record={} ALL-pages-painted={}",
                if self.pending() {
                    "EXACT APPROVAL"
                } else {
                    "INERT HISTORY"
                },
                document.page() + 1,
                document.pages(),
                self.selected,
                document.covered()
            ),
            Style::default().fg(Color::Yellow),
        );
        for (line, chunk) in document.payload().chunks(64).enumerate() {
            buffer.set_string(
                0,
                line as u16 + 2,
                format!(
                    "{:06}:{}",
                    document.page() * 512 + line * 64,
                    std::str::from_utf8(chunk).unwrap()
                ),
                Style::default(),
            );
        }
        for (line, chunk) in LEGEND.as_bytes().chunks(76).enumerate() {
            buffer.set_string(
                0,
                11 + line as u16,
                std::str::from_utf8(chunk).unwrap(),
                Style::default(),
            );
        }
        buffer.set_string(
            0,
            15,
            "F7 ALL-pages approve; F8 deny; F9/F10 pages; F12/F6 history record",
            Style::default(),
        );
        buffer.set_string(
            0,
            16,
            "Tab draft; F5 history; F4 Stop; Ctrl-Q quit. Gutters are not JSON bytes.",
            Style::default(),
        );
        self.pending()
    }
    pub fn painted(&mut self, complete: bool) -> bool {
        let old = self.armed;
        if !complete {
            self.disarm();
            return false;
        }
        self.displayed = Some(self.epoch);
        if let Some(Ok(document)) = &mut self.approval {
            document.painted();
            self.armed = document.covered();
        }
        !old && self.armed
    }
}
