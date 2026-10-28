use super::{Line, Model};
use crate::command_ui_demo::{Offer, Projection};
use scramjet_rs::CommandReadResult;
use std::{collections::VecDeque, fmt::Write, io};
const LINE_LIMIT: usize = 512;
const TEXT_LIMIT: usize = 32768;
pub(super) fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 6);
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            c => write!(out, "\\u{{{:X}}}", c as u32).unwrap(),
        }
    }
    out
}
fn lines(p: &Projection) -> io::Result<VecDeque<Line>> {
    let mut lines = VecDeque::with_capacity(LINE_LIMIT);
    let mut bytes = 0;
    let mut push = |id, text: String| -> io::Result<()> {
        if lines.len() >= LINE_LIMIT || bytes + text.len() > TEXT_LIMIT {
            return Err(io::Error::other("command presentation capacity exceeded"));
        }
        bytes += text.len();
        lines.push_back(Line {
            id,
            text,
            hard: true,
        });
        Ok(())
    };
    push(
        0,
        format!(
            "Command host={:?}; F5 invokes current offered action; F4 Stop",
            p.policy
        ),
    )?;
    push(
        1,
        "Quoted ASCII fields: concatenate chunks; \\n LF, \\t TAB, \\u{HEX} scalar".into(),
    )?;
    push(
        2,
        "Quotes/backslashes escaped; source UTF-8 byte lengths are not display lengths".into(),
    )?;
    push(
        3,
        format!(
            "offer={} ended={} error={}",
            p.offer.is_some(),
            p.ended,
            p.error.is_some()
        ),
    )?;
    if let Some(s) = &p.observation {
        push(
            4,
            format!(
                "invocations={} reads={} settled={} cancellation_requested={}",
                s.invocations.iter().flatten().count(),
                s.reads,
                s.settled,
                s.cancellation_requested
            ),
        )?;
        for fact in s.invocations.iter().flatten() {
            let base = fact.id.ordinal() as u64 * 1024;
            push(
                base,
                format!(
                    "session={} invocation={} command=\"{}\" phase={:?}",
                    fact.id.session(),
                    fact.id.ordinal(),
                    fact.command,
                    fact.phase
                ),
            )?;
            if let Some(marker) = p.markers[fact.id.ordinal() as usize - 1] {
                if marker.id != fact.id {
                    return Err(io::Error::other("foreign pre-report marker"));
                }
                push(
                    base + 1,
                    format!(
                        "Observed before report: {:?} reads={} returned_mask={:02b}",
                        marker.phase, marker.reads, marker.steps
                    ),
                )?;
            }
            for step in fact.steps.iter().flatten() {
                let key = base + 10 + step.ordinal as u64 * 128;
                match &step.result {
                    CommandReadResult::Read(text) => {
                        if text.len() > 1024 {
                            return Err(io::Error::other("oversized retained command read"));
                        }
                        push(
                            key,
                            format!(
                                "step={} target=\"{}\" source_bytes={} captured_ascii=\"",
                                step.ordinal,
                                step.target,
                                text.len()
                            ),
                        )?;
                        let encoded = encode(text);
                        for (chunk, bytes) in encoded.as_bytes().chunks(64).enumerate() {
                            push(
                                key + 1 + chunk as u64,
                                std::str::from_utf8(bytes).unwrap().to_owned(),
                            )?;
                        }
                        push(key + 97, "\"".into())?;
                    }
                    CommandReadResult::Failed(e) => push(
                        key,
                        format!(
                            "step={} target=\"{}\" read_failed={e:?}",
                            step.ordinal, step.target
                        ),
                    )?,
                }
            }
            if let Some(report) = &fact.report {
                push(
                    base + 300,
                    format!(
                        "Accepted explicit report={:?} next={:?} summary_ascii=\"",
                        report.completion, report.next
                    ),
                )?;
                for (chunk, bytes) in encode(&report.summary).as_bytes().chunks(64).enumerate() {
                    push(
                        base + 301 + chunk as u64,
                        std::str::from_utf8(bytes).unwrap().to_owned(),
                    )?;
                }
                push(base + 318, "\"".into())?;
            } else {
                push(base + 300, "No accepted explicit report".into())?;
            }
        }
        push(4000, format!("continuation={:?}", s.continuation))?;
    } else {
        push(4, "Loading disposable command offer; no invocation".into())?;
    }
    if let Some(error) = &p.error {
        if error.len() > 512 {
            return Err(io::Error::other("oversized command error"));
        }
        push(5000, "Error ASCII=\"".into())?;
        for (chunk, bytes) in encode(error).as_bytes().chunks(64).enumerate() {
            push(
                5001 + chunk as u64,
                std::str::from_utf8(bytes).unwrap().to_owned(),
            )?;
        }
        push(5050, "\"".into())?;
    }
    Ok(lines)
}
impl Model {
    pub(super) fn command_projection(&mut self, p: &Projection) -> io::Result<()> {
        let lines = lines(p)?;
        self.command_mode = true;
        if let Some((id, offset)) = self.anchor {
            self.anchor = lines
                .iter()
                .find(|l| l.id == id)
                .map(|l| (id, offset.min(l.text.len())))
                .or_else(|| lines.iter().find(|l| l.id >= id).map(|l| (l.id, 0)));
        }
        self.lines = lines;
        self.status = match p.offer {
            Some(Offer::Compare(_)) => {
                "offline command; F5 invoke compare; F4 Stop; draft never submits"
            }
            Some(Offer::Resume(_)) => {
                "offline command; F5 resume inspect; F4 Stop; draft never submits"
            }
            None => "offline command; no offered action; F4 Stop; draft never submits",
        };
        self.runtime_summary = Some(p.observation.as_ref().map_or_else(
            || "offer loading; reads=0".into(),
            |s| {
                format!(
                    "invocations={} reads={} settled={} host={:?} continuation={:?}",
                    s.invocations.iter().flatten().count(),
                    s.reads,
                    s.settled,
                    p.policy,
                    s.continuation
                )
            },
        ));
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_ui_demo::tests::maximum_projection;
    #[test]
    fn maximum_control_heavy_fields_are_complete_safe_bounded_and_keyed() {
        let p = maximum_projection();
        let lines = lines(&p).unwrap();
        assert!(lines.len() <= LINE_LIMIT);
        assert!(lines.iter().map(|l| l.text.len()).sum::<usize>() <= TEXT_LIMIT);
        assert!(
            lines
                .iter()
                .all(|l| l.text.bytes().all(|b| (32..=126).contains(&b)))
        );
        let ids: std::collections::HashSet<_> = lines.iter().map(|l| l.id).collect();
        assert_eq!(ids.len(), lines.len());
        for ordinal in 1..=2 {
            for step in 0..2 {
                let key = ordinal * 1024 + 10 + step * 128;
                let field: String = lines
                    .iter()
                    .filter(|l| l.id > key && l.id < key + 97)
                    .map(|l| l.text.as_str())
                    .collect();
                assert_eq!(field, "\\u{1B}".repeat(1024));
                assert_eq!(field.len(), 6144);
            }
        }
        let mut m = Model::runtime();
        m.command_projection(&p).unwrap();
        m.anchor = Some((1035, 60));
        m.editor = "retained draft".into();
        m.command_projection(&p).unwrap();
        assert_eq!(m.anchor, Some((1035, 60)));
        assert_eq!(m.editor, "retained draft");
        for (w, h) in [(140, 24), (8, 6), (140, 30)] {
            let display = m.project(w, h);
            assert!(display.text_bytes <= super::super::FRAME_LIMIT);
            m.displayed(display);
        }
        m.selection = Some(((0, 0), (12, 0)));
        m.displayed(m.project(140, 30));
        let copied = m.copied.clone();
        assert!(!copied.is_empty());
        let mut changed = p.clone();
        if let Some(scramjet_rs::CommandReadResult::Read(text)) =
            changed.observation.as_mut().unwrap().invocations[0]
                .as_mut()
                .unwrap()
                .steps[0]
                .as_mut()
                .map(|s| &mut s.result)
        {
            *text = "future replacement".into();
        }
        m.command_projection(&changed).unwrap();
        m.displayed(m.project(140, 30));
        assert_eq!(
            m.copied, copied,
            "selection copies the last displayed field, not changed source"
        );
        assert_eq!(
            encode("\"\\\n\t\x1b\u{202e}界"),
            "\\\"\\\\\\n\\t\\u{1B}\\u{202E}\\u{754C}"
        );
    }
}
