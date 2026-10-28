use super::journal::*;
use super::*;
use crate::{CodingHistoryEnding, CodingHistoryEntry, OperationState, ReaderInputs, ReaderTree};

pub struct ProjectHistory {
    pub header: ProjectHeader,
    pub records: Vec<Arc<ProjectRecord>>,
    pub coding: [Option<CodingHistory>; 2],
    pub readers: [Option<ReaderTree>; 2],
    pub closure_recorded: bool,
}
fn invalid(message: &'static str) -> CodingError {
    CodingError::History(message)
}
fn valid_report(report: &ProjectReport) -> Result<(), CodingError> {
    if !matches!(
        report.completion.as_str(),
        "Completed" | "Failed" | "Cancelled" | "Incomplete"
    ) || report.summary.len() > 512
        || report.summary.chars().any(char::is_control)
    {
        return Err(invalid("invalid project report"));
    }
    Ok(())
}
fn valid_file(file: &ProjectFile) -> Result<(), CodingError> {
    validate_project_name(&file.name)?;
    validate_content(&file.content)?;
    if !matches!(file.mode, 0o600 | 0o644) {
        return Err(invalid("unsupported recorded project mode"));
    }
    Ok(())
}
fn validate_goal(
    captured: &ProjectCaptured,
    readers: &[ProjectReaderFact; 3],
    history: &CodingHistory,
) -> Result<(), CodingError> {
    let goal = history
        .records()
        .iter()
        .find_map(|record| match &record.entry {
            CodingHistoryEntry::Material(m) if m.event == "ProjectGoal" => {
                serde_json::from_str::<context::Goal>(&m.text).ok()
            }
            _ => None,
        })
        .ok_or(invalid("coding artifact lacks strict project goal"))?;
    goal.validate()?;
    if goal.evidence.root != history.root()
        || history.session() != captured.coding_session
        || goal.evidence.request != captured.request
        || goal.evidence.capture != captured.files.each_ref().map(|f| f.identity)
        || goal.evidence.reader_session != captured.reader_session
        || goal.evidence.generation != captured.journey as u64
        || goal.expected != captured.files[0].content
        || goal.evidence.readers.iter().zip(readers).any(|(e, r)| {
            e.child != r.child
                || Some(e.request) != r.request
                || Some(e.summary.as_str()) != r.summary.as_deref()
        })
    {
        return Err(invalid(
            "coding evidence differs from captured reader artifacts",
        ));
    }
    Ok(())
}

pub(crate) fn validate_coding(
    captured: &ProjectCaptured,
    readers: &ProjectReaders,
    coding: &ProjectCoding,
    history: &CodingHistory,
) -> Result<(), CodingError> {
    if coding.owner == 0
        || coding.journey != captured.journey
        || readers.journey != captured.journey
        || !readers.ended
        || !readers.joined
        || readers.facts.iter().any(|r| r.state != "Completed")
        || !matches!(
            coding.completion.as_str(),
            "Completed" | "Failed" | "Incomplete"
        )
        || history.session() != captured.coding_session
    {
        return Err(invalid("coding source/reader/header correlation mismatch"));
    }
    validate_goal(captured, &readers.facts, history)?;
    let response_ids: Vec<_> = history
        .records()
        .iter()
        .filter_map(|r| match &r.entry {
            CodingHistoryEntry::Material(m) if m.event == "Response" => Some(m.request),
            _ => None,
        })
        .collect();
    for (index, id) in response_ids.iter().enumerate() {
        if index >= 4 || coding.requests[index] != Some(*id) {
            return Err(invalid("coding response/request mismatch"));
        }
    }
    let effect = history.operations().last().map(|f| f.state);
    let effect_valid = match coding.effect.as_str() {
        "None" => effect.is_none(),
        "Authorized" => effect == Some(OperationState::Authorized),
        "NotDispatched" => effect == Some(OperationState::NotDispatched),
        "Completed" => effect == Some(OperationState::Completed),
        "Uncertain" => matches!(
            effect,
            Some(OperationState::Uncertain | OperationState::Completed)
        ),
        _ => false,
    };
    if !effect_valid {
        return Err(invalid("project effect contradicts coding artifact"));
    }
    let summary = history.records().iter().rev().find_map(|r| match &r.entry {
        CodingHistoryEntry::Material(m) if m.event == "Final" => Some(m.text.as_str()),
        _ => None,
    });
    if coding
        .summary
        .as_deref()
        .is_some_and(|s| Some(s) != summary)
    {
        return Err(invalid("project final summary mismatch"));
    }
    if coding.completion == "Completed"
        && (!coding.joined
            || coding.effect != "Completed"
            || history.ending() != CodingHistoryEnding::RecordedFinal
            || response_ids.len() != 4
            || coding.requests.iter().any(Option::is_none)
            || coding.summary.as_deref() != summary
            || coding.error.is_some()
            || coding.edited.is_none())
    {
        return Err(invalid(
            "source success lacks complete joined artifact evidence",
        ));
    }
    if let Some(file) = &coding.edited {
        let verified = history
            .records()
            .iter()
            .find_map(|record| match &record.entry {
                CodingHistoryEntry::Material(material) if material.event == "VerifiedIdentity" => {
                    serde_json::from_str::<RootIdentity>(&material.text).ok()
                }
                _ => None,
            });
        if verified != Some(file.identity) {
            return Err(invalid("source file differs from verified coding identity"));
        }
        valid_file(file)?;
        if file.name != captured.request.target()
            || file.mode != captured.files[0].mode
            || file.content != captured.desired
            || coding.effect != "Completed"
        {
            return Err(invalid("edited target differs from captured goal"));
        }
    }
    Ok(())
}
fn storage_child(storage: &File, name: &str, expected: RootIdentity) -> Result<File, CodingError> {
    let file = File::from(openat(
        storage,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    validate_directory(&file, true)?;
    if identity(&file)? != expected {
        return Err(invalid("referenced storage identity mismatch"));
    }
    Ok(file)
}

pub fn reconstruct_project_history(
    storage: &Path,
    session: &str,
) -> Result<ProjectHistory, CodingError> {
    crate::coding::validate_session(session)?;
    let storage = open_directory(storage)?;
    let records = journal::read(&storage)?;
    let ProjectRecord::Header(header) = records[0].as_ref() else {
        return Err(invalid("missing project header"));
    };
    if header.version != 1
        || header.session != session
        || header.instance == 0
        || header.storage != identity(&storage)?
        || !Path::new(&header.project_path).is_absolute()
    {
        return Err(invalid("project header identity/version mismatch"));
    }
    let mut requests: [Option<&ProjectRequested>; 2] = [None, None];
    let mut captures: [Option<&ProjectCaptured>; 2] = [None, None];
    let mut reader_records: [Option<&ProjectReaders>; 2] = [None, None];
    let mut coding_records: [Option<&ProjectCoding>; 2] = [None, None];
    let mut source_reports: [Option<&ProjectReport>; 2] = [None, None];
    let mut verification_reports: [Option<&ProjectReport>; 2] = [None, None];
    let mut reads: [Option<&ProjectRead>; 2] = [None, None];
    let mut phases = [0u8; 2];
    let mut current = 0;
    let mut closed = false;
    let mut cancellation_reported = false;
    let mut coding: [Option<CodingHistory>; 2] = std::array::from_fn(|_| None);
    let mut readers: [Option<ReaderTree>; 2] = std::array::from_fn(|_| None);
    for record in records.iter().skip(1) {
        if closed {
            return Err(invalid("project record after closure"));
        }
        let journey = match record.as_ref() {
            ProjectRecord::Header(_) => return Err(invalid("duplicate project header")),
            ProjectRecord::Requested(r) => r.journey,
            ProjectRecord::Captured(c) => c.journey,
            ProjectRecord::Readers(r) => r.journey,
            ProjectRecord::Coding(c) => c.journey,
            ProjectRecord::SourceReport(r) | ProjectRecord::VerificationReport(r) => r.journey,
            ProjectRecord::VerificationReady(r) => r.journey,
            ProjectRecord::VerificationRead(r) => r.journey,
            ProjectRecord::Closure(closure) => {
                if closure.attempts > 14 || closure.unknown_usage > 14 || closure.unresolved > 4 {
                    return Err(invalid("invalid project closure bounds"));
                }
                let eligible = current == 0
                    || match phases[current as usize - 1] {
                        4 => source_reports[current as usize - 1].is_some_and(|r| {
                            matches!(r.completion.as_str(), "Completed" | "Failed" | "Cancelled")
                        }),
                        7 => true,
                        8 => verification_reports[current as usize - 1].is_some_and(|r| {
                            matches!(r.completion.as_str(), "Failed" | "Cancelled")
                        }),
                        _ => false,
                    };
                if (closure.logical_settled && !eligible)
                    || (cancellation_reported && !closure.cancelled)
                {
                    return Err(invalid(
                        "closure contradicts logical/cancellation acknowledgement",
                    ));
                }
                closed = true;
                continue;
            }
        };
        if !(1..=2).contains(&journey) {
            return Err(invalid("project journey ceiling"));
        }
        let index = journey as usize - 1;
        if !matches!(record.as_ref(), ProjectRecord::Requested(_)) && journey != current {
            return Err(invalid("cross-journey record ordering"));
        }
        match record.as_ref() {
            ProjectRecord::Requested(request) => {
                if journey != current + 1 || (current != 0 && phases[current as usize - 1] != 7) {
                    return Err(invalid("project request admission ordering"));
                }
                request.request.validate()?;
                current = journey;
                requests[index] = Some(request);
                phases[index] = 9;
            }
            ProjectRecord::Captured(capture) if phases[index] == 9 => {
                if requests[index].is_none_or(|request| request.request != capture.request) {
                    return Err(invalid("capture differs from admitted request"));
                }
                capture.request.validate()?;
                if capture.files.each_ref().map(|f| f.name.as_str()) != capture.request.names()
                    || capture.desired != capture.request.derive(&capture.files[0].content)?
                    || capture.reader_session != format!("p-{}-{journey}-r", header.instance)
                    || capture.coding_session != format!("p-{}-{journey}-c", header.instance)
                {
                    return Err(invalid("captured project provenance mismatch"));
                }
                for file in &capture.files {
                    valid_file(file)?;
                }
                storage_child(
                    &storage,
                    &format!("readers-{journey}"),
                    capture.reader_storage,
                )?;
                current = journey;
                captures[index] = Some(capture);
                phases[index] = 1;
            }
            ProjectRecord::Readers(record) if phases[index] == 1 => {
                let captured = captures[index].unwrap();
                let inputs = ReaderInputs::new(
                    &captured.reader_session,
                    journey as u64,
                    captured.files.each_ref().map(|f| f.content.as_str()),
                )
                .map_err(|_| invalid("reader input provenance"))?;
                let directory = storage_child(
                    &storage,
                    &format!("readers-{journey}"),
                    captured.reader_storage,
                )?;
                let recovered = crate::readers::journal::reconstruct_descriptor(directory, &inputs);
                match recovered {
                    Ok(tree) => {
                        if tree.parent_finished != record.ended {
                            return Err(invalid("reader End disagreement"));
                        }
                        for (index, (actual, stored)) in
                            tree.children.iter().zip(&record.facts).enumerate()
                        {
                            if stored.child as usize != index
                                || stored.request == Some(0)
                                || (stored.request.is_some()
                                    && record.facts[..index]
                                        .iter()
                                        .any(|f| f.request == stored.request))
                            {
                                return Err(invalid("reader request membership/correlation"));
                            }
                            let actual = super::session::reader_fact(actual);
                            if record.ended || actual.state == "Completed" {
                                if actual != *stored {
                                    return Err(invalid("reader accepted evidence mismatch"));
                                }
                            } else if stored.summary.is_some()
                                || !matches!(
                                    stored.state.as_str(),
                                    "Interrupted" | "NotDispatched" | "Cancelled"
                                )
                            {
                                return Err(invalid("unsupported unacknowledged reader state"));
                            }
                        }
                        readers[index] = Some(tree);
                    }
                    Err(crate::ReaderError::Io(error))
                        if error.kind() == std::io::ErrorKind::NotFound
                            && !record.ended
                            && record.error.is_some()
                            && record.facts.iter().all(|r| r.summary.is_none()) => {}
                    Err(_) => return Err(invalid("missing/invalid acknowledged reader artifact")),
                }
                reader_records[index] = Some(record);
                phases[index] = 2;
            }
            ProjectRecord::Coding(record) if phases[index] == 2 => {
                let captured = captures[index].unwrap();
                let directory =
                    storage_child(&storage, &format!("coding-{journey}"), record.storage)?;
                let history = crate::coding::history::from_storage(
                    &directory,
                    &captured.coding_session,
                    header.project,
                    true,
                )?;
                validate_coding(captured, reader_records[index].unwrap(), record, &history)?;
                if index == 1 && coding_records[0].is_some_and(|first| first.owner == record.owner)
                {
                    return Err(invalid("reused coding owner"));
                }
                coding[index] = Some(history);
                coding_records[index] = Some(record);
                phases[index] = 3;
            }
            ProjectRecord::SourceReport(report) if phases[index] == 3 => {
                valid_report(report)?;
                let source = coding_records[index].unwrap();
                if report.paused != (report.completion == "Completed")
                    || (matches!(report.completion.as_str(), "Completed" | "Failed")
                        && report.completion != source.completion)
                    || (report.completion == "Completed"
                        && source.summary.as_deref() != Some(report.summary.as_str()))
                {
                    return Err(invalid("source report contradicts actual coding evidence"));
                }
                cancellation_reported |= report.completion == "Cancelled";
                source_reports[index] = Some(report);
                phases[index] = 4;
            }
            ProjectRecord::VerificationReady(_)
                if phases[index] == 4 && source_reports[index].unwrap().paused =>
            {
                phases[index] = 5;
            }
            ProjectRecord::VerificationRead(read) if phases[index] == 5 => {
                if let Some(file) = &read.file {
                    valid_file(file)?;
                }
                if read.matched
                    && (read.file.is_none()
                        || read.file.as_ref() != coding_records[index].unwrap().edited.as_ref()
                        || read.error.is_some())
                {
                    return Err(invalid(
                        "verification read contradicts edited project evidence",
                    ));
                }
                if !read.matched && read.error.is_none() {
                    return Err(invalid("verification mismatch lacks error evidence"));
                }
                reads[index] = Some(read);
                phases[index] = 6;
            }
            ProjectRecord::VerificationReport(report) if phases[index] == 6 => {
                valid_report(report)?;
                if report.paused
                    || (matches!(report.completion.as_str(), "Completed" | "Failed")
                        && (report.completion == "Completed") != reads[index].unwrap().matched)
                {
                    return Err(invalid("verification report contradicts actual read"));
                }
                cancellation_reported |= report.completion == "Cancelled";
                verification_reports[index] = Some(report);
                phases[index] = if report.completion == "Completed" {
                    7
                } else {
                    8
                };
            }
            _ => return Err(invalid("invalid project history transition")),
        }
    }
    for index in 0..2 {
        let Some(captured) = captures[index] else {
            continue;
        };
        if readers[index].is_none() {
            let root = storage_child(
                &storage,
                &format!("readers-{}", captured.journey),
                captured.reader_storage,
            )?;
            let inputs = ReaderInputs::new(
                &captured.reader_session,
                captured.journey as u64,
                captured.files.each_ref().map(|f| f.content.as_str()),
            )
            .map_err(|_| invalid("captured reader input provenance"))?;
            match crate::readers::journal::reconstruct_descriptor(root, &inputs) {
                Ok(tree) => readers[index] = Some(tree),
                Err(crate::ReaderError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(invalid("invalid admitted reader artifact prefix")),
            }
        }
        if coding[index].is_none() {
            let name = format!("coding-{}", captured.journey);
            let root = match openat(
                &storage,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(root) => Some(File::from(root)),
                Err(rustix::io::Errno::NOENT) => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(root) = root {
                validate_directory(&root, true)?;
                let artifact = crate::coding::history::from_storage(
                    &root,
                    &captured.coding_session,
                    header.project,
                    true,
                )?;
                if !artifact.records().is_empty() || !artifact.operations().is_empty() {
                    let admitted = reader_records[index]
                        .ok_or(invalid("coding prefix lacks durable reader admission"))?;
                    let tree = readers[index]
                        .as_ref()
                        .ok_or(invalid("coding prefix lacks reader evidence"))?;
                    if !admitted.ended
                        || !admitted.joined
                        || admitted.facts.iter().any(|r| r.state != "Completed")
                        || !tree.parent_finished
                        || tree.children.each_ref().map(super::session::reader_fact)
                            != admitted.facts
                    {
                        return Err(invalid("coding prefix lacks joined accepted reader End"));
                    }
                    validate_goal(captured, &admitted.facts, &artifact)?;
                }
                coding[index] = Some(artifact);
            }
        }
    }
    Ok(ProjectHistory {
        header: header.clone(),
        records,
        coding,
        readers,
        closure_recorded: closed,
    })
}
