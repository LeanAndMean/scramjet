use crate::{CodingCapability, CodingError, CodingHistory, CodingSession, RootIdentity};
use rustix::fs::{
    FileType, FlockOperation, Mode, OFlags, fchmod, flistxattr, flock, fstat, mkdirat, open, openat,
};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub(crate) mod context;
mod history;
pub(crate) mod journal;
mod session;
pub use history::{ProjectHistory, reconstruct_project_history};
pub use journal::{
    PROJECT_JOURNAL_BYTES, PROJECT_JOURNAL_RECORD_BYTES, PROJECT_JOURNAL_RECORDS, ProjectCaptured,
    ProjectClosure, ProjectCoding, ProjectHeader, ProjectRead, ProjectReaderFact, ProjectReaders,
    ProjectReadyRecord, ProjectRecord, ProjectReport, ProjectRequested,
};
pub use session::{ProjectFinish, ProjectSession, ProjectSettlement};

pub const PROJECT_LOCK_NAME: &str = ".scramjet-project-lock";
pub const PROJECT_REQUEST_BYTES: usize = 1024;

struct RequestBuffer(Vec<u8>);
impl std::io::Write for RequestBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > PROJECT_REQUEST_BYTES - self.0.len() {
            return Err(std::io::Error::other(
                "project request serialization ceiling",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProjectRequest {
    pub(crate) operation: String,
    pub(crate) target: String,
    pub(crate) supporting: [String; 2],
    pub(crate) needle: String,
    pub(crate) replacement: String,
}

impl<'de> serde::Deserialize<'de> for ProjectRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> serde::de::Visitor<'de> for Object {
            type Value = ProjectRequest;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a strict replace_once request object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                use crate::coding::journal::BoundedString;
                use serde::de::Error;
                let (mut operation, mut target, mut supporting, mut needle, mut replacement) =
                    (None, None, None, None, None);
                while let Some(key) = map.next_key::<BoundedString<16>>()? {
                    macro_rules! field {
                        ($slot:ident, $ty:ty) => {{
                            if $slot.is_some() {
                                return Err(M::Error::custom("duplicate request field"));
                            }
                            $slot = Some(map.next_value::<$ty>()?);
                        }};
                    }
                    match key.0.as_str() {
                        "operation" => field!(operation, BoundedString<16>),
                        "target" => field!(target, BoundedString<64>),
                        "supporting" => field!(supporting, [BoundedString<64>; 2]),
                        "needle" => field!(needle, BoundedString<256>),
                        "replacement" => field!(replacement, BoundedString<256>),
                        _ => return Err(M::Error::custom("unknown request field")),
                    }
                }
                let request = ProjectRequest {
                    operation: operation
                        .ok_or_else(|| M::Error::missing_field("operation"))?
                        .0,
                    target: target.ok_or_else(|| M::Error::missing_field("target"))?.0,
                    supporting: supporting
                        .ok_or_else(|| M::Error::missing_field("supporting"))?
                        .map(|s| s.0),
                    needle: needle.ok_or_else(|| M::Error::missing_field("needle"))?.0,
                    replacement: replacement
                        .ok_or_else(|| M::Error::missing_field("replacement"))?
                        .0,
                };
                request.validate().map_err(M::Error::custom)?;
                Ok(request)
            }
        }
        deserializer.deserialize_map(Object)
    }
}
impl ProjectRequest {
    pub fn parse(json: &str) -> Result<Self, CodingError> {
        if json.len() > PROJECT_REQUEST_BYTES {
            return Err(CodingError::Invalid("project request byte ceiling"));
        }
        serde_json::from_str(json)
            .map_err(|_| CodingError::Invalid("invalid strict replace_once request"))
    }
    fn validate(&self) -> Result<(), CodingError> {
        if self.operation != "replace_once" {
            return Err(CodingError::Invalid("only replace_once is supported"));
        }
        let names = self.names();
        for name in names {
            validate_project_name(name)?;
        }
        if names[0] == names[1] || names[0] == names[2] || names[1] == names[2] {
            return Err(CodingError::Invalid(
                "three distinct input basenames required",
            ));
        }
        for text in [&self.needle, &self.replacement] {
            validate_content(text)?;
            if text.is_empty() || text.len() > 256 {
                return Err(CodingError::Invalid(
                    "nonempty fragments at most 256 bytes required",
                ));
            }
        }
        if self.needle == self.replacement {
            return Err(CodingError::Invalid("no-op request"));
        }
        let mut encoded = RequestBuffer(Vec::with_capacity(PROJECT_REQUEST_BYTES));
        serde_json::to_writer(&mut encoded, self)
            .map_err(|_| CodingError::Invalid("project request serialization ceiling"))?;
        Ok(())
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn supporting(&self) -> [&str; 2] {
        self.supporting.each_ref().map(String::as_str)
    }
    pub fn needle(&self) -> &str {
        &self.needle
    }
    pub fn replacement(&self) -> &str {
        &self.replacement
    }
    fn names(&self) -> [&str; 3] {
        [&self.target, &self.supporting[0], &self.supporting[1]]
    }
    fn derive(&self, text: &str) -> Result<String, CodingError> {
        let mut occurrences = text.match_indices(&self.needle);
        let (at, _) = occurrences
            .next()
            .ok_or(CodingError::Invalid("needle is absent"))?;
        // Check overlapping occurrences too; match_indices alone skips them.
        if text[at + text[at..].chars().next().unwrap().len_utf8()..].contains(&self.needle) {
            return Err(CodingError::Invalid("needle is not unique"));
        }
        let length = text.len() - self.needle.len() + self.replacement.len();
        if length > 1024 {
            return Err(CodingError::Invalid("derived target byte ceiling"));
        }
        let mut result = String::with_capacity(length);
        result.push_str(&text[..at]);
        result.push_str(&self.replacement);
        result.push_str(&text[at + self.needle.len()..]);
        Ok(result)
    }
}

pub(crate) fn validate_project_name(name: &str) -> Result<(), CodingError> {
    crate::coding::validate_name(name)?;
    if name == PROJECT_LOCK_NAME || name.starts_with(".scramjet-stage-") {
        return Err(CodingError::Invalid("reserved project basename"));
    }
    Ok(())
}
pub(crate) fn validate_content(text: &str) -> Result<(), CodingError> {
    if text.len() > 1024
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(CodingError::Invalid("unsupported project content"));
    }
    Ok(())
}
fn identity(file: &File) -> Result<RootIdentity, CodingError> {
    let stat = fstat(file)?;
    Ok(RootIdentity {
        device: stat.st_dev,
        inode: stat.st_ino,
    })
}
pub(crate) fn open_directory(path: &Path) -> Result<File, CodingError> {
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(CodingError::Invalid(
            "bounded absolute directory path required",
        ));
    }
    let mut directory = File::from(open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = File::from(openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?);
            }
            _ => return Err(CodingError::Invalid("non-normal directory component")),
        }
    }
    Ok(directory)
}
pub(crate) fn validate_directory(file: &File, storage: bool) -> Result<(), CodingError> {
    let stat = fstat(file)?;
    if stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7022 != 0
        || (storage && stat.st_mode & 0o7777 != 0o700)
    {
        return Err(CodingError::Invalid(
            "unsupported owned directory permissions",
        ));
    }
    no_attributes(file)
}
fn contains(ancestor: RootIdentity, descendant: &File) -> Result<bool, CodingError> {
    let mut current = descendant.try_clone()?;
    // A bounded absolute path can contain at most 2048 one-byte components.
    for _ in 0..=2048 {
        let id = identity(&current)?;
        if id == ancestor {
            return Ok(true);
        }
        let parent = File::from(openat(
            &current,
            "..",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        if identity(&parent)? == id {
            return Ok(false);
        }
        current = parent;
    }
    Err(CodingError::Invalid("directory ancestry work ceiling"))
}
fn no_attributes(file: &File) -> Result<(), CodingError> {
    if flistxattr(file, &mut [0u8; 1][..])? != 0 {
        return Err(CodingError::Invalid(
            "extended attributes/ACLs are unsupported",
        ));
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProjectFile {
    pub name: String,
    pub identity: RootIdentity,
    pub mode: u32,
    pub content: String,
}
pub(crate) fn read_file(root: &File, name: &str) -> Result<ProjectFile, CodingError> {
    validate_project_name(name)?;
    read_named_file(root, name)
}

fn read_named_file(root: &File, name: &str) -> Result<ProjectFile, CodingError> {
    let mut file = File::from(openat(
        root,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let stat = fstat(&file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || !matches!(stat.st_mode & 0o7777, 0o600 | 0o644)
        || stat.st_size < 0
        || stat.st_size > 1024
    {
        return Err(CodingError::Invalid(
            "unsupported project file metadata or size",
        ));
    }
    no_attributes(&file)?;
    let mut bytes = [0u8; 1025];
    let mut count = 0;
    loop {
        let n = file.read(&mut bytes[count..])?;
        if n == 0 {
            break;
        }
        count += n;
        if count > 1024 {
            return Err(CodingError::Invalid("project file byte ceiling"));
        }
    }
    let after = fstat(&file)?;
    if (
        after.st_dev,
        after.st_ino,
        after.st_mode,
        after.st_nlink,
        after.st_uid,
        after.st_size,
        after.st_mtime,
        after.st_mtime_nsec,
        after.st_ctime,
        after.st_ctime_nsec,
    ) != (
        stat.st_dev,
        stat.st_ino,
        stat.st_mode,
        stat.st_nlink,
        stat.st_uid,
        stat.st_size,
        stat.st_mtime,
        stat.st_mtime_nsec,
        stat.st_ctime,
        stat.st_ctime_nsec,
    ) {
        return Err(CodingError::TargetMismatch);
    }
    no_attributes(&file)?;
    let content = std::str::from_utf8(&bytes[..count])
        .map_err(|_| CodingError::Invalid("project file is not UTF-8"))?;
    validate_content(content)?;
    Ok(ProjectFile {
        name: name.into(),
        identity: identity(&file)?,
        mode: stat.st_mode & 0o7777,
        content: content.into(),
    })
}

pub(crate) struct Workspace {
    pub root: File,
    pub storage: File,
    pub identity: RootIdentity,
    path: PathBuf,
    lock: File,
}
impl Workspace {
    fn validate(&self) -> Result<(), CodingError> {
        validate_directory(&self.root, false)?;
        if identity(&open_directory(&self.path)?)? != self.identity {
            return Err(CodingError::Stale);
        }
        let entry = File::from(openat(
            &self.root,
            PROJECT_LOCK_NAME,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        validate_lock(&entry)?;
        if identity(&entry)? != identity(&self.lock)? {
            return Err(CodingError::Stale);
        }
        Ok(())
    }
}
pub(crate) fn check_stage(
    root: &File,
    name: &str,
    stage: &File,
    mode: u32,
    expected: &str,
) -> Result<(), CodingError> {
    let current = read_named_file(root, name)?;
    if current.identity != identity(stage)? || current.mode != mode || current.content != expected {
        return Err(CodingError::TargetMismatch);
    }
    Ok(())
}

pub(crate) fn validate_stage(file: &File, mode: u32) -> Result<(), CodingError> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7777 != mode
    {
        return Err(CodingError::Invalid("unsupported stage metadata"));
    }
    no_attributes(file)
}

fn validate_lock(file: &File) -> Result<(), CodingError> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7777 != 0o600
    {
        return Err(CodingError::Invalid("unsupported persistent project lock"));
    }
    no_attributes(file)
}

pub struct ProjectWorkspace(Arc<Workspace>);
pub struct ProjectCapture {
    pub(crate) workspace: Arc<Workspace>,
    request: ProjectRequest,
    files: [ProjectFile; 3],
    desired: String,
}
impl ProjectCapture {
    pub fn request(&self) -> &ProjectRequest {
        &self.request
    }
    pub fn files(&self) -> &[ProjectFile; 3] {
        &self.files
    }
    pub fn desired_target(&self) -> &str {
        &self.desired
    }
    pub fn root_identity(&self) -> RootIdentity {
        self.workspace.identity
    }
    pub fn recheck(&self) -> Result<(), CodingError> {
        self.check(false)
    }
    fn check(&self, edited: bool) -> Result<(), CodingError> {
        self.workspace.validate()?;
        for (index, captured) in self.files.iter().enumerate() {
            let current = read_file(&self.workspace.root, &captured.name)?;
            if index == 0 && edited {
                if current.mode != captured.mode || current.content != self.desired {
                    return Err(CodingError::TargetMismatch);
                }
            } else if current != *captured {
                return Err(CodingError::TargetMismatch);
            }
        }
        Ok(())
    }
    pub(crate) fn clone_for_coding(&self) -> Self {
        Self {
            workspace: self.workspace.clone(),
            request: self.request.clone(),
            files: self.files.clone(),
            desired: self.desired.clone(),
        }
    }
    pub(crate) fn check_coding(&self, edited: bool) -> Result<(), CodingError> {
        self.check(edited)
    }
    pub(crate) fn expected(&self) -> &str {
        &self.files[0].content
    }
    pub(crate) fn mode(&self) -> u32 {
        self.files[0].mode
    }
}
impl ProjectWorkspace {
    pub fn open(project: &Path, storage: &Path) -> Result<Self, CodingError> {
        let root = open_directory(project)?;
        let storage = open_directory(storage)?;
        validate_directory(&root, false)?;
        validate_directory(&storage, true)?;
        let root_id = identity(&root)?;
        if contains(root_id, &storage)? || contains(identity(&storage)?, &root)? {
            return Err(CodingError::Invalid(
                "project and storage must be disjoint directory trees",
            ));
        }
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let lock = match openat(&root, PROJECT_LOCK_NAME, flags, Mode::empty()) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => {
                let file = File::from(openat(
                    &root,
                    PROJECT_LOCK_NAME,
                    flags | OFlags::CREATE | OFlags::EXCL,
                    Mode::RUSR | Mode::WUSR,
                )?);
                file.sync_all()?;
                root.sync_all()?;
                file
            }
            Err(error) => return Err(error.into()),
        };
        validate_lock(&lock)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
        let workspace = Arc::new(Workspace {
            root,
            storage,
            identity: root_id,
            path: project.into(),
            lock,
        });
        workspace.validate()?;
        Ok(Self(workspace))
    }
    pub fn root_identity(&self) -> RootIdentity {
        self.0.identity
    }
    pub fn capture(&self, request: ProjectRequest) -> Result<ProjectCapture, CodingError> {
        request.validate()?;
        self.0.validate()?;
        let [target, first, second] = request.names();
        let files = [
            read_file(&self.0.root, target)?,
            read_file(&self.0.root, first)?,
            read_file(&self.0.root, second)?,
        ];
        let desired = request.derive(&files[0].content)?;
        let capture = ProjectCapture {
            workspace: self.0.clone(),
            request,
            files,
            desired,
        };
        capture.recheck()?;
        Ok(capture)
    }
    pub fn coding_session(
        &self,
        capture: ProjectCapture,
        session: &str,
        journey: u8,
    ) -> Result<CodingSession, CodingError> {
        if !Arc::ptr_eq(&capture.workspace, &self.0) || !matches!(journey, 1 | 2) {
            return Err(CodingError::Stale);
        }
        crate::coding::validate_session(session)?;
        capture.recheck()?;
        let name = format!("coding-{journey}");
        mkdirat(&self.0.storage, name.as_str(), Mode::RWXU)?;
        let storage = File::from(openat(
            &self.0.storage,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        fchmod(&storage, Mode::RWXU)?;
        self.0.storage.sync_all()?;
        CodingSession::create_selected(
            capture,
            &storage,
            session,
            CodingCapability::ControlledEdit,
            journey,
        )
    }
    pub fn coding_history(
        storage: &Path,
        journey: u8,
        session: &str,
        root: RootIdentity,
    ) -> Result<CodingHistory, CodingError> {
        if !matches!(journey, 1 | 2) {
            return Err(CodingError::Invalid("journey ceiling"));
        }
        let storage = open_directory(storage)?;
        validate_directory(&storage, true)?;
        let name = format!("coding-{journey}");
        let directory = File::from(openat(
            &storage,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        validate_directory(&directory, true)?;
        crate::coding::history::from_storage(&directory, session, root, true)
    }
}
