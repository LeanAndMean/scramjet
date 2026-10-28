use super::{Line, Model, command_projection::encode};
use crate::command_reader_ui_demo::Projection;
use scramjet_rs::ReaderState;
use std::{collections::VecDeque, io};
fn lines(p: &Projection, selected: u8, mask: u8, stopped: bool) -> io::Result<VecDeque<Line>> {
    let mut lines = VecDeque::with_capacity(256);
    let mut bytes = 0;
    let mut push = |id, text: String| -> io::Result<()> {
        if lines.len() == 512 || bytes + text.len() > 32768 {
            return Err(io::Error::other("command reader presentation ceiling"));
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
        "Offline AllReaders; F5 invoke; F7 next child; F6 cancel child; F4 Stop".into(),
    )?;
    push(
        1,
        format!(
            "offer={} selected={selected} child_requested={mask:03b} Stop={stopped}",
            p.offer.is_some()
        ),
    )?;
    push(
        2,
        "Model summaries are not independently verified conclusions.".into(),
    )?;
    push(
        3,
        "Quoted ASCII chunks: concatenate; \\n LF, \\t TAB, \\u{HEX} scalar".into(),
    )?;
    if let Some(command) = &p.command {
        push(
            4,
            format!(
                "invocations={} reads={} settled={} cancellation_requested={}",
                command.invocations.iter().flatten().count(),
                command.reads,
                command.settled,
                command.cancellation_requested
            ),
        )?;
        if let Some(fact) = &command.invocations[0] {
            push(
                5,
                format!(
                    "command={} session={} invocation={} phase={:?}",
                    fact.command,
                    fact.id.session(),
                    fact.id.ordinal(),
                    fact.phase
                ),
            )?;
            if p.before_report {
                push(6, "Observed before report: AwaitingCompletion".into())?;
            }
            if let Some(report) = &fact.report {
                if report.summary.len() > 512 {
                    return Err(io::Error::other("command report ceiling"));
                }
                push(
                    100,
                    format!(
                        "Accepted explicit report={:?} summary_ascii=\"",
                        report.completion
                    ),
                )?;
                for (i, chunk) in encode(&report.summary).as_bytes().chunks(64).enumerate() {
                    push(101 + i as u64, std::str::from_utf8(chunk).unwrap().into())?;
                }
                push(180, "\"".into())?;
            } else {
                push(100, "No accepted explicit report".into())?;
            }
            if let Some(readers) = &fact.readers {
                push(
                    190,
                    format!(
                        "logical_receipt={} cleanup={:?}",
                        readers.receipt_accepted, readers.cleanup
                    ),
                )?;
            }
        }
    } else {
        push(4, "Loading command offer; no child dispatch".into())?;
    }
    if let Some(readers) = &p.readers {
        push(
            200,
            format!(
                "reader session={}/{} End={} storage_joined={} runtime_joined={}",
                readers.session,
                readers.generation,
                readers.tree_ended,
                readers.storage_joined,
                readers.runtime_joined
            ),
        )?;
        for (child, slot) in readers.children.iter().enumerate() {
            let base = 1000 + child as u64 * 256;
            push(
                base,
                format!(
                    "Reader {child} id={:?} provisional={:?} bytes={} acceptance_order={:?}",
                    slot.request_id, slot.phase, slot.received, slot.acceptance_ordinal
                ),
            )?;
            let (label, summary) = match slot.fact.as_ref().map(|f| &f.state) {
                Some(ReaderState::Completed { summary }) => ("Completed", summary.as_str()),
                Some(ReaderState::Cancelled) => ("Cancelled", ""),
                Some(ReaderState::Failed(_)) => ("Failed", ""),
                Some(ReaderState::NotDispatched) => ("NotDispatched", ""),
                Some(ReaderState::Interrupted) => ("Interrupted", ""),
                None => ("pending", ""),
            };
            push(
                base + 1,
                format!(
                    "{} {label}; child_cancel_requested={}",
                    if slot.acceptance_ordinal.is_some() {
                        "Accepted"
                    } else {
                        "Unaccepted/final after End"
                    },
                    mask & (1 << child) != 0
                ),
            )?;
            if summary.len() > 512 {
                return Err(io::Error::other("reader summary ceiling"));
            }
            push(
                base + 2,
                format!("summary_source_bytes={} ascii=\"", summary.len()),
            )?;
            for (i, chunk) in encode(summary).as_bytes().chunks(64).enumerate() {
                push(
                    base + 3 + i as u64,
                    std::str::from_utf8(chunk).unwrap().into(),
                )?;
            }
            push(base + 60, "\"".into())?;
            push(base + 61, format!("Accounting {:?}", slot.accounting))?;
        }
    }
    push(
        4000,
        format!(
            "host ended={} fixture_joined={} reconstructed={} root_retained={} error={}",
            p.ended,
            p.fixture.is_some(),
            p.reconstructed,
            p.retained_root.is_some(),
            p.error.is_some()
        ),
    )?;
    if let Some(error) = &p.error {
        if error.len() > 512 {
            return Err(io::Error::other("command reader error ceiling"));
        }
        push(5000, "Error ASCII=\"".into())?;
        for (i, chunk) in encode(error).as_bytes().chunks(64).enumerate() {
            push(5001 + i as u64, std::str::from_utf8(chunk).unwrap().into())?;
        }
        push(5050, "\"".into())?;
    }
    Ok(lines)
}

impl Model {
    pub(super) fn command_reader_projection(
        &mut self,
        p: &Projection,
        selected: u8,
        mask: u8,
        stopped: bool,
    ) -> io::Result<()> {
        #[cfg(test)]
        if std::env::var_os("S412_PANIC_CHILD").is_some() && p.readers.is_some() {
            panic!("synthetic command reader projection panic");
        }
        let lines = lines(p, selected, mask, stopped)?;
        if let Some((id, offset)) = self.anchor {
            self.anchor = lines
                .iter()
                .find(|l| l.id == id)
                .map(|l| (id, offset.min(l.text.len())))
                .or_else(|| lines.iter().find(|l| l.id >= id).map(|l| (l.id, 0)));
        }
        self.lines = lines;
        self.command_mode = true;
        self.status =
            "offline AllReaders; F5 invoke; F7 child; F6 cancel; F4 Stop; draft never submits";
        let report = p
            .command
            .as_ref()
            .and_then(|c| c.invocations[0].as_ref())
            .and_then(|f| f.report.as_ref());
        self.runtime_summary = Some(format!(
            "selected={selected} offer={} {} settled={}",
            p.offer.is_some(),
            report.map_or_else(
                || "No accepted explicit report".into(),
                |r| format!("Accepted explicit report={:?}", r.completion)
            ),
            p.command.as_ref().is_some_and(|c| c.settled)
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet_rs::*;
    #[test]
    fn maximum_composed_projection_is_complete_ascii_bounded_keyed_and_retains_editor_selection() {
        let roots =
            crate::command_fixture::Roots::new(crate::command_readers_demo::CATALOG, b"", b"")
                .unwrap();
        let mut command =
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).unwrap();
        let input = "\t".repeat(1024);
        command.begin_readers("summarize", [&input; 3]).unwrap();
        let mut snapshot = command.snapshot();
        let fact = snapshot.invocations[0].as_mut().unwrap();
        fact.phase = CommandPhase::Completed;
        fact.report = Some(CommandReportFact {
            completion: CommandCompletion::Completed,
            summary: "\\".repeat(512),
            next: None,
        });
        let session = fact.readers.as_ref().unwrap().session.clone();
        let p = Projection {
            command: Some(snapshot),
            readers: Some(ReaderObservation {
                session: session.to_string(),
                generation: 1,
                children: std::array::from_fn(|i| ReaderSlot {
                    request_id: Some(RequestId(i as u64 + 1)),
                    phase: ReaderPhase::Terminal,
                    received: 2048,
                    accounting: Some(ReaderAccounting {
                        request_id: RequestId(i as u64 + 1),
                        unknown_usage: true,
                        unresolved_exposure: true,
                    }),
                    fact: Some(ReaderFact {
                        child: i as u8,
                        request_id: Some(RequestId(i as u64 + 1)),
                        state: ReaderState::Completed {
                            summary: "\u{a0}".repeat(256),
                        },
                    }),
                    acceptance_ordinal: Some(i as u8 + 1),
                }),
                tree_ended: true,
                storage_joined: false,
                runtime_joined: false,
                error: None,
            }),
            before_report: true,
            error: Some("\x1b".repeat(512)),
            ..Projection::default()
        };
        let projected = lines(&p, 2, 7, true).unwrap();
        assert!(
            projected.len() <= 512
                && projected.iter().map(|l| l.text.len()).sum::<usize>() <= 32768
        );
        assert!(
            projected
                .iter()
                .all(|l| l.text.bytes().all(|b| (32..=126).contains(&b)))
        );
        let ids: std::collections::HashSet<_> = projected.iter().map(|l| l.id).collect();
        assert_eq!(ids.len(), projected.len());
        for child in 0..3 {
            let base = 1000 + child * 256;
            let encoded: String = projected
                .iter()
                .filter(|l| l.id >= base + 3 && l.id < base + 60)
                .map(|l| l.text.as_str())
                .collect();
            assert_eq!(encoded, "\\u{A0}".repeat(256));
        }
        let mut model = Model::runtime();
        model.command_reader_projection(&p, 2, 7, true).unwrap();
        model.anchor = Some((1003, 3));
        model.editor = "retained\n界🙂".into();
        model.command_reader_projection(&p, 2, 7, true).unwrap();
        assert_eq!(model.anchor, Some((1003, 3)));
        assert_eq!(model.editor, "retained\n界🙂");
        for (w, h) in [(200, 60), (8, 6), (120, 24)] {
            let display = model.project(w, h);
            assert!(display.text_bytes <= super::super::FRAME_LIMIT);
            model.displayed(display);
        }
        model.selection = Some(((0, 0), (20, 0)));
        model.displayed(model.project(120, 24));
        let copied = model.copied.clone();
        assert!(!copied.is_empty());
        let mut changed = p.clone();
        changed.readers.as_mut().unwrap().children[0]
            .fact
            .as_mut()
            .unwrap()
            .state = ReaderState::Cancelled;
        model
            .command_reader_projection(&changed, 0, 0, false)
            .unwrap();
        model.displayed(model.project(120, 24));
        assert_eq!(model.copied, copied);
    }
}
