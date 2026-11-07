//! Skills, as Pi loads them (Scramjet's `docs/skills.md`): a directory with a `SKILL.md`, or a
//! `.md` file at the top of a `.scramjet/skills` directory. The instructions list them for the
//! model to read when a task matches, and `/skill:<name> args` sends one's instructions.
use super::Problem;
use super::commands::split_frontmatter;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use yaml_rust2::{Yaml, YamlLoader};

/// The Agent Skills standard's limits, which Pi only warns about.
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 1024;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// Listed in the instructions; `disable-model-invocation: true` leaves it to `/skill:`.
    pub listed: bool,
}

/// What the command line says about skills, as Scramjet's `--skill` and `--no-skills`.
#[derive(Clone, Debug, Default)]
pub struct SkillOptions {
    /// `--skill`: files and directories looked in after all the others.
    pub paths: Vec<PathBuf>,
    /// `--no-skills`: only those `paths`, none found by default or listed in the settings.
    pub no_skills: bool,
}

/// The skills a session has, in order of precedence, and why files were skipped.
#[derive(Clone, Debug, Default)]
pub struct Skills {
    list: Vec<Skill>,
    warnings: Vec<Problem>,
}

/// A place skills are looked for.
#[derive(Debug, PartialEq)]
enum Place {
    /// Looked for when it exists; `.md` files at its top are skills when `top_files`.
    Directory { top_files: bool },
    /// A file or directory the settings' `skills` list, which should exist.
    Listed,
    /// A file or directory `--skill` names, which should exist.
    Given,
}

/// Where Pi looks, first wins: the project's `.scramjet/skills` and the `.agents/skills` of the
/// project and the directories above it up to its Git root, the settings' `listed` paths, then
/// the user's `skills` in Scramjet's agent directory and `~/.agents/skills`; then, as
/// Scramjet's, the paths `--skill` gives, which are all with `--no-skills`.
fn places(
    listed: &[PathBuf],
    options: &SkillOptions,
    root: &Path,
    agent_directory: Option<&Path>,
    home: Option<&Path>,
) -> Vec<(PathBuf, Place)> {
    let given = (options.paths.iter()).map(|path| (path.clone(), Place::Given));
    if options.no_skills {
        return given.collect();
    }
    let user_agents = home.map(|home| crate::os::join(home, ".agents/skills"));
    let mut places = vec![(
        crate::os::join(root, ".scramjet/skills"),
        Place::Directory { top_files: true },
    )];
    for directory in root.ancestors() {
        let skills = crate::os::join(directory, ".agents/skills");
        if user_agents.as_ref() != Some(&skills) {
            places.push((skills, Place::Directory { top_files: false }));
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    places.extend(listed.iter().map(|path| (path.clone(), Place::Listed)));
    if let Some(directory) = agent_directory {
        places.push((
            directory.join("skills"),
            Place::Directory { top_files: true },
        ));
    }
    places.extend(user_agents.map(|skills| (skills, Place::Directory { top_files: false })));
    places.extend(given);
    places
}

impl Skills {
    /// The skills of a session in `root`, with the settings' `listed` files and directories and
    /// the command line's `options`.
    pub fn discover(listed: &[PathBuf], options: &SkillOptions, root: &Path) -> Self {
        let agent_directory = super::subagent::agent_directory();
        let home = std::env::home_dir();
        Self::load(&places(
            listed,
            options,
            root,
            agent_directory.as_deref(),
            home.as_deref(),
        ))
    }

    fn load(places: &[(PathBuf, Place)]) -> Self {
        let mut found = Self::default();
        let mut loaded = HashSet::new();
        for (path, place) in places {
            let files = match place {
                Place::Directory { top_files } => skill_files(path, *top_files),
                _ if path.is_dir() => skill_files(path, true),
                _ if path.is_file() && path.extension() == Some("md".as_ref()) => {
                    vec![path.clone()]
                }
                _ => {
                    let wrong = match path.exists() {
                        true => "is not a markdown file",
                        false => "does not exist",
                    };
                    let shown = path.display();
                    found.warnings.push(match place {
                        Place::Given => {
                            Problem::new(format!("skill path given with --skill {wrong}: {shown}"))
                        }
                        _ => Problem::in_settings(format!("skill path {wrong}: {shown}")),
                    });
                    continue;
                }
            };
            for file in files {
                // The same file reached again, through a link or another place, is no collision.
                if !loaded.insert(file.canonicalize().unwrap_or_else(|_| file.clone())) {
                    continue;
                }
                let mut warnings = Vec::new();
                let loaded = load_skill(&file, &mut warnings);
                let in_file = |text| Problem::in_file(text, &file);
                found.warnings.extend(warnings.into_iter().map(in_file));
                let Some(skill) = loaded else {
                    continue;
                };
                match found.list.iter().find(|known| known.name == skill.name) {
                    Some(known) => found.warnings.push(Problem::in_file(
                        format!(
                            "skipping skill {} at {}: name already loaded from {}",
                            skill.name,
                            file.display(),
                            known.path.display()
                        ),
                        &file,
                    )),
                    None => found.list.push(skill),
                }
            }
        }
        found
    }

    /// The skills in `directory`, as found in a `.scramjet/skills` directory.
    #[cfg(test)]
    pub(super) fn in_directory(directory: &Path) -> Self {
        Self::load(&[(
            directory.to_path_buf(),
            Place::Directory { top_files: true },
        )])
    }

    pub fn list(&self) -> &[Skill] {
        &self.list
    }

    /// Why skill files were skipped, and where they break the standard's rules.
    pub fn warnings(&self) -> &[Problem] {
        &self.warnings
    }

    /// Pi's skills section of the instructions, empty without skills the model may load.
    pub(super) fn catalog(&self) -> String {
        let listed: Vec<String> = (self.list.iter())
            .filter(|skill| skill.listed)
            .map(|skill| {
                format!(
                    "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    \
<location>{}</location>\n  </skill>\n",
                    escape(&skill.name),
                    escape(&skill.description),
                    escape(&skill.path.display().to_string())
                )
            })
            .collect();
        if listed.is_empty() {
            return String::new();
        }
        format!(
            "The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of \
SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
{}</available_skills>
",
            listed.concat()
        )
    }

    /// The text `/skill:<name> args` sends, as Pi's: the skill's instructions, read now, then
    /// the arguments. `None` when the prompt names no skill; an error when its file cannot be
    /// read.
    pub(super) fn expand(&self, prompt: &str) -> Option<Result<String, String>> {
        let rest = prompt.strip_prefix("/skill:")?;
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let skill = self.list.iter().find(|skill| skill.name == name)?;
        let path = skill.path.display();
        let content = match fs::read_to_string(&skill.path) {
            Ok(content) => content,
            Err(error) => {
                return Some(Err(format!(
                    "skill {name}: cannot read {path} ({error}); the prompt was sent as typed"
                )));
            }
        };
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        let directory = skill.path.parent().unwrap_or(Path::new("/")).display();
        let block = format!(
            "<skill name=\"{name}\" location=\"{path}\">\nReferences are relative to \
{directory}.\n\n{}\n</skill>",
            split_frontmatter(content).1.trim()
        );
        Some(Ok(match args.trim() {
            "" => block,
            args => format!("{block}\n\n{args}"),
        }))
    }
}

/// The skill files under `directory`, as Pi finds them: its `SKILL.md`, which ends the search
/// there, else the `.md` files at its top when `top_files`, and those found in its directories,
/// leaving out hidden entries and `node_modules`. Links are followed, each directory once.
fn skill_files(directory: &Path, top_files: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(directory, top_files, &mut HashSet::new(), &mut files);
    files
}

fn walk(directory: &Path, top_files: bool, seen: &mut HashSet<PathBuf>, files: &mut Vec<PathBuf>) {
    let Ok(real) = directory.canonicalize() else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let skill = directory.join("SKILL.md");
    if skill.is_file() {
        files.push(skill);
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| !name.starts_with('.') && name != "node_modules")
        .collect();
    entries.sort();
    for (name, path) in entries {
        if path.is_dir() {
            walk(&path, false, seen, files);
        } else if top_files && name.ends_with(".md") && path.is_file() {
            files.push(path);
        }
    }
}

/// The skill in `path`, with warnings where it breaks the standard's rules, as Pi loads it
/// regardless; `None`, with a warning, when it cannot be read or has no description.
fn load_skill(path: &Path, warnings: &mut Vec<String>) -> Option<Skill> {
    let shown = path.display();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            warnings.push(format!("skipping skill {shown}: {error}"));
            return None;
        }
    };
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let frontmatter = match YamlLoader::load_from_str(&split_frontmatter(content).0) {
        Ok(documents) => documents.into_iter().next().unwrap_or(Yaml::Null),
        Err(error) => {
            warnings.push(format!(
                "skipping skill {shown}: malformed frontmatter — {error}"
            ));
            return None;
        }
    };
    let text = |key: &str| {
        frontmatter[key]
            .as_str()
            .filter(|text| !text.trim().is_empty())
    };
    let directory = path.parent().and_then(Path::file_name);
    let name = text("name").map(str::to_owned).unwrap_or_else(|| {
        directory.map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    });
    for problem in name_problems(&name) {
        warnings.push(format!("skill {shown}: {problem}"));
    }
    let Some(description) = text("description") else {
        warnings.push(format!("skipping skill {shown}: description is required"));
        return None;
    };
    let length = description.chars().count();
    if length > MAX_DESCRIPTION {
        warnings.push(format!(
            "skill {shown}: description exceeds the Agent Skills limit of {MAX_DESCRIPTION} \
characters ({length}); it is kept whole"
        ));
    }
    Some(Skill {
        name,
        description: description.to_owned(),
        path: path.to_path_buf(),
        listed: frontmatter["disable-model-invocation"] != Yaml::Boolean(true),
    })
}

/// Where `name` breaks the Agent Skills standard's naming rules.
fn name_problems(name: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let length = name.chars().count();
    if length > MAX_NAME {
        problems.push(format!("name exceeds {MAX_NAME} characters ({length})"));
    }
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    if name.is_empty() || !name.chars().all(allowed) {
        problems.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".into(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        problems.push("name must not start or end with a hyphen".into());
    }
    if name.contains("--") {
        problems.push("name must not contain consecutive hyphens".into());
    }
    problems
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::super::ProblemFile;
    use super::super::tests::ScratchDir;
    use super::*;

    fn scratch(name: &str) -> ScratchDir {
        let path =
            std::env::temp_dir().join(format!("scramjet-skills-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        ScratchDir::new(path)
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn skill(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nDo it.\n")
    }

    #[test]
    fn places_are_pis_with_the_project_first_and_agents_directories_up_to_the_git_root() {
        let root = Path::new("/nonexistent/repo/sub");
        let found = places(
            &[PathBuf::from("/listed")],
            &SkillOptions::default(),
            root,
            Some(Path::new("/home/u/.scramjet/agent")),
            Some(Path::new("/home/u")),
        );
        let top = Place::Directory { top_files: true };
        let nested = || Place::Directory { top_files: false };
        // Without a Git root the walk goes on to `/`.
        assert_eq!(
            found,
            [
                (PathBuf::from("/nonexistent/repo/sub/.scramjet/skills"), top),
                (
                    PathBuf::from("/nonexistent/repo/sub/.agents/skills"),
                    nested()
                ),
                (PathBuf::from("/nonexistent/repo/.agents/skills"), nested()),
                (PathBuf::from("/nonexistent/.agents/skills"), nested()),
                (PathBuf::from("/.agents/skills"), nested()),
                (PathBuf::from("/listed"), Place::Listed),
                (
                    PathBuf::from("/home/u/.scramjet/agent/skills"),
                    Place::Directory { top_files: true }
                ),
                (PathBuf::from("/home/u/.agents/skills"), nested()),
            ]
        );

        let scratch = scratch("places");
        let home = scratch.0.join("home");
        let project = home.join("repo/sub");
        fs::create_dir_all(home.join("repo/.git")).unwrap();
        fs::create_dir_all(&project).unwrap();
        let found = places(&[], &SkillOptions::default(), &project, None, Some(&home));
        let found: Vec<_> = found.iter().map(|(path, _)| path.clone()).collect();
        assert_eq!(
            found,
            [
                project.join(".scramjet/skills"),
                project.join(".agents/skills"),
                home.join("repo/.agents/skills"),
                home.join(".agents/skills"),
            ]
        );
        // The user's `~/.agents/skills` is never also a project's.
        let found = places(&[], &SkillOptions::default(), &home, None, Some(&home));
        let user = home.join(".agents/skills");
        assert_eq!(found.iter().filter(|(path, _)| path == &user).count(), 1);
        assert_eq!(found.last().unwrap().0, user);
    }

    #[cfg(unix)]
    #[test]
    fn skills_are_found_as_pi_finds_them_and_the_first_of_a_name_wins() {
        let scratch = scratch("find");
        let project = scratch.0.join("project/.scramjet/skills");
        write(&project.join("top.md"), &skill("top", "A file at the top."));
        write(
            &project.join("pdf/SKILL.md"),
            &skill("pdf", "Project PDFs."),
        );
        // A directory with a SKILL.md is one skill; nothing under it is looked at.
        write(
            &project.join("pdf/inner/SKILL.md"),
            &skill("inner", "Never found."),
        );
        write(
            &project.join("group/deep/SKILL.md"),
            &skill("deep", "Nested."),
        );
        write(
            &project.join("group/notes.md"),
            &skill("notes", "Not at the top."),
        );
        write(
            &project.join(".hidden/SKILL.md"),
            &skill("hidden", "Hidden."),
        );
        write(
            &project.join("node_modules/x/SKILL.md"),
            &skill("dep", "A dependency."),
        );
        let agents = scratch.0.join("agents");
        write(
            &agents.join("top.md"),
            &skill("agents-top", "Not a skill here."),
        );
        write(
            &agents.join("pdf/SKILL.md"),
            &skill("pdf", "The user's PDFs."),
        );
        write(
            &agents.join("bare/SKILL.md"),
            "---\ndescription: Named after its directory.\n---\nBody",
        );
        write(
            &agents.join("none/SKILL.md"),
            "---\nname: none\n---\nNo description.",
        );
        write(&agents.join("bad/SKILL.md"), "---\nname: [\n---\n");
        let listed = scratch.0.join("one.md");
        write(&listed, &skill("Bad--Name-", "Listed in the settings."));
        // A link to a directory already loaded adds nothing and is no collision.
        std::os::unix::fs::symlink(project.join("pdf"), scratch.0.join("link")).unwrap();

        let found = Skills::load(&[
            (project.clone(), Place::Directory { top_files: true }),
            (agents.clone(), Place::Directory { top_files: false }),
            (listed.clone(), Place::Listed),
            (scratch.0.join("link"), Place::Listed),
            (scratch.0.join("missing"), Place::Listed),
            (
                scratch.0.join("missing-dir"),
                Place::Directory { top_files: true },
            ),
        ]);
        let names: Vec<_> = found
            .list()
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, ["deep", "pdf", "top", "bare", "Bad--Name-"]);
        assert_eq!(found.list()[1].path, project.join("pdf/SKILL.md"));
        let shown = |path: PathBuf| path.display().to_string();
        let (malformed, warnings) = found.warnings().split_first().unwrap();
        let bad = shown(agents.join("bad/SKILL.md"));
        assert!(
            (malformed.text)
                .starts_with(&format!("skipping skill {bad}: malformed frontmatter — ")),
            "{malformed}"
        );
        assert_eq!(
            malformed.file,
            Some(ProblemFile::Path(agents.join("bad/SKILL.md")))
        );
        // What the settings list is a problem there.
        let files: Vec<_> = warnings
            .iter()
            .map(|warning| warning.file.clone())
            .collect();
        let none = Some(ProblemFile::Path(agents.join("none/SKILL.md")));
        assert_eq!(
            (&files[0], files.last().unwrap()),
            (&none, &Some(ProblemFile::Settings))
        );
        let warnings: Vec<&str> = warnings
            .iter()
            .map(|warning| warning.text.as_str())
            .collect();
        assert_eq!(
            warnings,
            [
                format!(
                    "skipping skill {}: description is required",
                    shown(agents.join("none/SKILL.md"))
                ),
                format!(
                    "skipping skill pdf at {}: name already loaded from {}",
                    shown(agents.join("pdf/SKILL.md")),
                    shown(project.join("pdf/SKILL.md"))
                ),
                format!(
                    "skill {}: name contains invalid characters (must be lowercase a-z, 0-9, \
hyphens only)",
                    shown(listed.clone())
                ),
                format!(
                    "skill {}: name must not start or end with a hyphen",
                    shown(listed.clone())
                ),
                format!(
                    "skill {}: name must not contain consecutive hyphens",
                    shown(listed.clone())
                ),
                format!(
                    "skill path does not exist: {}",
                    shown(scratch.0.join("missing"))
                ),
            ]
        );
    }

    #[test]
    fn skill_options_add_paths_after_all_others_and_no_skills_leaves_only_them() {
        let given = SkillOptions {
            paths: vec![PathBuf::from("/given"), PathBuf::from("/more.md")],
            no_skills: false,
        };
        let found = places(
            &[PathBuf::from("/listed")],
            &given,
            Path::new("/"),
            None,
            Some(Path::new("/home/u")),
        );
        let last: Vec<_> = found.iter().rev().take(3).rev().collect();
        assert_eq!(
            last,
            [
                &(
                    PathBuf::from("/home/u/.agents/skills"),
                    Place::Directory { top_files: false }
                ),
                &(PathBuf::from("/given"), Place::Given),
                &(PathBuf::from("/more.md"), Place::Given),
            ]
        );
        let only = SkillOptions {
            no_skills: true,
            ..given
        };
        let found = places(
            &[PathBuf::from("/listed")],
            &only,
            Path::new("/"),
            None,
            None,
        );
        assert_eq!(
            found,
            [
                (PathBuf::from("/given"), Place::Given),
                (PathBuf::from("/more.md"), Place::Given),
            ]
        );
        assert!(
            places(
                &[],
                &SkillOptions {
                    no_skills: true,
                    ..Default::default()
                },
                Path::new("/"),
                None,
                None
            )
            .is_empty()
        );

        // A name loaded before wins; a path that is no skill is a problem of no file.
        let scratch = scratch("given");
        write(
            &scratch.0.join("found/pdf/SKILL.md"),
            &skill("pdf", "Found."),
        );
        write(
            &scratch.0.join("given/pdf/SKILL.md"),
            &skill("pdf", "Given."),
        );
        write(
            &scratch.0.join("given/csv/SKILL.md"),
            &skill("csv", "Given."),
        );
        write(&scratch.0.join("notes.txt"), "");
        let found = Skills::load(&[
            (
                scratch.0.join("found"),
                Place::Directory { top_files: true },
            ),
            (scratch.0.join("given"), Place::Given),
            (scratch.0.join("notes.txt"), Place::Given),
            (scratch.0.join("missing"), Place::Given),
        ]);
        let skills: Vec<_> = (found.list().iter())
            .map(|skill| (skill.name.as_str(), skill.description.as_str()))
            .collect();
        assert_eq!(skills, [("pdf", "Found."), ("csv", "Given.")]);
        let problems: Vec<_> = (found.warnings().iter().skip(1))
            .map(|problem| (problem.text.clone(), problem.file.clone()))
            .collect();
        assert_eq!(
            problems,
            [
                (
                    format!(
                        "skill path given with --skill is not a markdown file: {}",
                        scratch.0.join("notes.txt").display()
                    ),
                    None
                ),
                (
                    format!(
                        "skill path given with --skill does not exist: {}",
                        scratch.0.join("missing").display()
                    ),
                    None
                ),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_up_the_tree_is_followed_once() {
        let scratch = scratch("cycle");
        write(&scratch.0.join("a/one/SKILL.md"), &skill("one", "One."));
        std::os::unix::fs::symlink(&scratch.0, scratch.0.join("a/up")).unwrap();
        let files = skill_files(&scratch.0, true);
        assert_eq!(files, [scratch.0.join("a/one/SKILL.md")]);
    }

    #[test]
    fn the_catalog_lists_skills_the_model_may_load_and_a_skill_prompt_sends_its_instructions() {
        let scratch = scratch("expand");
        let pdf = crate::os::join(&scratch.0, "pdf/SKILL.md");
        write(&pdf, &skill("pdf", "Fills <forms> & \"merges\" PDFs."));
        write(
            &scratch.0.join("manual/SKILL.md"),
            "---\nname: manual\ndescription: Only when asked.\ndisable-model-invocation: true\n\
---\nManual steps.",
        );
        let found = Skills::load(&[(scratch.0.clone(), Place::Directory { top_files: true })]);
        assert_eq!(
            found.catalog(),
            format!(
                "The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of \
SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>pdf</name>
    <description>Fills &lt;forms&gt; &amp; &quot;merges&quot; PDFs.</description>
    <location>{}</location>
  </skill>
</available_skills>
",
                pdf.display()
            )
        );
        let directory = scratch.0.join("pdf");
        let block = format!(
            "<skill name=\"pdf\" location=\"{}\">\nReferences are relative to {}.\n\n# pdf\n\n\
Do it.\n</skill>",
            pdf.display(),
            directory.display()
        );
        assert_eq!(found.expand("/skill:pdf"), Some(Ok(block.clone())));
        assert_eq!(
            found.expand("/skill:pdf  fill form.pdf\n"),
            Some(Ok(format!("{block}\n\nfill form.pdf")))
        );
        // A skill left out of the instructions still runs; others are not skill prompts.
        assert!(
            found
                .expand("/skill:manual")
                .unwrap()
                .unwrap()
                .contains("Manual steps.")
        );
        assert_eq!(found.expand("/skill:other"), None);
        assert_eq!(found.expand("skill:pdf"), None);
        assert_eq!(found.expand("/skill:pdfx"), None);

        fs::remove_file(&pdf).unwrap();
        let error = found.expand("/skill:pdf").unwrap().unwrap_err();
        assert!(error.starts_with(&format!("skill pdf: cannot read {}", pdf.display())));
        assert!(Skills::default().catalog().is_empty());
    }
}
