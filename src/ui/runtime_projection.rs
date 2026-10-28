use super::{Line, Model};
use crate::runtime_ui_demo::Projection;
use unicode_segmentation::UnicodeSegmentation;

pub(super) struct Excerpt {
    pub(super) pieces: Vec<String>,
    pub(super) replaced: bool,
    pub(super) clipped: bool,
}
pub(super) fn excerpt(source: &str) -> Excerpt {
    let mut safe = String::with_capacity(8192);
    let mut replaced = false;
    let mut clipped = false;
    for character in source.chars() {
        let shown = if character.is_control()
            || matches!(character, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            replaced = true;
            '�'
        } else {
            character
        };
        if safe.len() + shown.len_utf8() > 8192 {
            clipped = true;
            break;
        }
        safe.push(shown);
    }
    let mut pieces = Vec::with_capacity(8);
    let mut piece = String::with_capacity(1024);
    for grapheme in safe.graphemes(true) {
        let shown = if grapheme.len() > 64 {
            replaced = true;
            "�"
        } else {
            grapheme
        };
        if piece.len() + shown.len() > 1024 {
            pieces.push(piece);
            if pieces.len() == 8 {
                clipped = true;
                return Excerpt {
                    pieces,
                    replaced,
                    clipped,
                };
            }
            piece = String::with_capacity(1024);
        }
        piece.push_str(shown);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    Excerpt {
        pieces,
        replaced,
        clipped,
    }
}
impl Model {
    pub(super) fn runtime() -> Self {
        let mut model = Self::new();
        model.lines.clear();
        model.status = "offline requests; F4 cancel all; draft never submits";
        model.runtime_summary = Some("waiting for first paint; no requests started".into());
        model
    }
    pub(super) fn runtime_projection(&mut self, projection: &Projection, cancel_requested: bool) {
        let mut lines = std::collections::VecDeque::with_capacity(40);
        for (ordinal, slot) in projection.slots.iter().enumerate() {
            let base = (ordinal * 16) as u64;
            lines.push_back(Line {
                id: base,
                text: format!(
                    "Request {} {:?} | provisional excerpt, received {} bytes",
                    ordinal + 1,
                    slot.id,
                    slot.received
                ),
                hard: true,
            });
            let excerpt = excerpt(&slot.prefix);
            let count = excerpt.pieces.len();
            for (index, text) in excerpt.pieces.into_iter().enumerate() {
                lines.push_back(Line {
                    id: base + index as u64 + 1,
                    text,
                    hard: index + 1 == count,
                });
            }
            lines.push_back(Line {
                id: base + 10,
                text: format!(
                    "Result {:?} | usage {:?} | clipped={} replaced={}",
                    slot.terminal.as_ref().map(|terminal| &terminal.outcome),
                    slot.terminal
                        .as_ref()
                        .map(|terminal| terminal.synthetic_usage),
                    slot.clipped || excerpt.clipped,
                    excerpt.replaced
                ),
                hard: true,
            });
        }
        if let Some((id, offset)) = self.anchor {
            if let Some(line) = lines.iter().find(|line| line.id == id) {
                let mut offset = offset.min(line.text.len());
                while !line.text.is_char_boundary(offset) {
                    offset -= 1;
                }
                self.anchor = Some((id, offset));
            } else {
                self.anchor = lines
                    .iter()
                    .find(|line| line.id >= id)
                    .map(|line| (line.id, 0))
                    .or_else(|| lines.back().map(|line| (line.id, 0)));
            }
        }
        self.lines = lines;
        let finals = projection
            .slots
            .iter()
            .filter(|slot| slot.terminal.is_some())
            .count();
        let outcomes = projection
            .slots
            .iter()
            .map(
                |slot| match slot.terminal.as_ref().map(|terminal| &terminal.outcome) {
                    Some(scramjet_rs::Outcome::Success) => 'S',
                    Some(scramjet_rs::Outcome::Cancelled) => 'C',
                    Some(_) => 'F',
                    None => '-',
                },
            )
            .collect::<String>();
        self.runtime_summary = Some(format!(
            "{outcomes} final={finals}/4 cancel_requested={cancel_requested} dense={} {}{}",
            projection.slots[2].received,
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
#[cfg(test)]
mod tests {
    use super::super::{selection_text, wrap_line};
    use super::*;
    use crate::runtime_ui_demo::{Projection, Slot};
    #[test]
    fn unicode_controls_and_fragment_boundaries_are_display_only() {
        let source = "e\u{301}界👩\u{200d}💻\x1b]52;c;payload\x07\u{85}\u{202e}";
        let result = excerpt(source);
        assert!(result.replaced);
        assert!(!result.clipped);
        assert_eq!(result.pieces.concat(), "é界👩‍💻�]52;c;payload���");
        assert!(
            result
                .pieces
                .iter()
                .all(|piece| piece.len() <= 1024 && !piece.chars().any(char::is_control))
        );
        assert!(excerpt(&"\x1b".repeat(4096)).clipped);
        let result = excerpt(&format!("e{}", "\u{301}".repeat(100)));
        assert_eq!(result.pieces.concat(), "�");
    }
    #[test]
    fn changing_grapheme_tail_preserves_displayed_selection_and_live_editor() {
        let mut projection = Projection::default();
        projection.slots[0].prefix = "e".into();
        let mut model = Model::runtime();
        model.runtime_projection(&projection, false);
        model.anchor = Some((0, 0));
        model.displayed(model.project(80, 24));
        model.selection = Some(((0, 1), (0, 1)));
        model.displayed(model.project(80, 24));
        assert_eq!(model.copied, "e");
        projection.slots[0].prefix.push('\u{301}');
        model.runtime_projection(&projection, true);
        model.displayed(model.project(80, 24));
        assert_eq!(model.copied, "e");
        model.event(super::super::Event::Paste("hé\n界🙂".into()));
        assert_eq!(model.editor, "lefthé\n界🙂right");
        assert_eq!(model.caret, "lefthé\n界🙂".len());
        model.clear_selection();
        model.displayed(model.project(80, 24));
        model.selection = Some(((0, 1), (0, 1)));
        model.displayed(model.project(80, 24));
        assert_eq!(model.copied, "é");
        model.event(super::super::Event::Focus(false));
        model.event(super::super::Event::CancelAll);
        assert!(!model.focused);
        assert_eq!(model.editor, "lefthé\n界🙂right");
    }

    #[test]
    fn stable_soft_pieces_selection_and_draft_survive_projection() {
        let mut projection = Projection::default();
        projection.slots[0] = Slot {
            prefix: "x".repeat(2048),
            ..Slot::default()
        };
        let mut model = Model::runtime();
        model.runtime_projection(&projection, false);
        let pieces = model
            .lines
            .iter()
            .filter(|line| matches!(line.id, 1 | 2))
            .cloned()
            .collect::<Vec<_>>();
        assert!(!pieces[0].hard && pieces[1].hard);
        let rows = pieces
            .iter()
            .flat_map(|line| wrap_line(line, 200))
            .collect::<Vec<_>>();
        let display = super::super::Display {
            rows,
            ..super::super::Display::default()
        };
        let copied = selection_text(
            &display,
            Some(((0, 0), (199, display.rows.len() as u16 - 1))),
        );
        assert_eq!(copied, "x".repeat(2048));
        model.anchor = Some((1, 1000));
        projection.slots[0].prefix = "é".into();
        model.runtime_projection(&projection, true);
        assert_eq!(model.anchor, Some((1, 2)));
        assert_eq!(model.editor, "leftright");
        assert_eq!(model.caret, 4);
        assert!(model.lines.len() <= 40);
    }
}
