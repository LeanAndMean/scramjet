//! Skills, as Pi loads them (Scramjet's `docs/skills.md`): a directory with a `SKILL.md`, or a
//! `.md` file at the top of a `.scramjet/skills` directory. The instructions list them for the
//! model to read when a task matches, and `/skill:<name> args` sends one's instructions.
use super::Problem;
use super::claude_commands::{self, Variables};
use super::commands::{Set, split_frontmatter};
use super::yaml;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use yaml_rust2::Yaml;

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
    /// Found in one of Claude Code's `skills` directories, so `/skill:` expands it as Claude Code
    /// does: arguments and variables substituted, its shell commands run.
    pub claude: bool,
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
    /// One of Claude Code's `skills` directories, looked for when it exists.
    Claude,
    /// A file or directory the settings' `skills` list, which should exist.
    Listed,
    /// A file or directory `--skill` names, which should exist.
    Given,
}

/// The directories other agents read skills from, in a project and its parents and in the home
/// directory: the Agent Skills standard's (Pi's and Codex's) and Claude Code's.
const AGENTS: &str = ".agents/skills";
const CLAUDE: &str = ".claude/skills";

/// `places_of` with Claude Code's directory in `home`.
#[cfg(test)]
fn places(
    listed: &[PathBuf],
    sets: &[PathBuf],
    options: &SkillOptions,
    root: &Path,
    agent_directory: Option<&Path>,
    home: Option<&Path>,
) -> Vec<(PathBuf, Place)> {
    let claude = home.map(|home| crate::os::join(home, ".claude"));
    let user = User {
        home,
        claude: claude.as_deref(),
    };
    places_of(listed, sets, options, root, agent_directory, user)
}

/// The user's home directory and Claude Code's, `~/.claude` unless `CLAUDE_CONFIG_DIR` moves it.
#[derive(Clone, Copy)]
struct User<'a> {
    home: Option<&'a Path>,
    claude: Option<&'a Path>,
}

/// Where Pi looks, first wins: the project's `.scramjet/skills`, then the `.agents/skills` and
/// Claude Code's `.claude/skills` of the project and the directories above it up to its Git
/// root, the settings' `listed` paths, the `skills` directories of the command sets (as a Claude
/// Code plugin's), then the user's `skills` in Scramjet's agent directory, `~/.agents/skills`
/// and the `skills` of Claude Code's directory (with `--bare`, none of them, and never as a
/// project's); then, as Scramjet's, the paths `--skill` gives, which are all with `--no-skills`.
fn places_of(
    listed: &[PathBuf],
    sets: &[PathBuf],
    options: &SkillOptions,
    root: &Path,
    agent_directory: Option<&Path>,
    user: User,
) -> Vec<(PathBuf, Place)> {
    let given = (options.paths.iter()).map(|path| (path.clone(), Place::Given));
    if options.no_skills {
        return given.collect();
    }
    let nested = |path| (path, nested_place());
    let theirs = |home: Option<&Path>, claude: Option<&Path>| -> Vec<PathBuf> {
        let agents = home.map(|home| crate::os::join(home, AGENTS));
        let claude = claude.map(|claude| crate::os::join(claude, "skills"));
        agents.into_iter().chain(claude).collect()
    };
    let users = theirs(user.home, user.claude);
    // The project's path is the real one, which a home reached through a symlink is not.
    let real = |path: Option<&Path>| {
        let path = path?;
        (crate::os::canonicalize(path).ok()).or_else(|| {
            let parent = crate::os::canonicalize(path.parent()?).ok()?;
            Some(parent.join(path.file_name()?))
        })
    };
    let (real_home, real_claude) = (real(user.home), real(user.claude));
    let reals = theirs(real_home.as_deref(), real_claude.as_deref());
    let mut places = vec![(
        crate::os::join(root, ".scramjet/skills"),
        Place::Directory { top_files: true },
    )];
    for directory in root.ancestors() {
        for (shared, place) in [(AGENTS, nested_place()), (CLAUDE, Place::Claude)] {
            let skills = crate::os::join(directory, shared);
            if !users.contains(&skills) && !reals.contains(&skills) {
                places.push((skills, place));
            }
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    places.extend(listed.iter().map(|path| (path.clone(), Place::Listed)));
    places.extend(
        sets.iter()
            .map(|set| nested(crate::os::join(set, "skills"))),
    );
    if let Some(directory) = agent_directory {
        places.push((
            directory.join("skills"),
            Place::Directory { top_files: true },
        ));
    }
    if !super::bare() {
        let places_of = [nested_place(), Place::Claude];
        places.extend(users.into_iter().zip(places_of));
    }
    places.extend(given);
    places
}

fn nested_place() -> Place {
    Place::Directory { top_files: false }
}

impl Skills {
    /// The skills of a session in `root`, with the settings' `listed` files and directories,
    /// the command `sets`' own and the command line's `options`.
    pub fn discover(listed: &[PathBuf], sets: &[Set], options: &SkillOptions, root: &Path) -> Self {
        let agent_directory = super::login::config_directory().filter(|_| !super::bare());
        let (home, claude) = (std::env::home_dir(), super::login::claude_directory());
        let sets: Vec<PathBuf> = sets.iter().map(|set| set.directory.clone()).collect();
        let user = User {
            home: home.as_deref(),
            claude: claude.as_deref(),
        };
        Self::load(&places_of(
            listed,
            &sets,
            options,
            root,
            agent_directory.as_deref(),
            user,
        ))
    }

    fn load(places: &[(PathBuf, Place)]) -> Self {
        let mut found = Self::default();
        let mut loaded = HashSet::new();
        for (path, place) in places {
            let files = match place {
                Place::Directory { .. } | Place::Claude
                    if let Some(owner) = crate::os::stranger(path) =>
                {
                    let text = crate::os::left_out(path, owner);
                    found.warnings.push(Problem::in_file(text, path));
                    continue;
                }
                Place::Directory { top_files } => skill_files(path, *top_files),
                Place::Claude => skill_files(path, false),
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
                let loaded = load_skill(&file, *place == Place::Claude, &mut warnings);
                let in_file = |text| Problem::in_file(text, &file);
                found.warnings.extend(warnings.into_iter().map(in_file));
                let Some(skill) = loaded else {
                    continue;
                };
                match found.list.iter().find(|known| known.name == skill.name) {
                    // A copy, as a project keeps for Codex in `.agents` and Claude Code in
                    // `.claude`, is no collision either.
                    Some(known) if same_content(&known.path, &file) => {}
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

    /// The skills in `directory`, as found in a `.claude/skills` directory.
    #[cfg(test)]
    pub(super) fn of_claude(directory: &Path) -> Self {
        Self::load(&[(directory.to_path_buf(), Place::Claude)])
    }

    /// The skills in the `skills` directory of the command set in `directory`.
    pub(super) fn of_set(directory: &Path) -> Self {
        let skills = crate::os::join(directory, "skills");
        Self::load(&[(skills, Place::Directory { top_files: false })])
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

    /// The skill `/<name> args` names, as Claude Code runs one, or `/skill:<name> args`, as Pi
    /// does, and the arguments.
    fn called<'a>(&self, prompt: &'a str) -> Option<(&Skill, &'a str)> {
        let rest = prompt.strip_prefix('/')?;
        let rest = rest.strip_prefix("skill:").unwrap_or(rest);
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let skill = self.list.iter().find(|skill| skill.name == name)?;
        Some((skill, args))
    }

    /// The skill `/<name> args` or `/skill:<name> args` names.
    pub(super) fn named(&self, prompt: &str) -> Option<&Skill> {
        self.called(prompt).map(|(skill, _)| skill)
    }

    /// The text `/<name> args` or `/skill:<name> args` sends, as Pi's: the skill's instructions, read now, then
    /// the arguments. One of Claude Code's has them substituted as Claude Code does, then its
    /// `${CLAUDE_SKILL_DIR}` and `session`'s variables; its shell commands are left for the
    /// caller to run. `None` when the prompt names no skill; an error when its file cannot be
    /// read.
    pub(super) fn expand(
        &self,
        prompt: &str,
        session: &Variables,
    ) -> Option<Result<String, String>> {
        let (skill, args) = self.called(prompt)?;
        let name = &skill.name;
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
        let directory = skill.path.parent().unwrap_or(Path::new("/"));
        let (frontmatter, body) = split_frontmatter(content);
        let (body, args) = match skill.claude {
            true => {
                let names = yaml::first(&frontmatter)
                    .map(|frontmatter| claude_commands::argument_names(&frontmatter["arguments"]))
                    .unwrap_or_default();
                let body = claude_commands::substitute(body.trim(), args.trim(), &names);
                let directory = directory.display().to_string();
                let body = body.replace("${CLAUDE_SKILL_DIR}", &directory);
                (session.substitute(&body), "")
            }
            false => (body.trim().to_owned(), args.trim()),
        };
        let block = format!(
            "<skill name=\"{name}\" location=\"{path}\">\nReferences are relative to \
{}.\n\n{body}\n</skill>",
            directory.display()
        );
        Some(Ok(match args {
            "" => block,
            args => format!("{block}\n\n{args}"),
        }))
    }
}

fn same_content(one: &Path, other: &Path) -> bool {
    matches!((fs::read(one), fs::read(other)), (Ok(one), Ok(other)) if one == other)
}

/// The skill files under `directory`, as Pi finds them: its `SKILL.md`, which ends the search
/// there, else the `.md` files at its top when `top_files`, and those found in its directories,
/// leaving out hidden entries, `node_modules` and what the `.gitignore`, `.ignore` and
/// `.fdignore` files met on the way ignore. Links are followed, each directory once.
fn skill_files(directory: &Path, top_files: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut walk = Walk {
        seen: HashSet::new(),
        ignores: Vec::new(),
        files: &mut files,
    };
    walk.directory(directory, top_files);
    files
}

struct Walk<'a> {
    seen: HashSet<PathBuf>,
    ignores: Vec<Gitignore>,
    files: &'a mut Vec<PathBuf>,
}

impl Walk<'_> {
    fn directory(&mut self, directory: &Path, top_files: bool) {
        let Ok(real) = directory.canonicalize() else {
            return;
        };
        if !self.seen.insert(real) {
            return;
        }
        let mut rules = GitignoreBuilder::new(directory);
        for name in [".gitignore", ".ignore", ".fdignore"] {
            let file = directory.join(name);
            if file.is_file() {
                rules.add(file);
            }
        }
        let rules = rules.build().ok().filter(|rules| !rules.is_empty());
        let pushed = rules.is_some();
        self.ignores.extend(rules);
        self.entries(directory, top_files);
        if pushed {
            self.ignores.pop();
        }
    }

    fn entries(&mut self, directory: &Path, top_files: bool) {
        let skill = directory.join("SKILL.md");
        if skill.is_file() {
            if !self.ignored(&skill, false) {
                self.files.push(skill);
            }
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
                if !self.ignored(&path, true) {
                    self.directory(&path, false);
                }
            } else if top_files
                && name.ends_with(".md")
                && path.is_file()
                && !self.ignored(&path, false)
            {
                self.files.push(path);
            }
        }
    }

    /// Whether the deepest rule matching `path` ignores it, as git and Pi decide.
    fn ignored(&self, path: &Path, is_dir: bool) -> bool {
        self.ignores
            .iter()
            .rev()
            .map(|rules| rules.matched(path, is_dir))
            .find(|found| !found.is_none())
            .is_some_and(|found| found.is_ignore())
    }
}

/// The skill in `path`, with warnings where it breaks the standard's rules, as Pi loads it
/// regardless; `None`, with a warning, when it cannot be read or has no description.
fn load_skill(path: &Path, claude: bool, warnings: &mut Vec<String>) -> Option<Skill> {
    let shown = path.display();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            warnings.push(format!("skipping skill {shown}: {error}"));
            return None;
        }
    };
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let frontmatter = match yaml::first(&split_frontmatter(content).0) {
        Ok(frontmatter) => frontmatter,
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
        claude,
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
            &[PathBuf::from("/sets/mach12")],
            &SkillOptions::default(),
            root,
            Some(Path::new("/home/u/.config/scramjet")),
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
                (
                    PathBuf::from("/nonexistent/repo/sub/.claude/skills"),
                    Place::Claude
                ),
                (PathBuf::from("/nonexistent/repo/.agents/skills"), nested()),
                (
                    PathBuf::from("/nonexistent/repo/.claude/skills"),
                    Place::Claude
                ),
                (PathBuf::from("/nonexistent/.agents/skills"), nested()),
                (PathBuf::from("/nonexistent/.claude/skills"), Place::Claude),
                (PathBuf::from("/.agents/skills"), nested()),
                (PathBuf::from("/.claude/skills"), Place::Claude),
                (PathBuf::from("/listed"), Place::Listed),
                (PathBuf::from("/sets/mach12/skills"), nested()),
                (
                    PathBuf::from("/home/u/.config/scramjet/skills"),
                    Place::Directory { top_files: true }
                ),
                (PathBuf::from("/home/u/.agents/skills"), nested()),
                (PathBuf::from("/home/u/.claude/skills"), Place::Claude),
            ]
        );

        let scratch = scratch("places");
        let home = scratch.0.join("home");
        let project = home.join("repo/sub");
        fs::create_dir_all(home.join("repo/.git")).unwrap();
        fs::create_dir_all(&project).unwrap();
        let found = places(
            &[],
            &[],
            &SkillOptions::default(),
            &project,
            None,
            Some(&home),
        );
        let found: Vec<_> = found.iter().map(|(path, _)| path.clone()).collect();
        assert_eq!(
            found,
            [
                project.join(".scramjet/skills"),
                project.join(".agents/skills"),
                project.join(".claude/skills"),
                home.join("repo/.agents/skills"),
                home.join("repo/.claude/skills"),
                home.join(".agents/skills"),
                home.join(".claude/skills"),
            ]
        );
        // The user's `~/.agents/skills` and `~/.claude/skills` are never also a project's.
        let found = places(&[], &[], &SkillOptions::default(), &home, None, Some(&home));
        for shared in [AGENTS, CLAUDE] {
            let user = home.join(shared);
            assert_eq!(found.iter().filter(|(path, _)| path == &user).count(), 1);
        }
        assert_eq!(found.last().unwrap().0, home.join(".claude/skills"));
        // Nor when the home directory is reached through a symlink, as macOS's `/var`.
        #[cfg(unix)]
        {
            let linked = scratch.0.join("linked-home");
            std::os::unix::fs::symlink(&home, &linked).unwrap();
            // A session's root is the real path.
            let real = std::fs::canonicalize(&home).unwrap();
            let found = places(
                &[],
                &[],
                &SkillOptions::default(),
                &real,
                None,
                Some(&linked),
            );
            let paths: Vec<_> = found.iter().map(|(path, _)| path.clone()).collect();
            assert!(!paths.contains(&real.join(".agents/skills")), "{paths:?}");
            assert!(!paths.contains(&real.join(".claude/skills")), "{paths:?}");
            assert_eq!(found.last().unwrap().0, linked.join(".claude/skills"));
        }
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
        // A copy of one loaded, as a project keeps in `.agents` and `.claude`, is no collision.
        write(&agents.join("deep/SKILL.md"), &skill("deep", "Nested."));
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
            &[PathBuf::from("/set")],
            &given,
            Path::new("/"),
            None,
            Some(Path::new("/home/u")),
        );
        let last: Vec<_> = found.iter().rev().take(3).rev().collect();
        assert_eq!(
            last,
            [
                &(PathBuf::from("/home/u/.claude/skills"), Place::Claude),
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
            &[PathBuf::from("/set")],
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
    fn ignore_files_met_on_the_way_leave_skills_out_deepest_rule_first() {
        let scratch = scratch("ignored");
        let top = &scratch.0;
        for (name, text) in [
            (".gitignore", "build/\ndraft.md\n"),
            (".ignore", "/private\nold\n"),
            ("group/.fdignore", "!old\n"),
            ("group/later/.gitignore", "SKILL.md\n"),
        ] {
            write(&top.join(name), text);
        }
        for path in [
            "top.md",
            "draft.md",
            "build/made/SKILL.md",
            "private/SKILL.md",
            "old/SKILL.md",
            "group/build/SKILL.md",
            "group/private/SKILL.md",
            "group/old/SKILL.md",
            "group/new/SKILL.md",
            "group/later/SKILL.md",
        ] {
            write(&top.join(path), &skill("x", "X."));
        }
        let files = skill_files(top, true);
        let found: Vec<_> = files.iter().map(|f| f.strip_prefix(top).unwrap()).collect();
        // As in git: unanchored rules reach every depth, `/private` only the top, and the
        // nearest file's `!old` lets group/old back in.
        assert_eq!(
            found,
            [
                Path::new("group/new/SKILL.md"),
                Path::new("group/old/SKILL.md"),
                Path::new("group/private/SKILL.md"),
                Path::new("top.md"),
            ]
        );
    }

    impl Skills {
        fn expand_now(&self, prompt: &str) -> Option<Result<String, String>> {
            let variables = Variables {
                project: PathBuf::from("/work"),
                session: "S1".to_owned(),
                effort: "low".to_owned(),
            };
            self.expand(prompt, &variables)
        }
    }

    #[test]
    fn claude_codes_skills_are_expanded_as_claude_code_does() {
        let scratch = scratch("claude-expand");
        let claude = crate::os::join(&scratch.0, ".claude/skills");
        let file = crate::os::join(&claude, "brief/SKILL.md");
        write(
            &file,
            "---\ndescription: Briefs.\narguments: [topic]\n---\nOn $topic from \
${CLAUDE_SKILL_DIR} in ${CLAUDE_PROJECT_DIR}, ${CLAUDE_SESSION_ID}, ${CLAUDE_EFFORT}.\n\
Status: !`git status`",
        );
        write(
            &crate::os::join(&claude, "plain/SKILL.md"),
            "---\ndescription: Plain.\n---\nPlain.",
        );
        let found = Skills::of_claude(&claude);
        assert!(found.list().iter().all(|skill| skill.claude));
        let directory = file.parent().unwrap().display().to_string();
        let block = |body: &str, path: &Path| {
            format!(
                "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n\
{body}\n</skill>",
                path.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy(),
                path.display(),
                path.parent().unwrap().display()
            )
        };
        // Arguments go in the body, not after it, and cannot start a shell command there.
        assert_eq!(
            found.expand_now("/skill:brief !`id` rest"),
            Some(Ok(block(
                &format!("On \\! `id` from {directory} in /work, S1, low.\nStatus: !`git status`"),
                &file
            )))
        );
        let plain = crate::os::join(&claude, "plain/SKILL.md");
        assert_eq!(
            found.expand_now("/skill:plain more"),
            Some(Ok(block("Plain.\n\nARGUMENTS: more", &plain)))
        );
        let others = Skills::in_directory(&claude);
        assert!(others.list().iter().all(|skill| !skill.claude));
        assert!(
            (others.expand_now("/skill:brief x").unwrap().unwrap())
                .ends_with("${CLAUDE_EFFORT}.\nStatus: !`git status`\n</skill>\n\nx")
        );
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
        assert_eq!(found.expand_now("/skill:pdf"), Some(Ok(block.clone())));
        assert_eq!(
            found.expand_now("/skill:pdf  fill form.pdf\n"),
            Some(Ok(format!("{block}\n\nfill form.pdf")))
        );
        // Claude Code's `/<name>` runs it too.
        assert_eq!(
            found.expand_now("/pdf  fill form.pdf\n"),
            Some(Ok(format!("{block}\n\nfill form.pdf")))
        );
        // A skill left out of the instructions still runs; others are not skill prompts.
        assert!(
            found
                .expand_now("/skill:manual")
                .unwrap()
                .unwrap()
                .contains("Manual steps.")
        );
        assert_eq!(found.expand_now("/skill:other"), None);
        assert_eq!(found.expand_now("skill:pdf"), None);
        assert_eq!(found.expand_now("/skill:pdfx"), None);
        assert_eq!(found.expand_now("/pdfx"), None);
        assert_eq!(found.expand_now("/"), None);

        fs::remove_file(&pdf).unwrap();
        let error = found.expand_now("/skill:pdf").unwrap().unwrap_err();
        assert!(error.starts_with(&format!("skill pdf: cannot read {}", pdf.display())));
        assert!(Skills::default().catalog().is_empty());
    }
}
