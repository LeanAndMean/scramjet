use super::CodingError;
use rustix::fs::{AtFlags, Mode, OFlags, fstat, openat, statat};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read};

pub(crate) const FILE_BYTES: usize = 512;
pub(crate) const RUN_ANCHOR: &str = ".coding-run";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedFile {
    pub path: String,
    pub identity: [u64; 2],
    pub content: String,
}

pub(crate) struct FileSlots<'a> {
    names: [&'a str; 2],
}

pub(crate) fn validate_basename(name: &str) -> Result<(), CodingError> {
    if name.is_empty()
        || name.contains(['/', '\0'])
        || [
            ".",
            "..",
            crate::PROJECT_LOCK_NAME,
            ".coding-stage",
            RUN_ANCHOR,
        ]
        .contains(&name)
    {
        return Err(CodingError::Invalid("unsupported selected basename"));
    }
    Ok(())
}
impl<'a> FileSlots<'a> {
    pub(crate) fn new(names: [&'a str; 2]) -> Result<Self, CodingError> {
        for name in names {
            validate_basename(name)?;
        }
        if names[0] == names[1] {
            return Err(CodingError::Invalid("duplicate selected basename"));
        }
        Ok(Self { names })
    }

    fn index(&self, path: &str) -> Result<usize, CodingError> {
        self.names
            .iter()
            .position(|name| *name == path)
            .ok_or(CodingError::Unsupported("file outside selected slots"))
    }

    pub(crate) fn absent(&self, root: &File, path: &str) -> Result<(), CodingError> {
        self.index(path)?;
        match statat(root, path, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => Err(CodingError::TargetMismatch),
        }
    }

    pub(crate) fn create(&self, root: &File, path: &str) -> Result<File, CodingError> {
        self.index(path)?;
        let file = File::from(openat(
            root,
            path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        crate::project::validate_stage(&file, 0o600)?;
        Ok(file)
    }

    pub(crate) fn capture(&self, root: &File, path: &str) -> Result<CapturedFile, CodingError> {
        self.index(path)?;
        let mut file = File::from(openat(
            root,
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        crate::project::validate_stage(&file, 0o600)?;
        let before = fstat(&file)?;
        if before.st_size < 0 || before.st_size > FILE_BYTES as i64 {
            return Err(CodingError::ResourceLimit("selected file resource limit"));
        }
        let mut bytes = [0; FILE_BYTES + 1];
        let mut count = 0;
        loop {
            let n = file.read(&mut bytes[count..])?;
            if n == 0 {
                break;
            }
            count += n;
            if count > FILE_BYTES {
                return Err(CodingError::ResourceLimit("selected file resource limit"));
            }
        }
        let after = fstat(&file)?;
        if (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_mode,
            before.st_nlink,
            before.st_uid,
            before.st_mtime,
            before.st_mtime_nsec,
            before.st_ctime,
            before.st_ctime_nsec,
        ) != (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_mode,
            after.st_nlink,
            after.st_uid,
            after.st_mtime,
            after.st_mtime_nsec,
            after.st_ctime,
            after.st_ctime_nsec,
        ) {
            return Err(CodingError::TargetMismatch);
        }
        crate::project::validate_stage(&file, 0o600)?;
        Ok(CapturedFile {
            path: path.into(),
            identity: [after.st_dev, after.st_ino],
            content: String::from_utf8(bytes[..count].to_vec())
                .map_err(|_| CodingError::Invalid("selected file is not UTF-8"))?,
        })
    }
}

#[cfg(test)]
mod tests;
