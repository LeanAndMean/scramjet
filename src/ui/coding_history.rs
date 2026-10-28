use super::{Event, Focus, Model};
use ratatui_core::{
    buffer::Buffer,
    style::{Color, Style},
};
use scramjet_rs::{CodingHistory, CodingHistoryEntry};
use std::{fmt::Write, sync::Arc};

const DOCUMENT_BYTES: usize = 32768;
#[derive(Default)]
pub(super) struct History {
    snapshot: Option<Arc<CodingHistory>>,
    sources: [Option<Arc<CodingHistory>>; 2],
    sessions: [Option<Arc<str>>; 2],
    errors: [Option<&'static str>; 2],
    finished: [bool; 2],
    slots: usize,
    selected: usize,
    current: Option<Arc<str>>,
    unavailable: Option<&'static str>,
    settled: bool,
    open: bool,
    fact: usize,
    page: usize,
    document: String,
}
impl History {
    #[cfg(test)]
    pub(super) fn document_len(&self) -> usize {
        self.document.len()
    }
    pub fn active(&self) -> bool {
        self.open
    }
    pub fn suppresses(&self, event: &Event) -> bool {
        self.open
            && matches!(
                event,
                Event::Approve | Event::Deny | Event::Submit | Event::Copy | Event::Mouse { .. }
            )
    }
    pub fn observe(&mut self, p: &crate::coding_ui_demo::Projection) {
        self.current = p.session.clone();
        self.slots = if p.previous.is_some() { 2 } else { 1 };
        for (index, source) in p
            .previous
            .iter()
            .map(|p| p.as_ref())
            .chain(std::iter::once(p))
            .enumerate()
        {
            if self.sessions[index] != source.session {
                self.sources[index] = None;
            }
            self.sessions[index] = source.session.clone();
            self.finished[index] = source.ended;
            self.errors[index] = source.history_error;
            if self.sources[index].is_none() && source.ended {
                self.sources[index] = source.history.clone();
            }
        }
        let chosen = &self.sources[self.selected];
        let changed = match (&self.snapshot, chosen) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.snapshot = chosen.clone();
            self.fact = 0;
            self.select();
        }
        self.settled = self.finished[self.selected];
        self.unavailable = self.errors[self.selected];
    }
    fn pages(&self) -> usize {
        self.document.len().div_ceil(512).max(1)
    }
    fn select(&mut self) {
        self.page = 0;
        self.document.clear();
        let Some(h) = &self.snapshot else {
            return;
        };
        let Some(r) = h.records().get(self.fact) else {
            return;
        };
        let d = &mut self.document;
        write!(d, "HISTORY READ ONLY | version={} session UTF8 bytes={} encoded=\"{}\" root device={} inode={} | sequence={} | Legend: ASCII literal; quote/backslash escaped; LF=\\n; TAB=\\t; other scalar=\\u{{HEX}}. Quotes delimit complete fields. Lines/pages concatenate WITHOUT inserted bytes. | ", h.version(), h.session().len(), super::coding_approval::encode(h.session()), h.root().device, h.root().inode, r.sequence).unwrap();
        match &r.entry {
            CodingHistoryEntry::FollowUp {
                invocation,
                after_turn,
                prompt,
            } => {
                write!(d, "HISTORICAL INVOCATION BOUNDARY - NOT CURRENT AUTHORITY | invocation={invocation} after-turn={after_turn} prompt UTF8 bytes={} encoded=\"{}\"", prompt.len(), super::coding_approval::encode(prompt)).unwrap();
            }
            CodingHistoryEntry::Material(m) => {
                write!(d, "material turn={} request={} tool-turn={} event=\"{}\" status=\"{}\" | {} | text UTF8 bytes={} encoded=\"{}\"", m.turn, m.request, m.operation, m.event, m.status, label(m), m.text.len(), super::coding_approval::encode(&m.text)).unwrap();
            }
            CodingHistoryEntry::Authorized(i) => {
                write!(d, "HISTORICAL AUTHORIZATION - NOT A CURRENT APPROVAL | edit-operation={} intent session UTF8 bytes={} encoded=\"{}\" root device={} inode={} | target UTF8 bytes={} encoded=\"{}\" | expected UTF8 bytes={} encoded=\"{}\" | replacement UTF8 bytes={} encoded=\"{}\"", i.operation, i.session.len(), super::coding_approval::encode(&i.session), i.root.device, i.root.inode, i.target.len(), super::coding_approval::encode(&i.target), i.expected.len(), super::coding_approval::encode(&i.expected), i.replacement.len(), super::coding_approval::encode(&i.replacement)).unwrap();
            }
            CodingHistoryEntry::Dispatched { operation } => {
                write!(d, "edit-operation={operation} recorded Dispatched: effect may have occurred; not a completion acknowledgement").unwrap();
            }
            CodingHistoryEntry::Completed { operation } => {
                write!(d, "edit-operation={operation} recorded Completed: durable effect acknowledgement; live verification/accounting remain separate").unwrap();
            }
            CodingHistoryEntry::Cancelled { operation } => {
                write!(
                    d,
                    "edit-operation={operation} recorded Cancelled: NotDispatched"
                )
                .unwrap();
            }
        }
        d.push_str(" | END HISTORY FACT");
        if d.len() > DOCUMENT_BYTES {
            d.clear();
            self.unavailable = Some("selected document ceiling exceeded; not clipped");
        }
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
        let previous = (self.fact, self.page);
        match e {
            Event::HistoryJourney => {
                self.selected = (self.selected + 1) % self.slots.max(1);
                self.snapshot = self.sources[self.selected].clone();
                self.settled = self.finished[self.selected];
                self.unavailable = self.errors[self.selected];
                self.fact = 0;
                self.select();
            }
            Event::CancelChild => {
                self.fact = (self.fact + 1).min(
                    self.snapshot
                        .as_ref()
                        .map_or(0, |h| h.records().len().saturating_sub(1)),
                );
                if previous.0 != self.fact {
                    self.select();
                }
            }
            Event::HistoryPrevious => {
                self.fact = self.fact.saturating_sub(1);
                if previous.0 != self.fact {
                    self.select();
                }
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
        let width = b.area.width as usize;
        let line = |b: &mut Buffer, y, text: &str| {
            if y < b.area.height {
                b.set_stringn(0, y, text, width, Style::default());
            }
        };
        let Some(h) = &self.snapshot else {
            line(
                b,
                0,
                &format!(
                    "HISTORY READ ONLY - unavailable | journey {}/{} selected={} current={}",
                    self.selected + 1,
                    self.slots,
                    self.sessions[self.selected]
                        .as_deref()
                        .unwrap_or("unavailable"),
                    self.current.as_deref().unwrap_or("unavailable")
                ),
            );
            line(
                b,
                2,
                self.unavailable.unwrap_or(if self.settled {
                    "No validated history snapshot"
                } else {
                    "Pending: history unavailable while execution is live"
                }),
            );
            line(
                b,
                12,
                "F5 close history; F12 journey; Tab editor; F4 Stop; Ctrl-Q quit",
            );
            return;
        };
        line(
            b,
            0,
            &format!(
                "HISTORY READ ONLY fact {}/{} seq={} page {}/{} | journey {}/{}",
                if h.records().is_empty() {
                    0
                } else {
                    self.fact + 1
                },
                h.records().len(),
                h.records().get(self.fact).map_or(0, |r| r.sequence),
                self.page + 1,
                self.pages(),
                self.selected + 1,
                self.slots
            ),
        );
        line(
            b,
            1,
            &format!(
                "selected={} current={} Ending={:?}; effect={:?}",
                h.session(),
                self.current.as_deref().unwrap_or("unavailable"),
                h.ending(),
                h.operations().last().map(|f| f.state)
            ),
        );
        if b.area.width < 80 || b.area.height < 24 {
            line(b, 2, "Detail unavailable: resize to >=80x24");
            return;
        }
        if let Some(error) = self.unavailable {
            line(b, 2, error);
            return;
        }
        if h.records().is_empty() {
            line(b, 2, "Header only: incomplete; no terminal reason recorded");
        }
        let start = self.page * 512;
        let end = (start + 512).min(self.document.len());
        for (n, chunk) in self.document.as_bytes()[start..end].chunks(64).enumerate() {
            line(
                b,
                n as u16 + 2,
                &format!(
                    "{:05}:{}",
                    start + n * 64,
                    std::str::from_utf8(chunk).unwrap()
                ),
            );
        }
        line(
            b,
            11,
            "Gutter=document byte offset; concatenate lines/pages without bytes.",
        );
        line(
            b,
            12,
            "F5 close; F12 journey; F6/F11 facts; F9/F10 pages; Tab editor",
        );
        line(
            b,
            13,
            "F7/F8/F1 do nothing here; Copy disabled; F4 Stop; Ctrl-Q quit",
        );
        b[(0, 0)].set_style(Style::default().fg(Color::Yellow));
    }
}
#[cfg(test)]
#[path = "coding_history_tests.rs"]
pub(crate) mod tests;

fn label(m: &scramjet_rs::CodingMaterial) -> &'static str {
    match (m.event.as_str(), m.status.as_str()) {
        ("Request", _) => "Recorded context before admission; request0 is not dispatch",
        ("ResponsePart", _) => "Recorded settled response fragment; not live stream receipt",
        ("Response", "Success") => "Transport response acknowledged; not tool/effect success",
        ("Response", _) => "Recorded failed response; no inferred failure cause",
        ("Final", _) => "Recorded validated summary; not a new completion or active context",
        ("Tool", "Error") => "Recorded generic tool error; not proof of denial/cancellation",
        _ => "Recorded tool material; payload claims are not independent authority",
    }
}
