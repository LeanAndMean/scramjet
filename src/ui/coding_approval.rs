use super::{Event, Focus, Model};
use ratatui_core::{
    buffer::Buffer,
    style::{Color, Style},
};
use scramjet_rs::{CodingDecision, CodingProposal};
use std::sync::Arc;

pub(super) const DOCUMENT_LIMIT: usize = 32768;
#[derive(Default)]
pub(super) struct Approval {
    proposal: Option<Arc<CodingProposal>>,
    document: String,
    page: usize,
    coverage: u64,
    pub armed: bool,
    pub displayed: Option<Arc<CodingProposal>>,
}
pub(super) fn encode(text: &str) -> String {
    let mut output = String::with_capacity(text.len() * 12);
    for c in text.chars() {
        match c {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            ' '..='~' => output.push(c),
            _ => {
                use std::fmt::Write;
                write!(output, "\\u{{{:X}}}", c as u32).unwrap();
            }
        }
    }
    output
}
fn document(p: &CodingProposal) -> String {
    format!(
        "EXACT EDIT APPROVAL - complete ASCII document | Legend: ASCII literal; quote/backslash escaped; LF=\\n; TAB=\\t; other scalar=\\u{{HEX}}. | Quotes delimit complete fields, including spaces/empty strings. UTF8 counts are source bytes, not encoded bytes. No ellipsis or hidden payload. | Page lines concatenate WITHOUT inserted bytes. | owner={} session=\"{}\" request={} turn={} operation={} | root device={} inode={} target=\"{}\" | expected UTF8 bytes={} encoded=\"{}\" | replacement UTF8 bytes={} encoded=\"{}\" | invocation={} | END EXACT EDIT APPROVAL",
        p.owner,
        encode(&p.intent.session),
        p.request_id.0,
        p.turn,
        p.intent.operation,
        p.intent.root.device,
        p.intent.root.inode,
        encode(&p.intent.target),
        p.intent.expected.len(),
        encode(&p.intent.expected),
        p.intent.replacement.len(),
        encode(&p.intent.replacement),
        p.invocation
    )
}
impl Approval {
    #[cfg(test)]
    pub(super) fn document_len(&self) -> usize {
        self.document.len()
    }
    pub fn observe(&mut self, proposal: Option<Arc<CodingProposal>>) {
        if self.proposal == proposal {
            return;
        }
        self.document = proposal.as_ref().map_or_else(String::new, |p| document(p));
        self.proposal = proposal;
        self.page = 0;
        self.coverage = 0;
        self.armed = false;
        self.displayed = None;
    }
    pub fn pending(&self) -> bool {
        self.proposal.is_some()
    }
    fn pages(&self) -> usize {
        self.document.len().div_ceil(512)
    }
    fn suitable(&self, model: &Model, width: u16, height: u16) -> bool {
        self.pending()
            && model.focused
            && model.focus == Focus::Transcript
            && width >= 80
            && height >= 24
            && self.document.len() <= DOCUMENT_LIMIT
    }
    pub fn event(
        &mut self,
        event: &Event,
        model: &Model,
        ingress: &Option<Arc<CodingProposal>>,
        width: u16,
        height: u16,
    ) -> Option<(Arc<CodingProposal>, CodingDecision)> {
        if !self.suitable(model, width, height) {
            self.armed = false;
            return None;
        }
        match event {
            Event::ApprovalNext => {
                self.page = (self.page + 1).min(self.pages() - 1);
                self.armed = false;
            }
            Event::ApprovalPrevious => {
                self.page = self.page.saturating_sub(1);
                self.armed = false;
            }
            Event::Approve if self.armed && ingress == &self.displayed => {
                self.armed = false;
                return self.displayed.clone().map(|p| (p, CodingDecision::Approve));
            }
            Event::Deny if ingress == &self.displayed => {
                self.armed = false;
                return self.displayed.clone().map(|p| (p, CodingDecision::Deny));
            }
            _ => {}
        }
        None
    }
    pub fn paint(&self, model: &Model, buffer: &mut Buffer) -> bool {
        if !self.pending() || model.focus != Focus::Transcript {
            return false;
        }
        let width = buffer.area.width;
        let height = buffer.area.height;
        for y in 0..height.min(14) {
            for x in 0..width {
                buffer[(x, y)].set_symbol(" ").set_style(Style::default());
            }
        }
        if !self.suitable(model, width, height) {
            buffer.set_string(
                0,
                0,
                "Approval unavailable: focus and resize to >=80x24",
                Style::default(),
            );
            return false;
        }
        buffer.set_string(
            0,
            0,
            format!(
                "Exact proposal page {}/{} | coverage {:016X}",
                self.page + 1,
                self.pages(),
                self.coverage
            ),
            Style::default().fg(Color::Yellow),
        );
        let start = self.page * 512;
        let end = (start + 512).min(self.document.len());
        let bytes = &self.document.as_bytes()[start..end];
        for (line, chunk) in bytes.chunks(64).enumerate() {
            let printable = std::str::from_utf8(chunk).unwrap();
            buffer.set_string(
                0,
                line as u16 + 2,
                format!("{:05}:{}", start + line * 64, printable),
                Style::default(),
            );
        }
        buffer.set_string(
            0,
            11,
            "Gutter = document byte offset; concatenate 64-byte lines/pages.",
            Style::default(),
        );
        buffer.set_string(
            0,
            12,
            "F7 approve after ALL pages; F8 deny; F9 next; F10 previous",
            Style::default(),
        );
        buffer.set_string(
            0,
            13,
            "Tab draft/browse; F4 Stop; Ctrl-Q quit. Display is not proof of reading.",
            Style::default(),
        );
        true
    }
    pub fn painted(&mut self, complete: bool) -> bool {
        self.displayed = if complete {
            self.proposal.clone()
        } else {
            None
        };
        if !complete {
            self.armed = false;
            return false;
        }
        self.coverage |= 1 << self.page;
        let all = u64::MAX >> (64 - self.pages());
        let became_armed = !self.armed && self.coverage == all;
        self.armed = self.coverage == all;
        became_armed
    }
}
#[cfg(test)]
#[path = "coding_approval_tests.rs"]
mod tests;

impl Model {
    pub(super) fn submit(
        &self,
        bridge: &crate::coding_ui_demo::Bridge,
        ingress: Option<crate::coding_ui_demo::Offer>,
        approval: bool,
        width: u16,
        height: u16,
    ) {
        if !self.focused
            || self.focus != Focus::Editor
            || self.anchor.is_some()
            || approval
            || width < 80
            || height < 24
            || super::clamp(width, height) != (self.display.width, self.display.height)
            || self.display.exhausted
        {
            bridge.reject(ingress, "FocusBrowseApprovalOrViewport");
        } else {
            bridge.submit(ingress, self.revision, &self.editor);
        }
    }
    pub(super) fn coding_projection(&mut self, projection: &crate::coding_ui_demo::Projection) {
        self.status = "F1 submit <=1024 bytes; Enter newline; Tab approval; F7/F8; F4 Stop";
        let mut lines = std::collections::VecDeque::with_capacity(80);
        if let Some(prior) = &projection.previous {
            if let Some(o) = &prior.observation {
                lines.push_back(super::Line {
                    id: 153,
                    text: format!(
                        "Prior {} Decision {:?}; Verified={} final={} (read only)",
                        prior.session.as_deref().unwrap_or("unavailable"),
                        o.decision,
                        o.verified,
                        o.final_summary.is_some()
                    ),
                    hard: true,
                });
            }
            lines.push_back(super::Line {
                id: 154,
                text: "Prior cleanup retained; F5 history; F12 select journey".into(),
                hard: true,
            });
        }
        if let Some(command) = &projection.command {
            self.status =
                "F1 command <=1024 bytes; Enter newline; Tab exact approval; F7/F8; F4 Stop";
            lines.push_back(super::Line {
                id: 156,
                hard: true,
                text: "COMMAND replace / ReplaceVerified / sample.txt; input is entire replacement; F1 is NOT edit approval".into(),
            });
            if let Some(fact) = &command.invocations[0] {
                let coding = fact.coding.as_ref().unwrap();
                lines.push_back(super::Line {
                    id: 157,
                    hard: true,
                    text: format!(
                        "Command {:?} -> {} owner={:?} input_bytes={} phase={:?}",
                        fact.id,
                        coding.session,
                        coding.owner,
                        coding.input.len(),
                        fact.phase
                    ),
                });
                lines.push_back(super::Line {
                    id: 158, hard: true,
                    text: format!("AwaitingCompletion reached={}; explicit report={:?}; command_local_settled={}; history is NOT a command report", coding.receipt_accepted, fact.report.as_ref().map(|r| r.completion), command.settled),
                });
            }
        }
        if projection.follow_up {
            lines.push_back(super::Line {
                id: 155,
                hard: true,
                text: format!(
                    "Same session {}; offered invocation {} | {}",
                    projection.session.as_deref().unwrap_or("unavailable"),
                    projection.invocation,
                    if projection.checkpoint.is_some() {
                        "first verified checkpoint (live receipt); history pending until finish"
                    } else {
                        "first invocation; one explicit follow-up only after verified checkpoint"
                    }
                ),
            });
        }
        if let Some(id) = projection.offer {
            lines.push_back(super::Line {
                id: 150,
                text: format!(
                    "Submit offer={} revision={} bytes={}",
                    id,
                    self.revision,
                    self.editor.len()
                ),
                hard: true,
            });
        }
        if let Some(o) = &projection.observation {
            for (turn, slot) in o
                .attempts
                .iter()
                .enumerate()
                .filter(|(_, s)| s.recorded || s.request_id.is_some())
            {
                let base = turn as u64 * 16;
                lines.push_back(super::Line {
                    id: base,
                    text: format!(
                        "Turn {} recorded={} {:?} {:?} material={:?}",
                        turn + 1,
                        slot.recorded,
                        slot.request_id,
                        slot.outcome,
                        slot.material
                    ),
                    hard: true,
                });
                lines.push_back(super::Line {
                    id: base + 1,
                    text: format!("Accounting {:?}", slot.accounting),
                    hard: true,
                });
                if let Some(output) = &slot.output {
                    for (index, text) in super::runtime_projection::excerpt(output)
                        .pieces
                        .into_iter()
                        .enumerate()
                    {
                        lines.push_back(super::Line {
                            id: base + 2 + index as u64,
                            text,
                            hard: true,
                        });
                    }
                }
            }
            lines.push_back(super::Line {
                id: 128,
                text: format!(
                    "Invocation {} Decision {:?}; authorized={} dispatched={} effect={:?}",
                    o.invocation, o.decision, o.authorized, o.dispatched, o.effect
                ),
                hard: true,
            });
            lines.push_back(super::Line {
                id: 129,
                text: format!(
                    "Verified={} final={} run_ended={} worker_joined={}",
                    o.verified,
                    o.final_summary.is_some(),
                    o.run_ended,
                    o.worker_joined
                ),
                hard: true,
            });
            if let Some(error) = &o.error {
                for (index, text) in super::runtime_projection::excerpt(error)
                    .pieces
                    .into_iter()
                    .enumerate()
                {
                    lines.push_back(super::Line {
                        id: 130 + index as u64,
                        text,
                        hard: true,
                    });
                }
            }
        }
        if projection.ended {
            lines.push_back(super::Line {
                id: 145,
                text: format!(
                    "Read-only durable effect {:?}; comparison={} (live receipts separate)",
                    projection.durable_effect, projection.reconstructed
                ),
                hard: true,
            });
        }
        if let Some(receipt) = projection.submission {
            lines.push_back(super::Line {
                id: 151,
                text: format!(
                    "Queued offer={} revision={} bytes={} (local ownership only)",
                    receipt.offer.id, receipt.offer.revision, receipt.bytes
                ),
                hard: true,
            });
        }
        if let Some((offer, reason)) = projection.rejection {
            lines.push_back(super::Line {
                id: 152,
                text: format!("Rejected {:?}: {}", offer, reason),
                hard: true,
            });
        }
        for line in &mut lines {
            line.id += projection.journey * 256;
        }
        self.lines = lines;
        self.runtime_summary = Some(format!(
            "coding-ui: {}{}",
            if projection.ended {
                "cleanup retained; Ctrl-Q quit"
            } else if projection.follow_up
                && projection.checkpoint.is_some()
                && projection.submission.is_none()
            {
                "same-session invocation 2; awaiting explicit F1 follow-up"
            } else if projection.previous.is_some() && projection.submission.is_none() {
                "cleanup retained; Ctrl-Q quit | awaiting F1 submission"
            } else if projection.submission.is_none() {
                "awaiting F1 submission"
            } else {
                "running; Tab exact approval"
            },
            if projection.error.is_some()
                || projection
                    .previous
                    .as_ref()
                    .is_some_and(|p| p.error.is_some())
            {
                "; ERROR"
            } else {
                ""
            }
        ));
    }
}
