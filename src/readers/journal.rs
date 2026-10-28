use super::*;
use rustix::fs::{FileType, FlockOperation, Mode, OFlags, flock, fstat, open, openat};
use std::{
    fs::File,
    io::{Read, Write},
};
const NAME: &str = ".reader-journal";
const RECORD: usize = 4096;
const TOTAL: usize = 32768;
const RECORDS: usize = 11;

pub(super) enum Record {
    Started(u8),
    Terminal {
        child: u8,
        id: Option<RequestId>,
        state: ReaderState,
        response: String,
    },
    End(u8),
    Accept {
        child: u8,
        cancelled: bool,
    },
}
pub(super) struct Journal {
    file: File,
    _root: File,
    records: usize,
    bytes: usize,
    poisoned: bool,
    #[cfg(test)]
    pub(super) accept_fault: Option<Fault>,
    #[cfg(test)]
    pub(super) accept_gate: Option<(
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
    #[cfg(test)]
    fault: Option<Fault>,
    #[cfg(test)]
    pub(super) limits: (usize, usize),
    #[cfg(test)]
    pub(super) gate: Option<(
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
}
#[cfg(test)]
pub(super) enum Fault {
    Partial,
    Append,
    Sync,
}

fn root(path: &Path) -> Result<(File, u64, u64), ReaderError> {
    let file = File::from(open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let stat = fstat(&file)?;
    if stat.st_mode & 0o7777 != 0o700 {
        return Err(ReaderError::Invalid("owned root mode"));
    }
    Ok((file, stat.st_dev, stat.st_ino))
}
fn regular(file: &File) -> Result<(), ReaderError> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != 0o600
    {
        return Err(ReaderError::Invalid("journal file identity"));
    }
    Ok(())
}
struct Buffer {
    bytes: [u8; RECORD],
    len: usize,
}
impl Buffer {
    fn new(sequence: usize, tag: u8) -> Self {
        let mut buffer = Self {
            bytes: [0; RECORD],
            len: 4,
        };
        buffer.put(&[1, sequence as u8, tag]).expect("fixed prefix");
        buffer
    }
    fn put(&mut self, bytes: &[u8]) -> Result<(), ReaderError> {
        if bytes.len() > RECORD - self.len {
            return Err(ReaderError::Invalid("record ceiling"));
        }
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
    fn number(&mut self, number: u64) -> Result<(), ReaderError> {
        self.put(&number.to_le_bytes())
    }
    fn text(&mut self, text: &str) -> Result<(), ReaderError> {
        self.put(&(text.len() as u16).to_le_bytes())?;
        self.put(text.as_bytes())
    }
    fn finish(&mut self) {
        self.bytes[..4].copy_from_slice(&((self.len - 4) as u32).to_le_bytes());
    }
}
impl Journal {
    pub(super) fn create(path: &Path, inputs: &ReaderInputs) -> Result<Self, ReaderError> {
        let (root, _, _) = root(path)?;
        Self::create_root(root, inputs)
    }
    pub(super) fn create_descriptor(
        root: File,
        inputs: &ReaderInputs,
    ) -> Result<Self, ReaderError> {
        crate::project::validate_directory(&root, true)
            .map_err(|_| ReaderError::Invalid("owned reader storage"))?;
        Self::create_root(root, inputs)
    }
    fn create_root(root: File, inputs: &ReaderInputs) -> Result<Self, ReaderError> {
        let stat = fstat(&root)?;
        let (device, inode) = (stat.st_dev, stat.st_ino);
        let mut header = Buffer::new(0, 0);
        header.number(device)?;
        header.number(inode)?;
        header.text(&inputs.session)?;
        header.number(inputs.generation)?;
        header.text(READER_INSTRUCTION)?;
        for content in &inputs.contents {
            header.text(content)?;
        }
        header.finish();
        let file = File::from(openat(
            &root,
            NAME,
            OFlags::CREATE
                | OFlags::EXCL
                | OFlags::WRONLY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        regular(&file)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        root.sync_all()?;
        let mut journal = Self {
            file,
            _root: root,
            records: 0,
            bytes: 0,
            poisoned: false,
            #[cfg(test)]
            accept_fault: None,
            #[cfg(test)]
            accept_gate: None,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            limits: (RECORDS, TOTAL),
            #[cfg(test)]
            gate: None,
        };
        journal.write(&header)?;
        journal._root.sync_all()?;
        Ok(journal)
    }
    fn write(&mut self, buffer: &Buffer) -> Result<(), ReaderError> {
        if self.poisoned {
            return Err(ReaderError::Storage);
        }
        let limits = (RECORDS, TOTAL);
        #[cfg(test)]
        let limits = {
            let _ = limits;
            self.limits
        };
        if self.records >= limits.0 || self.bytes > limits.1 || buffer.len > limits.1 - self.bytes {
            return Err(ReaderError::Invalid("journal capacity"));
        }
        let result = (|| {
            #[cfg(test)]
            if let Some((reached, release)) = self.gate.take() {
                reached.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            #[cfg(test)]
            match self.fault.take() {
                Some(Fault::Partial) => {
                    self.file.write_all(&buffer.bytes[..buffer.len / 2])?;
                    return Err(io::Error::other("partial append"));
                }
                Some(Fault::Append) => return Err(io::Error::other("append failure")),
                Some(Fault::Sync) => {
                    self.file.write_all(&buffer.bytes[..buffer.len])?;
                    return Err(io::Error::other("sync failure"));
                }
                None => {}
            }
            self.file.write_all(&buffer.bytes[..buffer.len])?;
            self.file.sync_all()
        })();
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error.into());
        }
        self.records += 1;
        self.bytes += buffer.len;
        Ok(())
    }
    pub(super) fn append(&mut self, record: &Record) -> Result<(), ReaderError> {
        let mut b = Buffer::new(
            self.records,
            match record {
                Record::Started(_) => 1,
                Record::Terminal { .. } => 2,
                Record::End(_) => 3,
                Record::Accept { .. } => 4,
            },
        );
        match record {
            Record::Started(child) => b.put(&[*child])?,
            Record::Terminal {
                child,
                id,
                state,
                response,
            } => {
                b.put(&[*child])?;
                b.number(id.map_or(0, |id| id.0))?;
                match state {
                    ReaderState::Completed { summary } => {
                        b.put(&[0])?;
                        b.text(response)?;
                        b.text(summary)?;
                    }
                    ReaderState::Cancelled => b.put(&[1])?,
                    ReaderState::Failed(failure) => b.put(&[
                        2,
                        match failure {
                            ReaderFailure::Admission => 0,
                            ReaderFailure::Transport => 1,
                            ReaderFailure::InvalidResponse => 2,
                            ReaderFailure::ResponseLimit => 3,
                            ReaderFailure::Storage => 4,
                        },
                    ])?,
                    _ => return Err(ReaderError::Invalid("terminal state")),
                }
            }
            Record::End(mask) => b.put(&[*mask])?,
            Record::Accept { child, cancelled } => b.put(&[*child, u8::from(*cancelled)])?,
        }
        b.finish();
        #[cfg(test)]
        if matches!(record, Record::Accept { .. }) {
            self.fault = self.accept_fault.take();
            self.gate = self.accept_gate.take();
        }
        self.write(&b)
    }
}
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], ReaderError> {
        if len > self.bytes.len() - self.pos {
            return Err(ReaderError::Invalid("truncated record"));
        }
        let bytes = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8, ReaderError> {
        Ok(self.take(1)?[0])
    }
    fn number(&mut self) -> Result<u64, ReaderError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn text(&mut self, limit: usize) -> Result<&'a str, ReaderError> {
        let len = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
        if len > limit {
            return Err(ReaderError::Invalid("stored text ceiling"));
        }
        std::str::from_utf8(self.take(len)?).map_err(|_| ReaderError::Invalid("stored UTF-8"))
    }
}
pub(super) fn reconstruct(
    path: &Path,
    session: &str,
    generation: u64,
) -> Result<ReaderTree, ReaderError> {
    ReaderInputs::new(session, generation, [""; 3])?;
    let (root, _, _) = root(path)?;
    reconstruct_root(root, session, generation, None)
}
pub(crate) fn reconstruct_descriptor(
    root: File,
    inputs: &ReaderInputs,
) -> Result<ReaderTree, ReaderError> {
    crate::project::validate_directory(&root, true)
        .map_err(|_| ReaderError::Invalid("owned reader storage"))?;
    reconstruct_root(
        root,
        &inputs.session,
        inputs.generation,
        Some(inputs.contents.each_ref().map(|s| s.as_ref())),
    )
}
fn reconstruct_root(
    root: File,
    session: &str,
    generation: u64,
    expected: Option<[&str; 3]>,
) -> Result<ReaderTree, ReaderError> {
    let stat = fstat(&root)?;
    let (device, inode) = (stat.st_dev, stat.st_ino);
    let mut file = File::from(openat(
        &root,
        NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    regular(&file)?;
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    let size = fstat(&file)?.st_size;
    if size < 0 || size as u64 > TOTAL as u64 {
        return Err(ReaderError::Invalid("journal size"));
    }
    let mut children = std::array::from_fn(|child| ReaderFact {
        child: child as u8,
        request_id: None,
        state: ReaderState::NotDispatched,
    });
    let mut finished = false;
    let mut accepted = [false; 3];
    let mut terminal_ids = [None; 3];
    let mut provisional: [Option<(Option<RequestId>, ReaderState)>; 3] =
        std::array::from_fn(|_| None);
    let mut sequence = 0;
    let mut total = 0;
    let mut buffer = [0; RECORD];
    loop {
        let count = file.read(&mut buffer[..1])?;
        if count == 0 {
            break;
        }
        if sequence >= RECORDS || finished {
            return Err(ReaderError::Invalid("record count or after end"));
        }
        file.read_exact(&mut buffer[1..4])?;
        let len = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
        if !(3..=RECORD - 4).contains(&len) || total + 4 + len > TOTAL {
            return Err(ReaderError::Invalid("record framing ceiling"));
        }
        file.read_exact(&mut buffer[4..4 + len])?;
        total += 4 + len;
        let mut c = Cursor {
            bytes: &buffer[4..4 + len],
            pos: 0,
        };
        if c.byte()? != 1 || c.byte()? as usize != sequence {
            return Err(ReaderError::Invalid("version or sequence"));
        }
        let tag = c.byte()?;
        match tag {
            0 if sequence == 0 => {
                if c.number()? != device
                    || c.number()? != inode
                    || c.text(64)? != session
                    || c.number()? != generation
                    || c.text(256)? != READER_INSTRUCTION
                {
                    return Err(ReaderError::Invalid("header identity"));
                }
                for index in 0..3 {
                    let content = c.text(CONTENT)?;
                    validate_text(content, CONTENT, true)?;
                    if expected
                        .as_ref()
                        .is_some_and(|inputs| inputs[index] != content)
                    {
                        return Err(ReaderError::Invalid(
                            "complete captured reader input mismatch",
                        ));
                    }
                }
            }
            1 if sequence > 0 => {
                let child = c.byte()? as usize;
                let fact = children
                    .get_mut(child)
                    .ok_or(ReaderError::Invalid("child membership"))?;
                if fact.state != ReaderState::NotDispatched {
                    return Err(ReaderError::Invalid("duplicate start"));
                }
                fact.state = ReaderState::Interrupted;
            }
            2 if sequence > 0 => {
                let child = c.byte()?;
                let fact = children
                    .get_mut(child as usize)
                    .ok_or(ReaderError::Invalid("child membership"))?;
                if fact.state != ReaderState::Interrupted {
                    return Err(ReaderError::Invalid("terminal transition"));
                }
                let request = c.number()?;
                let id = (request != 0).then_some(RequestId(request));
                if id.is_some() && terminal_ids.contains(&id) {
                    return Err(ReaderError::Invalid("duplicate terminal request identity"));
                }
                terminal_ids[child as usize] = id;
                let state = match c.byte()? {
                    0 => {
                        if id.is_none() {
                            return Err(ReaderError::Invalid("completed without attempt"));
                        }
                        let response = c.text(RESPONSE)?;
                        let summary = c.text(SUMMARY)?;
                        if decode(response, session, generation, child)? != summary {
                            return Err(ReaderError::Invalid("summary equivalence"));
                        }
                        ReaderState::Completed {
                            summary: summary.to_owned(),
                        }
                    }
                    1 => ReaderState::Cancelled,
                    2 => ReaderState::Failed(match c.byte()? {
                        0 => ReaderFailure::Admission,
                        1 => ReaderFailure::Transport,
                        2 => ReaderFailure::InvalidResponse,
                        3 => ReaderFailure::ResponseLimit,
                        _ => return Err(ReaderError::Invalid("failure code")),
                    }),
                    _ => return Err(ReaderError::Invalid("terminal code")),
                };
                let attempt_valid = match &state {
                    ReaderState::Completed { .. }
                    | ReaderState::Failed(
                        ReaderFailure::Transport
                        | ReaderFailure::InvalidResponse
                        | ReaderFailure::ResponseLimit,
                    ) => id.is_some(),
                    ReaderState::Failed(ReaderFailure::Admission) => id.is_none(),
                    ReaderState::Cancelled => true,
                    _ => false,
                };
                if !attempt_valid {
                    return Err(ReaderError::Invalid("terminal attempt compatibility"));
                }
                if provisional[child as usize].is_some() {
                    return Err(ReaderError::Invalid("duplicate terminal"));
                }
                provisional[child as usize] = Some((id, state));
            }
            4 if sequence > 0 => {
                let child = c.byte()? as usize;
                let fact = children
                    .get_mut(child)
                    .ok_or(ReaderError::Invalid("cancel child"))?;
                if accepted[child] {
                    return Err(ReaderError::Invalid("duplicate acceptance"));
                }
                let (id, state) = provisional[child]
                    .take()
                    .ok_or(ReaderError::Invalid("acceptance without terminal"))?;
                fact.request_id = id;
                fact.state = match c.byte()? {
                    0 if state != ReaderState::Cancelled => state,
                    1 => ReaderState::Cancelled,
                    _ => return Err(ReaderError::Invalid("acceptance disposition")),
                };
                accepted[child] = true;
            }
            3 if sequence > 0 => {
                if children.iter().any(|f| f.state == ReaderState::Interrupted) {
                    return Err(ReaderError::Invalid("unfinished child at end"));
                }
                let mask = c.byte()?;
                let expected = children.iter().fold(0, |mask, fact| {
                    mask | if fact.state == ReaderState::Cancelled {
                        1 << fact.child
                    } else {
                        0
                    }
                });
                if mask != expected {
                    return Err(ReaderError::Invalid("contradictory end disposition"));
                }
                finished = true;
            }
            _ => return Err(ReaderError::Invalid("record tag or transition")),
        }
        if c.pos != c.bytes.len() {
            return Err(ReaderError::Invalid("trailing record data"));
        }
        sequence += 1;
    }
    if sequence == 0 {
        return Err(ReaderError::Invalid("missing header"));
    }
    for i in 0..3 {
        for j in 0..i {
            if children[i].request_id.is_some() && children[i].request_id == children[j].request_id
            {
                return Err(ReaderError::Invalid("duplicate request identity"));
            }
        }
    }
    Ok(ReaderTree {
        session: session.to_owned(),
        generation,
        children,
        parent_finished: finished,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::tests::support;
    #[test]
    fn terminal_attempt_matrix_is_strict_before_acceptance_override() {
        let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
        for code in 0..=5 {
            for request in [0u64, 7] {
                for acceptance in [None, Some(0), Some(1)] {
                    let root = support::OwnedRoot::new();
                    let mut journal = Journal::create(
                        &root.path,
                        &ReaderInputs::new("matrix", 1, [""; 3]).unwrap(),
                    )
                    .unwrap();
                    journal.append(&Record::Started(0)).unwrap();
                    drop(journal);
                    let path = root.path.join(NAME);
                    let mut bytes = std::fs::read(&path).unwrap();
                    let mut terminal = Buffer::new(2, 2);
                    terminal.put(&[0]).unwrap();
                    terminal.number(request).unwrap();
                    if code == 5 {
                        terminal.put(&[0]).unwrap();
                        terminal
                            .text(
                                r#"{"session":"matrix","generation":1,"child":0,"summary":"done"}"#,
                            )
                            .unwrap();
                        terminal.text("done").unwrap();
                    } else {
                        terminal.put(&[2, code]).unwrap();
                    }
                    terminal.finish();
                    bytes.extend_from_slice(&terminal.bytes[..terminal.len]);
                    if let Some(cancelled) = acceptance {
                        let mut accept = Buffer::new(3, 4);
                        accept.put(&[0, cancelled]).unwrap();
                        accept.finish();
                        bytes.extend_from_slice(&accept.bytes[..accept.len]);
                    }
                    std::fs::write(&path, &bytes).unwrap();
                    let inode = fstat(File::open(&path).unwrap()).unwrap().st_ino;
                    let result = reconstruct(&root.path, "matrix", 1);
                    let valid = match code {
                        0 => request == 0,
                        1..=3 | 5 => request != 0,
                        _ => false,
                    };
                    assert_eq!(
                        result.is_ok(),
                        valid,
                        "code={code} id={request} accept={acceptance:?}"
                    );
                    if let Ok(tree) = result {
                        assert_eq!(
                            tree.children[0].request_id,
                            acceptance.and((request != 0).then_some(RequestId(request)))
                        );
                        assert_eq!(
                            tree.children[0].state,
                            match acceptance {
                                None => ReaderState::Interrupted,
                                Some(1) => ReaderState::Cancelled,
                                Some(0) if code == 5 => ReaderState::Completed {
                                    summary: "done".to_owned()
                                },
                                _ => ReaderState::Failed(match code {
                                    0 => ReaderFailure::Admission,
                                    1 => ReaderFailure::Transport,
                                    2 => ReaderFailure::InvalidResponse,
                                    3 => ReaderFailure::ResponseLimit,
                                    _ => unreachable!(),
                                }),
                            }
                        );
                    }
                    assert_eq!(std::fs::read(&path).unwrap(), bytes);
                    assert_eq!(fstat(File::open(&path).unwrap()).unwrap().st_ino, inode);
                }
            }
        }
        for request in [None, Some(RequestId(7))] {
            let root = support::OwnedRoot::new();
            let mut journal = Journal::create(
                &root.path,
                &ReaderInputs::new("cancelled", 1, [""; 3]).unwrap(),
            )
            .unwrap();
            journal.append(&Record::Started(0)).unwrap();
            journal
                .append(&Record::Terminal {
                    child: 0,
                    id: request,
                    state: ReaderState::Cancelled,
                    response: String::new(),
                })
                .unwrap();
            journal
                .append(&Record::Accept {
                    child: 0,
                    cancelled: true,
                })
                .unwrap();
            drop(journal);
            let tree = reconstruct(&root.path, "cancelled", 1).unwrap();
            assert_eq!(tree.children[0].request_id, request);
            assert_eq!(tree.children[0].state, ReaderState::Cancelled);
        }
    }
    #[test]
    fn duplicate_provisional_terminal_ids_fail_closed() {
        let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
        let root = support::OwnedRoot::new();
        let mut journal = Journal::create(
            &root.path,
            &ReaderInputs::new("duplicate", 1, [""; 3]).unwrap(),
        )
        .unwrap();
        for child in 0..2 {
            journal.append(&Record::Started(child)).unwrap();
            journal
                .append(&Record::Terminal {
                    child,
                    id: Some(RequestId(5)),
                    state: ReaderState::Failed(ReaderFailure::Transport),
                    response: String::new(),
                })
                .unwrap();
        }
        drop(journal);
        assert!(reconstruct(&root.path, "duplicate", 1).is_err());
    }
    #[test]
    fn provisional_terminal_and_contradictory_end_are_not_accepted_facts() {
        let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
        let root = support::OwnedRoot::new();
        let mut journal = Journal::create(
            &root.path,
            &ReaderInputs::new("accept", 1, [""; 3]).unwrap(),
        )
        .unwrap();
        journal.append(&Record::Started(0)).unwrap();
        journal
            .append(&Record::Terminal {
                child: 0,
                id: Some(RequestId(5)),
                state: ReaderState::Completed {
                    summary: "done".to_owned(),
                },
                response: r#"{"session":"accept","generation":1,"child":0,"summary":"done"}"#
                    .to_owned(),
            })
            .unwrap();
        drop(journal);
        assert_eq!(
            reconstruct(&root.path, "accept", 1).unwrap().children[0].state,
            ReaderState::Interrupted
        );
        let path = root.path.join(NAME);
        let prefix = std::fs::read(&path).unwrap();
        let mut accept = Buffer::new(3, 4);
        accept.put(&[0, 0]).unwrap();
        accept.finish();
        let mut end = Buffer::new(4, 3);
        end.put(&[1]).unwrap();
        end.finish();
        let bytes = [
            prefix,
            accept.bytes[..accept.len].to_vec(),
            end.bytes[..end.len].to_vec(),
        ]
        .concat();
        std::fs::write(&path, &bytes).unwrap();
        assert!(reconstruct(&root.path, "accept", 1).is_err());
        let mut corrected = bytes.clone();
        *corrected.last_mut().unwrap() = 0;
        std::fs::write(&path, corrected).unwrap();
        assert!(matches!(
            reconstruct(&root.path, "accept", 1).unwrap().children[0].state,
            ReaderState::Completed { .. }
        ));
    }
    #[test]
    fn blocked_handoff_and_poison_join_owned_worker_without_success() {
        let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
        for fault in [Fault::Partial, Fault::Append, Fault::Sync] {
            let root = support::OwnedRoot::new();
            let mut journal = Journal::create(
                &root.path,
                &ReaderInputs::new("poison", 1, [""; 3]).unwrap(),
            )
            .unwrap();
            journal.append(&Record::Started(0)).unwrap();
            journal.fault = Some(fault);
            let control = Arc::new(Control::new("poison", 1));
            let (sender, receiver) = mpsc::channel(1);
            let (reply, receive) = oneshot::channel();
            sender
                .try_send(Work {
                    record: Record::Terminal {
                        child: 0,
                        id: Some(RequestId(5)),
                        state: ReaderState::Completed {
                            summary: "done".to_owned(),
                        },
                        response:
                            r#"{"session":"poison","generation":1,"child":0,"summary":"done"}"#
                                .to_owned(),
                    },
                    reply,
                })
                .unwrap();
            drop(sender);
            journal_worker(journal, receiver, control.clone());
            assert!(receive.blocking_recv().unwrap().is_err());
            assert!(control.state.lock().unwrap().cancelled.iter().all(|c| *c));
            assert!(!control.state.lock().unwrap().accepted[0]);
        }
        let root = support::OwnedRoot::new();
        let mut journal =
            Journal::create(&root.path, &ReaderInputs::new("queue", 1, [""; 3]).unwrap()).unwrap();
        let (reached, waiting) = std::sync::mpsc::sync_channel(1);
        let (release, blocked) = std::sync::mpsc::sync_channel(1);
        journal.gate = Some((reached, blocked));
        let control = Arc::new(Control::new("queue", 1));
        let (sender, receiver) = mpsc::channel(1);
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
        let (reply, receive) = oneshot::channel();
        sender
            .try_send(Work {
                record: Record::Started(0),
                reply,
            })
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let (reply, second) = oneshot::channel();
        sender
            .try_send(Work {
                record: Record::Started(1),
                reply,
            })
            .unwrap();
        let (reply, _) = oneshot::channel();
        assert!(
            sender
                .try_send(Work {
                    record: Record::Started(2),
                    reply
                })
                .is_err()
        );
        cancel(&control, None);
        assert!(control.state.lock().unwrap().cancelled.iter().all(|c| *c));
        release.send(()).unwrap();
        receive.blocking_recv().unwrap().unwrap();
        second.blocking_recv().unwrap().unwrap();
        drop(sender);
        worker.join().unwrap();
        assert_eq!(
            reconstruct(&root.path, "queue", 1).unwrap().children[0].state,
            ReaderState::Interrupted
        );
    }
    #[test]
    fn faults_capacity_prefix_and_corruption_fail_closed() {
        let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
        for fault in [Fault::Partial, Fault::Append, Fault::Sync] {
            let root = support::OwnedRoot::new();
            let inputs = ReaderInputs::new("fault", 1, [""; 3]).unwrap();
            let mut journal = Journal::create(&root.path, &inputs).unwrap();
            journal.fault = Some(fault);
            assert!(journal.append(&Record::Started(0)).is_err());
            assert!(journal.poisoned);
            assert!(journal.append(&Record::Started(1)).is_err());
            drop(journal);
        }
        for limits in [(1, TOTAL), (8, 1)] {
            let root = support::OwnedRoot::new();
            let mut journal =
                Journal::create(&root.path, &ReaderInputs::new("quota", 1, [""; 3]).unwrap())
                    .unwrap();
            journal.limits = limits;
            assert!(journal.append(&Record::Started(0)).is_err());
            drop(journal);
            assert_eq!(
                reconstruct(&root.path, "quota", 1).unwrap().children[0].state,
                ReaderState::NotDispatched
            );
        }
        let root = support::OwnedRoot::new();
        let mut journal = Journal::create(
            &root.path,
            &ReaderInputs::new("prefix", 1, [""; 3]).unwrap(),
        )
        .unwrap();
        journal.append(&Record::Started(0)).unwrap();
        journal
            .append(&Record::Terminal {
                child: 0,
                id: Some(RequestId(1)),
                state: ReaderState::Completed {
                    summary: "done".to_owned(),
                },
                response: r#"{"session":"prefix","generation":1,"child":0,"summary":"done"}"#
                    .to_owned(),
            })
            .unwrap();
        journal
            .append(&Record::Accept {
                child: 0,
                cancelled: false,
            })
            .unwrap();
        journal.append(&Record::Started(1)).unwrap();
        drop(journal);
        let tree = reconstruct(&root.path, "prefix", 1).unwrap();
        assert!(matches!(
            tree.children[0].state,
            ReaderState::Completed { .. }
        ));
        assert_eq!(tree.children[1].state, ReaderState::Interrupted);
        assert_eq!(tree.children[2].state, ReaderState::NotDispatched);
        let path = root.path.join(NAME);
        let original = std::fs::read(&path).unwrap();
        for bytes in [
            original[..original.len() - 1].to_vec(),
            [original.clone(), vec![0]].concat(),
            {
                let mut b = original.clone();
                b[4] = 9;
                b
            },
            {
                let mut b = original.clone();
                b[..4].copy_from_slice(&4096u32.to_le_bytes());
                b
            },
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(reconstruct(&root.path, "prefix", 1).is_err());
        }
        std::fs::write(path, original).unwrap();
        assert!(reconstruct(&root.path, "prefix", 2).is_err());
    }
}

#[cfg(test)]
mod restart_probe {
    use super::*;
    #[test]
    #[ignore = "invoked in isolated fresh processes by the restart evidence recipe"]
    fn acknowledged_child_exit_and_read_only_facts() {
        let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
        let Ok(path) = std::env::var("SCRAMJET_READER_RESTART_ROOT") else {
            return;
        };
        let path = Path::new(&path);
        if std::env::var("SCRAMJET_READER_RESTART_MODE").as_deref() == Ok("write") {
            let inputs = ReaderInputs::new("restart", 1, ["a", "b", "c"]).unwrap();
            let mut journal = Journal::create(path, &inputs).unwrap();
            journal.append(&Record::Started(0)).unwrap();
            journal
                .append(&Record::Terminal {
                    child: 0,
                    id: Some(RequestId(42)),
                    state: ReaderState::Completed {
                        summary: "done".to_owned(),
                    },
                    response: r#"{"session":"restart","generation":1,"child":0,"summary":"done"}"#
                        .to_owned(),
                })
                .unwrap();
            journal
                .append(&Record::Accept {
                    child: 0,
                    cancelled: false,
                })
                .unwrap();
            journal.append(&Record::Started(1)).unwrap();
            std::process::exit(17);
        }
        let before = std::fs::read(path.join(NAME)).unwrap();
        let inode = fstat(File::open(path.join(NAME)).unwrap()).unwrap().st_ino;
        let tree = reconstruct(path, "restart", 1).unwrap();
        assert_eq!(
            tree.children[0].state,
            ReaderState::Completed {
                summary: if std::env::var("SCRAMJET_READER_RESTART_MODE").as_deref()
                    == Ok("actual-read")
                {
                    "reader-0"
                } else {
                    "done"
                }
                .to_owned()
            }
        );
        assert_eq!(tree.children[1].state, ReaderState::Interrupted);
        assert_eq!(tree.children[2].state, ReaderState::NotDispatched);
        assert!(!tree.parent_finished);
        assert_eq!(before, std::fs::read(path.join(NAME)).unwrap());
        assert_eq!(
            inode,
            fstat(File::open(path.join(NAME)).unwrap()).unwrap().st_ino
        );
    }
}
