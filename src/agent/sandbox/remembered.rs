//! Write access the user granted "always for this project": `sandbox-grants.json` in the user's
//! configuration directory, an object from a project's real path to the directories granted
//! there. Never read from a repository, so a project cannot grant itself access.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub struct Remembered {
    file: PathBuf,
}

impl Remembered {
    /// The user's, in `$XDG_CONFIG_HOME/scramjet`.
    pub fn user() -> Option<Self> {
        crate::agent::login::config_directory().map(|directory| Self {
            file: directory.join("sandbox-grants.json"),
        })
    }

    #[cfg(test)]
    pub(crate) fn at(file: PathBuf) -> Self {
        Self { file }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The directories granted always in the project at `root`.
    pub fn of(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        let object = self.read()?;
        let listed = object.get(&key(root)).and_then(Value::as_array);
        Ok((listed.into_iter().flatten())
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect())
    }

    /// Keeps `directories` granted always in the project at `root`.
    pub fn add(&self, root: &Path, directories: &[PathBuf]) -> Result<(), String> {
        self.change(root, |listed| {
            for directory in directories {
                let directory = Value::from(directory.display().to_string());
                if !listed.contains(&directory) {
                    listed.push(directory);
                }
            }
        })
        .map(drop)
    }

    /// Forgets `directory` in the project at `root`; whether it was kept.
    pub fn remove(&self, root: &Path, directory: &Path) -> Result<bool, String> {
        let directory = Value::from(directory.display().to_string());
        self.change(root, |listed| listed.retain(|kept| *kept != directory))
    }

    /// Changes the project's list, and says whether that changed it.
    fn change(&self, root: &Path, change: impl FnOnce(&mut Vec<Value>)) -> Result<bool, String> {
        let mut object = self.read()?;
        let project = key(root);
        let mut listed = match object.remove(&project) {
            Some(Value::Array(listed)) => listed,
            _ => Vec::new(),
        };
        let before = listed.clone();
        change(&mut listed);
        if listed == before {
            return Ok(false);
        }
        if !listed.is_empty() {
            object.insert(project, Value::Array(listed));
        }
        let text = serde_json::to_string_pretty(&Value::Object(object))
            .map_err(|error| error.to_string())?;
        crate::agent::tools::write_replacing(&self.file, &(text + "\n"))
            .map_err(|error| format!("{}: {error}", self.shown()))?;
        Ok(true)
    }

    fn read(&self) -> Result<Map<String, Value>, String> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(error) => return Err(format!("{}: {error}", self.shown())),
        };
        match serde_json::from_str(&text) {
            Ok(Value::Object(object)) => Ok(object),
            Ok(_) => Err(format!("{}: not a JSON object", self.shown())),
            Err(error) => Err(format!("{}: {error}", self.shown())),
        }
    }

    fn shown(&self) -> String {
        crate::agent::tools::shortened(&self.file.display().to_string())
    }
}

fn key(root: &Path) -> String {
    let project = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    project.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_are_kept_per_project_and_forgotten() {
        let scratch =
            std::env::temp_dir().join(format!("scramjet-remembered-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(scratch.clone());
        let (one, two) = (scratch.join("one"), scratch.join("two"));
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        let remembered = Remembered::at(scratch.join("config/sandbox-grants.json"));
        assert_eq!(remembered.of(&one).unwrap(), Vec::<PathBuf>::new());
        let docs = PathBuf::from("/somewhere/docs");
        remembered.add(&one, &[docs.clone(), docs.clone()]).unwrap();
        remembered.add(&one, std::slice::from_ref(&docs)).unwrap();
        assert_eq!(remembered.of(&one).unwrap(), vec![docs.clone()]);
        assert!(remembered.of(&two).unwrap().is_empty());
        assert!(!remembered.remove(&two, &docs).unwrap());
        assert!(remembered.remove(&one, &docs).unwrap());
        assert!(remembered.of(&one).unwrap().is_empty());
        let text = std::fs::read_to_string(remembered.file()).unwrap();
        assert_eq!(text, "{}\n");
        std::fs::write(remembered.file(), "[]").unwrap();
        assert!(
            remembered
                .of(&one)
                .unwrap_err()
                .ends_with("not a JSON object")
        );
        assert!(remembered.add(&one, &[docs]).is_err());
        assert_eq!(std::fs::read_to_string(remembered.file()).unwrap(), "[]");
    }
}
