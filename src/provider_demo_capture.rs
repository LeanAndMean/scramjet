use rustix::fs::{Mode, OFlags, openat};
use scramjet_rs::{
    PROVIDER_CAPTURE_BYTES, PROVIDER_DIAGNOSTIC_BYTES, PROVIDER_MODEL, ProviderCapture,
    replay_provider_data,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsFd, IntoRawFd},
        unix::fs::PermissionsExt,
    },
    path::Path,
    process::{Command, Stdio},
};

pub(crate) struct Prepared {
    directory: File,
    parent: File,
    raw: File,
    report: File,
    provenance: File,
    candidate_sha256: String,
    route: &'static str,
}
#[cfg(test)]
impl Prepared {
    #[allow(dead_code)]
    pub(crate) fn fail_raw_for_test(&mut self, path: &str) {
        self.raw = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    version: u8,
    representation: String,
    route: String,
    model: String,
    candidate_sha256: String,
    data_sha256: String,
    request_id: u64,
    reservation: Option<usize>,
    received_bytes: usize,
    captured_bytes: usize,
    parser_fed_bytes: usize,
    body_eof: bool,
    overflow: bool,
    parser_accepted: bool,
    local_joined: bool,
}
fn private_directory(path: &Path) -> Result<File, String> {
    let directory: File = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "private capture directory refused")?
    .into();
    private(&directory, true)?;
    Ok(directory)
}
fn private(file: &File, directory: bool) -> Result<(), String> {
    let stat = rustix::fs::fstat(file).map_err(|_| "capture metadata unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "capture metadata unavailable")?;
    if stat.st_uid != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o777 != if directory { 0o700 } else { 0o600 }
        || (!directory && (!metadata.is_file() || stat.st_nlink != 1))
    {
        return Err("private capture permissions/ownership refused".into());
    }
    Ok(())
}
fn file(directory: &File, name: &str, create: bool) -> Result<File, String> {
    let flags = if create {
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL
    } else {
        OFlags::RDONLY
    };
    let file: File = openat(
        directory.as_fd(),
        name,
        flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| "exclusive private capture file refused")?
    .into();
    private(&file, false)?;
    Ok(file)
}
fn digest(mut file: File) -> Result<String, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "capture hash seek failed")?;
    let output = Command::new("sha256sum")
        .stdin(Stdio::from(file))
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "capture hash tool unavailable")?;
    let text = std::str::from_utf8(&output.stdout).map_err(|_| "capture hash failed")?;
    let hash = text
        .split_whitespace()
        .next()
        .ok_or("capture hash failed")?;
    if !output.status.success() || hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("capture hash failed".into());
    }
    Ok(hash.into())
}
fn digest_bytes(bytes: &[u8]) -> Result<String, String> {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "capture hash tool unavailable")?;
    let written = child.stdin.take().unwrap().write_all(bytes);
    let output = child
        .wait_with_output()
        .map_err(|_| "capture hash join failed")?;
    written.map_err(|_| "capture hash write failed")?;
    let text = std::str::from_utf8(&output.stdout).map_err(|_| "capture hash failed")?;
    let hash = text
        .split_whitespace()
        .next()
        .ok_or("capture hash failed")?;
    if !output.status.success() || hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("capture hash failed".into());
    }
    Ok(hash.into())
}
pub(crate) fn prepare(path: &Path, live: bool) -> Result<Prepared, String> {
    let candidate_sha256 = digest(
        File::open(std::env::current_exe().map_err(|_| "candidate identity unavailable")?)
            .map_err(|_| "candidate identity unavailable")?,
    )?;
    let parent_path = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent: File = rustix::fs::open(
        parent_path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "capture parent refused; no dispatch")?
    .into();
    let name = path
        .file_name()
        .ok_or("capture directory name refused; no dispatch")?;
    rustix::fs::mkdirat(&parent, name, Mode::RWXU)
        .map_err(|_| "new exclusive capture directory required; no dispatch")?;
    let directory: File = openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "private capture directory refused")?
    .into();
    private(&directory, true)?;
    let raw = file(&directory, "response.data", true)?;
    let report = file(&directory, "diagnostic.json", true)?;
    let provenance = file(&directory, "provenance.json", true)?;
    directory
        .sync_all()
        .map_err(|_| "capture setup sync failed; no dispatch")?;
    parent
        .sync_all()
        .map_err(|_| "capture parent sync failed; no dispatch")?;
    Ok(Prepared {
        directory,
        parent,
        raw,
        report,
        provenance,
        candidate_sha256,
        route: if live {
            "proxy-responses"
        } else {
            "fixture-responses"
        },
    })
}
unsafe extern "C" {
    fn close(fd: std::os::fd::RawFd) -> std::os::raw::c_int;
}
fn close_checked(file: File) -> Result<(), String> {
    let fd = file.into_raw_fd();
    // Never retry close: even an error may have released the descriptor.
    if unsafe { close(fd) } == 0 {
        Ok(())
    } else {
        Err("capture close failed; no retry".into())
    }
}
pub(crate) fn persist(
    mut prepared: Prepared,
    capture: ProviderCapture,
    report: Vec<u8>,
) -> Result<(), String> {
    if !capture.local_joined
        || capture.bytes.len() > PROVIDER_CAPTURE_BYTES
        || report.len() > PROVIDER_DIAGNOSTIC_BYTES
        || capture.parser_fed_bytes > capture.bytes.len()
        || (!capture.overflow && capture.received_bytes != capture.bytes.len())
        || (capture.overflow
            && (capture.received_bytes <= PROVIDER_CAPTURE_BYTES || capture.body_eof))
        || (capture.body_eof && capture.parser_fed_bytes != capture.received_bytes)
        || (capture.parser_accepted && !capture.body_eof)
    {
        return Err("capture not locally joined or artifact budget exceeded; no retry".into());
    }
    prepared
        .raw
        .write_all(&capture.bytes)
        .and_then(|()| prepared.raw.sync_all())
        .map_err(|_| "raw capture write/sync failed; no retry")?;
    let data_sha256 = digest_bytes(&capture.bytes)?;
    close_checked(prepared.raw)?;
    prepared
        .report
        .write_all(&report)
        .and_then(|()| prepared.report.sync_all())
        .map_err(|_| "diagnostic capture write/sync failed; no retry")?;
    close_checked(prepared.report)?;
    let provenance = Provenance {
        version: 1,
        representation: "client-visible-http-response-DATA".into(),
        route: prepared.route.into(),
        model: PROVIDER_MODEL.into(),
        candidate_sha256: prepared.candidate_sha256,
        data_sha256,
        request_id: capture.id.0,
        reservation: capture.reservation,
        received_bytes: capture.received_bytes,
        captured_bytes: capture.bytes.len(),
        parser_fed_bytes: capture.parser_fed_bytes,
        body_eof: capture.body_eof,
        overflow: capture.overflow,
        parser_accepted: capture.parser_accepted,
        local_joined: capture.local_joined,
    };
    let mut writer = super::ReportWriter(Vec::with_capacity(PROVIDER_DIAGNOSTIC_BYTES));
    serde_json::to_writer(&mut writer, &provenance)
        .map_err(|_| "provenance budget exceeded; no retry")?;
    prepared
        .provenance
        .write_all(&writer.0)
        .and_then(|()| prepared.provenance.sync_all())
        .map_err(|_| "provenance write/sync failed; no retry")?;
    close_checked(prepared.provenance)?;
    prepared
        .directory
        .sync_all()
        .map_err(|_| "capture directory sync failed; no retry")?;
    close_checked(prepared.directory)?;
    close_checked(prepared.parent)
}
fn read_bounded(mut file: File, max: usize) -> Result<Vec<u8>, String> {
    let length = usize::try_from(file.metadata().map_err(|_| "capture stat failed")?.len())
        .map_err(|_| "capture budget exceeded")?;
    if length > max {
        return Err("capture budget exceeded".into());
    }
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)
        .map_err(|_| "capture read failed")?;
    let mut extra = [0];
    if file.read(&mut extra).map_err(|_| "capture read failed")? != 0 {
        return Err("capture changed or budget exceeded".into());
    }
    Ok(bytes)
}
pub(crate) fn replay(path: &Path) -> Result<Vec<u8>, String> {
    let directory = private_directory(path)?;
    let raw = file(&directory, "response.data", false)?;
    let bytes = read_bounded(raw, PROVIDER_CAPTURE_BYTES)?;
    let data_sha256 = digest_bytes(&bytes)?;
    let provenance: Provenance = serde_json::from_slice(&read_bounded(
        file(&directory, "provenance.json", false)?,
        PROVIDER_DIAGNOSTIC_BYTES,
    )?)
    .map_err(|_| "capture provenance refused")?;
    let report: serde_json::Value = serde_json::from_slice(&read_bounded(
        file(&directory, "diagnostic.json", false)?,
        PROVIDER_DIAGNOSTIC_BYTES,
    )?)
    .map_err(|_| "capture diagnostic refused")?;
    if provenance.version != 1
        || provenance.representation != "client-visible-http-response-DATA"
        || !matches!(
            provenance.route.as_str(),
            "proxy-responses" | "fixture-responses"
        )
        || provenance.model != PROVIDER_MODEL
        || provenance.candidate_sha256.len() != 64
        || !provenance
            .candidate_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || provenance.data_sha256 != data_sha256
        || provenance.captured_bytes != bytes.len()
        || provenance.received_bytes < bytes.len()
        || provenance.parser_fed_bytes > provenance.captured_bytes
        || (provenance.overflow
            && (provenance.received_bytes <= PROVIDER_CAPTURE_BYTES || provenance.body_eof))
        || (provenance.body_eof && provenance.parser_fed_bytes != provenance.received_bytes)
        || (provenance.parser_accepted && (!provenance.body_eof || provenance.overflow))
        || (!provenance.overflow && provenance.received_bytes != bytes.len())
        || !provenance.local_joined
        || report["report"] != "provider-diagnostic"
        || report["version"] != 1
    {
        return Err("capture provenance inconsistent".into());
    }
    let replay = replay_provider_data(&bytes);
    if provenance.parser_accepted && !replay.parser_accepted {
        return Err("capture provenance inconsistent".into());
    }
    let full_stream =
        provenance.body_eof && !provenance.overflow && provenance.received_bytes == bytes.len();
    let report = serde_json::json!({"report":"provider-replay", "version":1, "full_stream":full_stream, "parser_accepted":replay.parser_accepted,
        "original_parser_accepted":provenance.parser_accepted, "outcome":format!("{:?}",replay.outcome), "failure":replay.failure,
        "received_bytes":provenance.received_bytes, "parser_fed_bytes":provenance.parser_fed_bytes, "captured_bytes":bytes.len(),
        "execution_authority":false, "settlement_authority":false});
    let mut writer = super::ReportWriter(Vec::with_capacity(PROVIDER_DIAGNOSTIC_BYTES));
    serde_json::to_writer(&mut writer, &report).map_err(|_| "replay report budget exceeded")?;
    Ok(writer.0)
}
