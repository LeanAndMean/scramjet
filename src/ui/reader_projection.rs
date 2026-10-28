use super::{Line, Model, runtime_projection::excerpt};
use crate::reader_ui_demo::Projection;
use scramjet_rs::ReaderState;
impl Model {
    pub(super) fn reader_projection(
        &mut self,
        projection: &Projection,
        selected: u8,
        child_mask: u8,
        tree_requested: bool,
    ) {
        self.reader_mode = true;
        self.status = "offline readers; F4 tree; F5 next; F6 cancel child; draft never submits";
        let mut lines = std::collections::VecDeque::with_capacity(24);
        if let Some(observation) = &projection.observation {
            for (child, slot) in observation.children.iter().enumerate() {
                let base = child as u64 * 16;
                lines.push_back(Line {
                    id: base,
                    text: format!(
                        "Reader {child} {}/{} {:?} | provisional {:?} {} bytes",
                        observation.session,
                        observation.generation,
                        slot.request_id,
                        slot.phase,
                        slot.received
                    ),
                    hard: true,
                });
                let (state, summary) = match slot.fact.as_ref().map(|fact| &fact.state) {
                    Some(ReaderState::Completed { summary }) => ("Completed", summary.as_str()),
                    Some(ReaderState::Cancelled) => ("Cancelled", ""),
                    Some(ReaderState::Failed(_)) => ("Failed", ""),
                    Some(ReaderState::NotDispatched) => ("NotDispatched", ""),
                    Some(ReaderState::Interrupted) => ("Interrupted", ""),
                    None => ("pending", ""),
                };
                lines.push_back(Line {
                    id: base + 1,
                    text: format!(
                        "{} {state} | acceptance order {:?} | cancel_requested={}",
                        if slot.acceptance_ordinal.is_some() {
                            "Accepted"
                        } else if slot.fact.is_some() {
                            "Final after End"
                        } else {
                            "No accepted summary"
                        },
                        slot.acceptance_ordinal,
                        child_mask & (1 << child) != 0
                    ),
                    hard: true,
                });
                let excerpt = excerpt(summary);
                for (index, text) in excerpt.pieces.into_iter().enumerate() {
                    lines.push_back(Line {
                        id: base + 2 + index as u64,
                        text,
                        hard: true,
                    });
                }
                lines.push_back(Line {
                    id: base + 10,
                    text: format!("Accounting {:?}", slot.accounting),
                    hard: true,
                });
            }
        }
        if let Some((id, offset)) = self.anchor {
            self.anchor = lines
                .iter()
                .find(|line| line.id == id)
                .map(|line| {
                    let mut offset = offset.min(line.text.len());
                    while !line.text.is_char_boundary(offset) {
                        offset -= 1;
                    }
                    (id, offset)
                })
                .or_else(|| {
                    lines
                        .iter()
                        .find(|line| line.id >= id)
                        .map(|line| (line.id, 0))
                });
        }
        self.lines = lines;
        let accepted = projection.observation.as_ref().map_or(0, |o| {
            o.children
                .iter()
                .filter(|s| s.acceptance_ordinal.is_some())
                .count()
        });
        self.runtime_summary = Some(format!(
            "selected={selected} accepted={accepted}/3 child_requested={child_mask:03b} tree_requested={tree_requested} {}{}",
            if projection.ended {
                "cleanup report retained"
            } else {
                "running"
            },
            if projection.error.is_some() {
                "; JOURNEY ERROR"
            } else {
                ""
            }
        ));
    }
}
