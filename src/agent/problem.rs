//! Something wrong in what a session loaded, with where the user can fix it.
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub text: String,
    pub file: Option<ProblemFile>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProblemFile {
    /// The file at fault, or a link to it.
    Path(PathBuf),
    /// The settings file, which lists what is wrong.
    Settings,
}

impl Problem {
    /// A problem no one file can fix, such as a directory that cannot be read.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            file: None,
        }
    }

    pub fn in_file(text: impl Into<String>, file: &Path) -> Self {
        Self {
            text: text.into(),
            file: Some(ProblemFile::Path(file.to_path_buf())),
        }
    }

    pub fn in_settings(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            file: Some(ProblemFile::Settings),
        }
    }

    /// The same problem, its text after `prefix`.
    pub fn prefixed(self, prefix: &str) -> Self {
        Self {
            text: format!("{prefix}{}", self.text),
            ..self
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.text)
    }
}
