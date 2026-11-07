//! Files given on the command line as `@path`, sent with the first prompt as Scramjet sends them:
//! each in a `<file name="…">` element before the prompt's text, a text file with its contents,
//! an image with its note and the image itself, fitted as `read` fits it.
use super::{TYPED, image, tools};
use serde_json::{Value, json};
use std::path::Path;

pub struct Attached {
    /// The paths as given, for the prompt as shown.
    given: Vec<String>,
    /// The `<file>` elements.
    text: String,
    /// Responses `input_image` parts.
    images: Vec<Value>,
}

impl Attached {
    /// Reads the files at `paths`, relative to `cwd`; a leading `~` is the home directory. Empty
    /// files are left out, as Scramjet leaves them.
    pub fn read(paths: &[String], cwd: &Path) -> Result<Self, String> {
        let mut attached = Self {
            given: paths.to_vec(),
            text: String::new(),
            images: Vec::new(),
        };
        for given in paths {
            let path = tools::resolve(cwd, given);
            let name = path.display();
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!("file not found: {name}"));
                }
                Err(error) => return Err(format!("cannot read {name}: {error}")),
            };
            if bytes.is_empty() {
                continue;
            }
            let element = match image::mime(&bytes) {
                Some(mime) => match image::fit(&bytes, mime) {
                    Some(fitted) => {
                        attached.images.push(fitted.image.part());
                        format!(
                            "<file name=\"{name}\">{}</file>\n",
                            fitted.note().unwrap_or_default()
                        )
                    }
                    None => format!(
                        "<file name=\"{name}\">[Image omitted: could not be resized below the \
inline image size limit.]</file>\n"
                    ),
                },
                None => {
                    let text = String::from_utf8(bytes)
                        .map_err(|_| format!("{name} is not a UTF-8 text file or an image"))?;
                    format!("<file name=\"{name}\">\n{text}\n</file>\n")
                }
            };
            attached.text.push_str(&element);
        }
        Ok(attached)
    }

    /// `prompt` as shown and recalled: the paths as given, then the prompt.
    pub(super) fn typed(&self, prompt: &str) -> String {
        let mut words: Vec<String> = (self.given.iter())
            .map(|path| match path.contains(char::is_whitespace) {
                true => format!("@\"{path}\""),
                false => format!("@{path}"),
            })
            .collect();
        if !prompt.is_empty() {
            words.push(prompt.to_owned());
        }
        words.join(" ")
    }

    /// The text sent with `prompt`.
    pub(super) fn sent(&self, prompt: &str) -> String {
        format!("{}{prompt}", self.text)
    }

    /// The user message that sends `prompt` with the files: its text, then the images.
    pub(super) fn message(&self, prompt: &str) -> Value {
        let text = self.sent(prompt);
        let content = match self.images.is_empty() {
            true => json!(text),
            false => {
                let mut parts = vec![json!({"type":"input_text","text":text})];
                parts.extend(self.images.iter().cloned());
                json!(parts)
            }
        };
        json!({"role":"user","content":content,TYPED:self.typed(prompt)})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;

    fn scratch(name: &str) -> ScratchDir {
        let path = std::env::temp_dir().join(format!("scramjet-{name}-{}", std::process::id()));
        ScratchDir::new(path)
    }

    #[test]
    fn text_files_and_images_go_in_file_elements_before_the_prompt() {
        let scratch = scratch("attached");
        let dir = &scratch.0;
        std::fs::write(dir.join("notes.md"), "line one\nline two").unwrap();
        std::fs::write(dir.join("empty.txt"), "").unwrap();
        std::fs::create_dir(dir.join("shots")).unwrap();
        let png = image::tests::png(3, 2);
        std::fs::write(dir.join("shots/a shot.png"), &png).unwrap();
        let paths = ["notes.md", "empty.txt", "shots/a shot.png"].map(String::from);
        let attached = Attached::read(&paths, dir).unwrap();
        let (notes, shot) = (
            dir.join("notes.md"),
            crate::os::join(dir, "shots/a shot.png"),
        );
        let text = format!(
            "<file name=\"{}\">\nline one\nline two\n</file>\n<file name=\"{}\"></file>\nWhat is \
this?",
            notes.display(),
            shot.display()
        );
        let typed = "@notes.md @empty.txt @\"shots/a shot.png\" What is this?";
        assert_eq!(
            attached.message("What is this?"),
            json!({"role":"user","content":[
                {"type":"input_text","text":text},
                {"type":"input_image","detail":"auto",
                 "image_url":format!("data:image/png;base64,{}", image::base64(&png))},
            ],TYPED:typed})
        );
        // Without a prompt, the files are sent alone.
        let notes_only = Attached::read(&paths[..1], dir).unwrap();
        assert_eq!(
            notes_only.message(""),
            json!({"role":"user","content":format!("<file name=\"{}\">\nline one\nline two\n\
</file>\n", notes.display()),TYPED:"@notes.md"})
        );
    }

    #[test]
    fn a_missing_or_binary_file_is_refused() {
        let scratch = scratch("attached-refused");
        let dir = &scratch.0;
        let missing = Attached::read(&["gone.txt".to_owned()], dir).err().unwrap();
        assert_eq!(
            missing,
            format!("file not found: {}", dir.join("gone.txt").display())
        );
        std::fs::write(dir.join("blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
        let binary = Attached::read(&["blob.bin".to_owned()], dir).err().unwrap();
        assert_eq!(
            binary,
            format!(
                "{} is not a UTF-8 text file or an image",
                dir.join("blob.bin").display()
            )
        );
    }
}
