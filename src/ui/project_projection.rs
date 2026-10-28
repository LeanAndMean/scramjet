use super::{Event, Focus, Line, Model, coding_approval::encode};
use crate::project_ui_demo::{Intent, Projection, RetainedHistory};
use ratatui_core::{buffer::Buffer, style::Style};
use scramjet_rs::{ProjectRecord, ReaderState};
use std::{collections::VecDeque, sync::Arc};

const DOCUMENT_LIMIT: usize = 196608;
#[derive(Default)]
pub(super) struct History {
    open: bool,
    selected: usize,
    fact: usize,
    page: usize,
    records: Vec<Arc<ProjectRecord>>,
    recovered: Option<Arc<RetainedHistory>>,
    document: String,
    selected_record: Option<Arc<ProjectRecord>>,
    coding_fact: Option<(Arc<RetainedHistory>, usize)>,
    unavailable: bool,
}
impl History {
    pub fn active(&self) -> bool {
        self.open
    }
    #[cfg(test)]
    pub(super) fn document_len(&self) -> usize {
        self.document.len()
    }
    fn facts(&self) -> Vec<Arc<ProjectRecord>> {
        self.records
            .iter()
            .filter(|r| match r.as_ref() {
                ProjectRecord::Header(_) | ProjectRecord::Closure(_) => true,
                ProjectRecord::Requested(r) => r.journey as usize == self.selected + 1,
                ProjectRecord::Captured(r) => r.journey as usize == self.selected + 1,
                ProjectRecord::Readers(r) => r.journey as usize == self.selected + 1,
                ProjectRecord::Coding(r) => r.journey as usize == self.selected + 1,
                ProjectRecord::SourceReport(r) | ProjectRecord::VerificationReport(r) => {
                    r.journey as usize == self.selected + 1
                }
                ProjectRecord::VerificationReady(r) => r.journey as usize == self.selected + 1,
                ProjectRecord::VerificationRead(r) => r.journey as usize == self.selected + 1,
            })
            .cloned()
            .collect()
    }
    pub fn observe(&mut self, p: &Projection) {
        self.records.clone_from(&p.snapshot.records);
        self.recovered = p.history.clone();
        let facts = self.facts();
        if let Some(record) = &self.selected_record {
            if let Some(index) = facts.iter().position(|r| Arc::ptr_eq(r, record)) {
                self.fact = index;
            }
        } else if let Some((_, index)) = &self.coding_fact {
            self.fact = facts.len() + index;
        }
        self.select();
    }
    fn count(&self) -> usize {
        self.facts().len()
            + self
                .recovered
                .as_ref()
                .and_then(|h| h.coding[self.selected].as_ref())
                .map_or(0, |h| h.records().len())
    }
    fn select(&mut self) {
        let facts = self.facts();
        self.fact = self.fact.min(self.count().saturating_sub(1));
        let record = facts.get(self.fact).cloned();
        let coding = if self.fact >= facts.len() {
            self.recovered
                .as_ref()
                .map(|h| (h.clone(), self.fact - facts.len()))
        } else {
            None
        };
        let same_record = match (&record, &self.selected_record) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let same_coding = match (&coding, &self.coding_fact) {
            (Some((a, i)), Some((b, j))) => Arc::ptr_eq(a, b) && i == j,
            (None, None) => true,
            _ => false,
        };
        if same_record && same_coding {
            return;
        }
        self.selected_record = record;
        self.coding_fact = coding;
        self.unavailable = false;
        let json = if let Some(record) = &self.selected_record {
            serde_json::to_string(record.as_ref()).ok()
        } else {
            self.coding_fact.as_ref().and_then(|(h, i)| {
                h.coding[self.selected].as_ref().and_then(|c| {
                    c.records().get(*i).map(|r| {
                        format!(
                            "CODING MATERIAL READ ONLY session={} sequence={} {:?}",
                            c.session(),
                            r.sequence,
                            r.entry
                        )
                    })
                })
            })
        };
        let mut document = json.as_deref().map_or_else(String::new, encode);
        if document.len() > DOCUMENT_LIMIT {
            document.clear();
            self.unavailable = true;
        }
        if document != self.document {
            self.page = 0;
            self.document = document;
        }
    }
    fn pages(&self) -> usize {
        self.document.len().div_ceil(512).max(1)
    }
    pub fn suppresses(&self, e: &Event) -> bool {
        self.open
            && matches!(
                e,
                Event::Submit | Event::Approve | Event::Deny | Event::Copy | Event::Mouse { .. }
            )
    }
    pub fn event(&mut self, e: &Event, model: &mut Model) -> bool {
        if !model.focused {
            return false;
        }
        if *e == Event::NextChild {
            self.open = !self.open;
            model.focus = Focus::Transcript;
            model.clear_selection();
            return true;
        }
        if !self.open || model.focus != Focus::Transcript {
            return false;
        }
        match e {
            Event::HistoryJourney => {
                self.selected = 1 - self.selected;
                self.fact = 0;
                self.selected_record = None;
                self.coding_fact = None;
                self.select();
            }
            Event::CancelChild => {
                self.fact = (self.fact + 1).min(self.count().saturating_sub(1));
                self.select();
            }
            Event::HistoryPrevious => {
                self.fact = self.fact.saturating_sub(1);
                self.select();
            }
            Event::ApprovalNext => self.page = (self.page + 1).min(self.pages() - 1),
            Event::ApprovalPrevious => self.page = self.page.saturating_sub(1),
            _ => return false,
        }
        model.clear_selection();
        true
    }
    pub fn paint(&self, model: &Model, b: &mut Buffer) {
        if !self.open || model.focus != Focus::Transcript {
            return;
        }
        for y in 0..b.area.height.min(14) {
            for x in 0..b.area.width {
                b[(x, y)].set_symbol(" ").set_style(Style::default());
            }
        }
        let line = |b: &mut Buffer, y, text: &str| {
            if y < b.area.height {
                b.set_stringn(0, y, text, b.area.width as usize, Style::default());
            }
        };
        line(
            b,
            0,
            &format!(
                "PROJECT HISTORY READ ONLY journey={} fact={}/{} page={}/{}",
                self.selected + 1,
                self.fact + 1,
                self.count(),
                self.page + 1,
                self.pages()
            ),
        );
        let coding = self
            .recovered
            .as_ref()
            .and_then(|h| h.coding[self.selected].as_ref());
        line(
            b,
            1,
            &format!(
                "Durable coding material records={} effect={:?}; historical facts NEVER grant authority",
                coding.map_or(0, |h| h.records().len()),
                coding.and_then(|h| h.operations().last().map(|o| o.state))
            ),
        );
        if b.area.width < 80 || b.area.height < 24 {
            line(b, 2, "Detail unavailable: resize to >=80x24");
            return;
        }
        if self.unavailable {
            line(b, 2, "Complete document unavailable; never clipped");
        }
        let start = self.page * 512;
        let end = (start + 512).min(self.document.len());
        for (i, chunk) in self.document.as_bytes()[start..end].chunks(64).enumerate() {
            line(
                b,
                i as u16 + 2,
                &format!(
                    "{:05}:{}",
                    start + i * 64,
                    std::str::from_utf8(chunk).unwrap()
                ),
            );
        }
        line(
            b,
            11,
            "ASCII escaped JSON/debug fact; concatenate pages/lines; gutter=byte offset.",
        );
        line(
            b,
            12,
            "F5 close; F12 journey; F6/F11 fact; F9/F10 page; Tab edit",
        );
        line(
            b,
            13,
            "History is read-only; no F1/F7/F8 or Copy; F4 Stop; Ctrl-Q quit",
        );
    }
}
#[cfg(test)]
pub(super) fn check_keyed_history(before: &Projection, after: &Projection) -> History {
    let mut history = History::default();
    history.observe(before);
    history.open = true;
    let coding = before.history.as_ref().unwrap().coding[0].as_ref().unwrap();
    let index = coding
        .records()
        .iter()
        .position(|r| encode(&format!("{:?}", r.entry)).len() > 512)
        .unwrap();
    history.fact = history.facts().len() + index;
    history.select();
    history.page = 1;
    let document = history.document.clone();
    history.observe(after);
    assert_eq!(history.document, document);
    assert_eq!(history.page, 1);
    assert_eq!(history.coding_fact.as_ref().unwrap().1, index);
    assert_eq!(history.selected, 0);
    assert!(history.open);
    assert!(history.document.len() <= DOCUMENT_LIMIT);
    let mut final_projection = Projection {
        snapshot: after.snapshot.clone(),
        offer: None,
        error: None,
        rejection: None,
        history: after.history.clone(),
    };
    final_projection
        .snapshot
        .records
        .push(Arc::new(ProjectRecord::Closure(
            scramjet_rs::ProjectClosure {
                cancelled: false,
                logical_settled: true,
                readers_joined: true,
                coding_joined: true,
                runtime_joined: true,
                attempts: 7,
                unknown_usage: 0,
                unresolved: 0,
            },
        )));
    history.observe(&final_projection);
    assert_eq!(history.document, document);
    assert_eq!(history.page, 1);
    history
}

impl Model {
    pub(super) fn project_projection(&mut self, p: &Projection, selected: u8) {
        self.status = "F1 request; F5 history; F3 verify/tail; F12 reader; Tab approval; F4 Stop";
        let mut lines = VecDeque::with_capacity(160);
        let mut add = |id, text| {
            lines.push_back(Line {
                id,
                text,
                hard: true,
            })
        };
        add(
            0,
            format!(
                "PROJECT {} instance={} phase={:?} cancelled={} settled={}",
                p.snapshot.session,
                p.snapshot.instance,
                p.snapshot.phase,
                p.snapshot.cancelled,
                p.snapshot.settled
            ),
        );
        add(1,"OFFLINE replace_once: three basenames, literal needle/replacement; <=1024 JSON bytes; two requests, one Runtime/120s deadline".into());
        add(
            2,
            format!(
                "Intent={:?} draft_revision={}; request F1 is NOT exact edit approval; F3 verification is an explicit read-only continuation",
                p.offer.map(|o| (o.journey, o.intent)),
                self.revision
            ),
        );
        if let Some(reason) = p.rejection {
            add(3, format!("Rejected: {reason}"));
        }
        for (index, journey) in p.snapshot.journeys.iter().enumerate() {
            let base = 100 + index as u64 * 1000;
            if let Some(readers) = &journey.readers {
                for (child, slot) in readers.children.iter().enumerate() {
                    let id = base + child as u64 * 16;
                    add(
                        id,
                        format!(
                            "Reader {child} journey={} {}/{} request={:?} phase={:?} received={}",
                            index + 1,
                            readers.session,
                            readers.generation,
                            slot.request_id,
                            slot.phase,
                            slot.received
                        ),
                    );
                    add(
                        id + 1,
                        format!(
                            "Accepted order={:?} fact={:?} accounting={:?}",
                            slot.acceptance_ordinal,
                            slot.fact.as_ref().map(|f| &f.state),
                            slot.accounting
                        ),
                    );
                    if let Some(ReaderState::Completed { summary }) =
                        slot.fact.as_ref().map(|f| &f.state)
                    {
                        for (n, text) in super::runtime_projection::excerpt(summary)
                            .pieces
                            .into_iter()
                            .enumerate()
                        {
                            add(id + 2 + n as u64, text);
                        }
                    }
                }
            }
            if let Some(coding) = &journey.coding {
                add(
                    base + 60,
                    format!(
                        "Coding journey={} decision={:?} authorized={} dispatched={} effect={:?} verified={} ended={} joined={}",
                        index + 1,
                        coding.decision,
                        coding.authorized,
                        coding.dispatched,
                        coding.effect,
                        coding.verified,
                        coding.run_ended,
                        coding.worker_joined
                    ),
                );
                for (turn, slot) in coding.attempts.iter().enumerate() {
                    if slot.request_id.is_some() {
                        add(
                            base + 61 + turn as u64,
                            format!(
                                "Turn {} request={:?} outcome={:?} accounting={:?}",
                                turn + 1,
                                slot.request_id,
                                slot.outcome,
                                slot.accounting
                            ),
                        );
                    }
                }
            }
        }
        for (index, record) in p.snapshot.records.iter().enumerate() {
            let text = match record.as_ref() {
                ProjectRecord::SourceReport(r) => format!(
                    "Source report journey={} {} paused={} summary={}",
                    r.journey,
                    r.completion,
                    r.paused,
                    encode(&r.summary)
                ),
                ProjectRecord::VerificationReport(r) => format!(
                    "Verification report journey={} {} summary={}",
                    r.journey,
                    r.completion,
                    encode(&r.summary)
                ),
                ProjectRecord::VerificationRead(r) => format!(
                    "Actual verification read journey={} matched={}",
                    r.journey, r.matched
                ),
                _ => continue,
            };
            add(3000 + index as u64, text);
        }
        if let Some(error) = p.error.as_deref().or(p.snapshot.error.as_deref()) {
            add(4000, format!("Error: {}", encode(error)));
        }
        self.lines = lines;
        self.runtime_summary = Some(format!(
            "project-ui {:?} settled={} | {} | reader={selected} | F5 history; Ctrl-Q quit",
            p.snapshot.phase,
            p.snapshot.settled,
            match p.offer.map(|o| o.intent) {
                Some(Intent::Request) => "awaiting F1 supported request",
                Some(Intent::Verify) => "PAUSED: F3 verify_edited",
                None if p.snapshot.current.is_some_and(|i| i.journey() == 2)
                    && p.snapshot.phase == scramjet_rs::ProjectPhase::Verified =>
                    "TwoJourneyLimit exhausted",
                None if p.snapshot.settled => "local joins/history acknowledged; READ ONLY",
                None => "live facts; final settlement only after terminal return",
            }
        ));
    }
}
