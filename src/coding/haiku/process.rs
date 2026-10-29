use super::{
    CodingError, EditCancellation, HAIKU_COMMAND, HAIKU_SCRIPT, HaikuCapture, HaikuProcessResult,
};
use crate::coding::process;
pub(super) const PROFILE: &str = process::LEGACY_PROFILE;
pub(super) fn check_host() -> Result<(), CodingError> {
    process::check_host()?;
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata("/bin/bash")?;
    if metadata.uid() != 0 || metadata.mode() & 0o6022 != 0 {
        return Err(CodingError::Invalid(
            "untrusted installed isolation runtime",
        ));
    }
    Ok(())
}
fn validate(captures: &[HaikuCapture; 2]) -> Result<(), CodingError> {
    check_host()?;
    if captures[0].path != "haiku.md"
        || captures[1].path != "print_haiku.py"
        || captures.iter().any(|c| c.content.len() > 512)
        || captures[1].content != HAIKU_SCRIPT
    {
        return Err(CodingError::Invalid("unsupported isolated capture"));
    }
    Ok(())
}
fn result(result: &process::ProcessResult) -> Result<HaikuProcessResult, CodingError> {
    Ok(HaikuProcessResult {
        exit_code: result.exit_code,
        signal: result.signal,
        stdout: result
            .stdout
            .text()
            .ok_or(CodingError::Invalid("isolated stdout is not UTF-8"))?
            .into(),
        stderr: result
            .stderr
            .text()
            .ok_or(CodingError::Invalid("isolated stderr is not UTF-8"))?
            .into(),
        stopped: result.stopped.clone(),
        launcher_reaped: result.launcher_reaped,
        namespace_dead: result.namespace_dead,
    })
}
pub(super) fn execute(
    captures: &[HaikuCapture; 2],
    cancellation: &EditCancellation,
    start: impl FnMut(u32, u32) -> Result<(), CodingError>,
    mut observed: impl FnMut(&HaikuProcessResult),
) -> Result<HaikuProcessResult, CodingError> {
    validate(captures)?;
    let output = process::legacy(
        captures,
        HAIKU_SCRIPT,
        HAIKU_COMMAND,
        cancellation,
        std::time::Duration::ZERO,
        start,
        |r| {
            if let Ok(r) = result(r) {
                observed(&r);
            }
        },
    )?;
    result(&output)
}
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(super) fn control_started(
    captures: &[HaikuCapture; 2],
    cancellation: &EditCancellation,
    script: &str,
    start: impl FnMut(u32, u32) -> Result<(), CodingError>,
) -> Result<HaikuProcessResult, CodingError> {
    validate(captures)?;
    result(&process::legacy(
        captures,
        script,
        HAIKU_COMMAND,
        cancellation,
        std::time::Duration::from_millis(100),
        start,
        |_| {},
    )?)
}
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(super) fn control(
    captures: &[HaikuCapture; 2],
    cancellation: &EditCancellation,
    script: &str,
) -> Result<HaikuProcessResult, CodingError> {
    control_started(captures, cancellation, script, |_, _| Ok(()))
}
