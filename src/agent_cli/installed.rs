//! `/settings`' Installed: what the session loaded, each with where it comes from, and what is
//! wrong there, so that the user can find and fix it. Launch says at most one line about it.
use super::keymap::{self, Binding};
use scramjet_rs::agent::{
    AgentSource, Command, Installed, Origin, Problem, ProblemFile, ProjectApproval, Skill,
    shortened,
};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

/// What is loaded besides `Installed`: the commands, the skills, and the settings file with
/// what is wrong in it.
pub(super) struct Loaded<'a> {
    pub installed: &'a Installed,
    pub commands: &'a [Command],
    pub skills: &'a [Skill],
    pub settings: Option<&'a Path>,
    /// Every problem, the settings' first, as `problems` gives them.
    pub problems: &'a [Problem],
}

/// Rows under headings, with a note for each row and the file it shows, if any, which Enter
/// opens in the user's editor, or offers to remove when it is a link that leads nowhere.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Listing {
    pub rows: Vec<String>,
    pub headings: Vec<bool>,
    pub notes: Vec<String>,
    pub files: Vec<Option<PathBuf>>,
    /// The rows of project agents the user's approval covers, as they are or were, which Enter
    /// also offers to forget.
    pub approved: Vec<bool>,
}

/// A row's columns, its note and its file.
type Row = (Vec<String>, String, Option<PathBuf>);

impl Listing {
    /// `heading` over `rows`, the columns but the last as wide as the widest of the section;
    /// nothing without rows.
    fn section(&mut self, heading: &str, rows: Vec<Row>) {
        if rows.is_empty() {
            return;
        }
        self.rows.push(heading.to_owned());
        self.headings.push(true);
        self.notes.push(String::new());
        self.files.push(None);
        self.approved.push(false);
        let columns = rows.iter().map(|(columns, ..)| columns.len()).max();
        let widths: Vec<usize> = (0..columns.unwrap_or(0).saturating_sub(1))
            .map(|at| {
                let width = |(columns, ..): &Row| columns.get(at).map(|c| c.width());
                rows.iter().filter_map(width).max().unwrap_or(0)
            })
            .collect();
        for (columns, note, file) in rows {
            let mut row = String::from(" ");
            for (at, column) in columns.iter().enumerate() {
                row.push(' ');
                row.push_str(column);
                if let Some(width) = widths.get(at) {
                    row.push_str(&" ".repeat(width - column.width() + 1));
                }
            }
            self.rows.push(row.trim_end().to_owned());
            self.headings.push(false);
            self.notes.push(note);
            self.files.push(file);
            self.approved.push(false);
        }
    }

    /// The row showing `file` nearest `near`, as a file can have a problem's row and its own.
    pub(super) fn row_of(&self, file: &Path, near: usize) -> Option<usize> {
        (self.files.iter().enumerate())
            .filter(|(_, row)| row.as_deref() == Some(file))
            .map(|(at, _)| at)
            .min_by_key(|at| at.abs_diff(near))
    }
}

/// The problems, the command sets, commands, agents, skills, system prompt, context and settings
/// files.
pub(super) fn listing(loaded: &Loaded) -> Listing {
    let installed = loaded.installed;
    let mut listing = Listing::default();
    let problems = (loaded.problems.iter())
        .map(|problem| problem_row(problem, loaded.settings))
        .collect();
    listing.section("Needs attention", problems);
    let settings = loaded.settings.map(shown);
    let sets = (installed.sets.iter())
        .map(|set| {
            let commands = (loaded.commands.iter())
                .filter(|command| command.path.starts_with(&set.directory))
                .count();
            let agents = (installed.agents.iter())
                .filter(|agent| agent.source == AgentSource::Set(set.name.clone()))
                .count();
            let providers = match set.directory.join("models.json").is_file() {
                true => " Its models.json adds providers.",
                false => "",
            };
            let contents = format!(
                "{}, {}.{providers}",
                count(commands, "command"),
                count(agents, "agent")
            );
            let directory = shown(&set.directory);
            let found = match set.origin {
                Origin::Installed => match &settings {
                    Some(settings) => format!(
                        "Installed by reference, listed in {settings} (commandSets); `scramjet-rs \
                         remove {}` takes it out.",
                        set.name
                    ),
                    None => "Installed by reference.".to_owned(),
                },
                Origin::Global => "Found in Scramjet's directory of command sets.".to_owned(),
                Origin::Project => "Found in this project's .scramjet directory.".to_owned(),
            };
            let columns = vec![
                set.name.clone(),
                set.origin.name().to_owned(),
                directory.clone(),
            ];
            (columns, format!("{directory}\n{contents} {found}"), None)
        })
        .collect();
    listing.section("Command sets", sets);
    let commands = (loaded.commands.iter())
        .map(|command| {
            let delegated = match command.delegate_only {
                true => "Run by other commands, so not in the / list; typed in full, it runs.\n",
                false => "",
            };
            let note = format!(
                "{}\n{delegated}{}",
                shown(&command.path),
                command.description
            );
            let columns = vec![format!("/{}", command.name), command.description.clone()];
            (columns, note, Some(command.path.clone()))
        })
        .collect();
    listing.section("Commands", commands);
    let agents = (installed.agents.iter())
        .map(|agent| {
            let mut columns = vec![agent.name.clone()];
            let from = match &agent.source {
                AgentSource::Set(name) => {
                    columns.push(name.clone());
                    format!("From the command set {name}.")
                }
                AgentSource::User => {
                    columns.push("user".to_owned());
                    "In your agent directory.".to_owned()
                }
                AgentSource::Project(approval) => {
                    let enter = keymap::key(Binding::SelectConfirm);
                    let opens = |key: &String| format!("{key} opens it or forgets the approval");
                    let (state, from) = match approval {
                        ProjectApproval::Approved => (
                            "approved",
                            format!(
                                "In this project's .scramjet/agents. You approved running this \
                                 project's agents with this file as it is{}.",
                                enter.map_or_else(String::new, |key| format!("; {}", opens(&key)))
                            ),
                        ),
                        ProjectApproval::Changed => (
                            "changed since approved",
                            format!(
                                "In this project's .scramjet/agents, changed since you approved \
                                 running this project's agents: a subagent call naming one asks \
                                 again.{}",
                                enter.map_or_else(String::new, |key| format!(" {}.", opens(&key)))
                            ),
                        ),
                        ProjectApproval::NotApproved => (
                            "not approved",
                            "In this project's .scramjet/agents, not approved: a subagent call \
                             naming one of this project's agents asks before it runs."
                                .to_owned(),
                        ),
                    };
                    columns.extend(["project".to_owned(), state.to_owned()]);
                    from
                }
            };
            let note = format!("{}\n{from} {}", shown(&agent.path), agent.description);
            (columns, note, Some(agent.path.clone()))
        })
        .collect();
    let first = listing.rows.len() + 1;
    listing.section("Agents", agents);
    for (at, agent) in installed.agents.iter().enumerate() {
        if let AgentSource::Project(ProjectApproval::Approved | ProjectApproval::Changed) =
            agent.source
        {
            listing.approved[first + at] = true;
        }
    }
    let skills = (loaded.skills.iter())
        .map(|skill| {
            let offered = match skill.listed {
                true => "Offered to the model",
                false => "Not offered to the model (disable-model-invocation)",
            };
            let path = shown(&skill.path);
            let note = format!(
                "{path}\n{offered}; /skill:{} runs it. {}",
                skill.name, skill.description
            );
            (
                vec![skill.name.clone(), path],
                note,
                Some(skill.path.clone()),
            )
        })
        .collect();
    listing.section("Skills", skills);
    let prompt = (installed.prompt_files.iter())
        .map(|(path, opening)| {
            let (what, note) = match opening {
                true => (
                    "replaces the opening",
                    "In the instructions in place of their opening (the tools and guidelines), as \
                     Scramjet's SYSTEM.md or --system-prompt; /reload reads it again.",
                ),
                false => (
                    "after the opening",
                    "In the instructions after their opening, as Scramjet's APPEND_SYSTEM.md or \
                     --append-system-prompt; /reload reads it again.",
                ),
            };
            let columns = vec![shown(path), what.to_owned()];
            (
                columns,
                format!("{}\n{note}", shown(path)),
                Some(path.clone()),
            )
        })
        .collect();
    listing.section("System prompt", prompt);
    let context = (installed.context_files.iter())
        .map(|path| {
            let note = "In the instructions as the project's context, outermost first; /reload \
                        reads them again.";
            (vec![shown(path)], note.to_owned(), Some(path.clone()))
        })
        .collect();
    listing.section("Context files", context);
    let (project, global) = &installed.autonomy;
    let keybindings = loaded.settings.map(super::keymap::path);
    let mut files = Vec::new();
    if let Some(path) = loaded.settings {
        let what = "scramjet-rs's settings: those in this list, saved as you change them.";
        files.push((path, "settings", what, "a setting"));
    }
    if let Some(path) = &keybindings {
        let what = "scramjet-rs's keys, in Scramjet's format: those /hotkeys changes.";
        files.push((path, "keys", what, "a key in /hotkeys"));
    }
    if let Some(path) = global {
        let what = "Scramjet's, which it reads too: what follows each command, and whether its \
                    publications ask first, in all projects.";
        let label = "next steps and publishing, all projects";
        files.push((
            path,
            label,
            what,
            "a next step or publishing for all projects",
        ));
    }
    let what = "Scramjet's, which it reads too: what follows each command in this project.";
    files.push((
        project,
        "next steps, this project",
        what,
        "a next step for this project",
    ));
    let files = (files.into_iter())
        .map(|(path, label, what, change)| {
            let (missing, file) = match path.exists() {
                true => (String::new(), Some(path.to_path_buf())),
                false => (
                    format!(" Not there yet: changing {change} writes it."),
                    None,
                ),
            };
            let note = format!("{}\n{what}{missing}", shown(path));
            (vec![shown(path), label.to_owned()], note, file)
        })
        .collect();
    listing.section("Settings files", files);
    listing
}

/// A problem's row, with its file when Enter can do something with it.
fn problem_row(problem: &Problem, settings: Option<&Path>) -> Row {
    let text = home_shortened(&problem.text);
    let file = match &problem.file {
        Some(ProblemFile::Path(path)) => Some(path.as_path()),
        Some(ProblemFile::Settings) => settings,
        None => None,
    };
    let enter = keymap::key(Binding::SelectConfirm);
    let action = file.and_then(|file| match (broken_link(file), enter) {
        (Some(_), Some(key)) => Some(format!(
            "A link that leads nowhere: {key} removes it once you confirm."
        )),
        (Some(_), None) => Some("A link that leads nowhere.".to_owned()),
        (None, Some(key)) if file.is_file() => Some(format!("{key} opens {}.", shown(file))),
        (None, _) => None,
    });
    match action {
        Some(action) => {
            let file = file.map(Path::to_path_buf);
            (vec![text.clone()], format!("{text}\n{action}"), file)
        }
        None => (vec![text.clone()], text, None),
    }
}

/// Where `path` leads when it is a link to nothing.
pub(super) fn broken_link(path: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(path).ok()?;
    let missing = std::fs::metadata(path).err()?.kind() == std::io::ErrorKind::NotFound;
    missing.then_some(target)
}

/// `/settings`' row: how many problems, sets, commands, agents and skills.
pub(super) fn summary(loaded: &Loaded) -> String {
    let parts: Vec<String> = [
        (loaded.installed.sets.len(), "command set"),
        (loaded.commands.len(), "command"),
        (loaded.installed.agents.len(), "agent"),
        (loaded.skills.len(), "skill"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| count(n, what))
    .collect();
    let contents = match parts.is_empty() {
        true => "no command sets, agents or skills".to_owned(),
        false => parts.join(", "),
    };
    match loaded.problems.len() {
        0 => contents,
        n => format!("{} · {contents}", count(n, "problem")),
    }
}

/// What the full screen says about `problems` at start and after `/reload`: the problem when
/// there is one, else how many there are and where they are listed.
pub(super) fn attention(problems: &[Problem]) -> Option<String> {
    match problems {
        [] => None,
        [problem] => Some(problem.text.clone()),
        problems => Some(format!(
            "{} problems in what was loaded: see Installed in /settings",
            problems.len()
        )),
    }
}

pub(super) fn count(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}

/// `text` with the home directory shown as `~` wherever a path in it starts with it.
fn home_shortened(text: &str) -> String {
    match std::env::home_dir().map(|home| format!("{}/", home.display())) {
        Some(home) if home != "/" => text.replace(&home, "~/"),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet_rs::agent::{AgentFile, Commands, Sets, Skills};
    use std::{fs, path::PathBuf};

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn lists_what_is_loaded_with_its_source_under_headings_problems_first() {
        let base = std::env::temp_dir().join(format!("scramjet-installed-{}", std::process::id()));
        let _scratch = Scratch(base.clone());
        let set = base.join("sets/demo");
        write(
            &set.join("commands/demo:greet.md"),
            "---\ndescription: Greet someone\n---\nHi $ARGUMENTS\n",
        );
        write(
            &set.join("commands/demo:inner.md"),
            "---\ndescription: Help\ndelegate-only: true\n---\nHelp\n",
        );
        write(&set.join("agents/demo:helper.md"), "");
        let root = base.join("project");
        write(
            &root.join(".scramjet/skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: Fills PDF forms.\n---\nUse ./fill.sh\n",
        );
        let settings = base.join("config/settings.json");
        write(&settings, "{}");
        let sets = Sets::find(std::slice::from_ref(&set), None, &root.join(".scramjet"));
        let commands = Commands::discover(&sets);
        let skills = Skills::discover(&[], &Default::default(), &root);
        let agent = |name: &str, path: PathBuf, source| AgentFile {
            name: name.to_owned(),
            description: format!("Does {name}."),
            path,
            source,
        };
        let found = Installed {
            sets: sets.list().to_vec(),
            agents: vec![
                agent("mine", base.join("user/mine.md"), AgentSource::User),
                agent(
                    "demo:helper",
                    set.join("agents/demo:helper.md"),
                    AgentSource::Set("demo".into()),
                ),
            ],
            context_files: vec![root.join("AGENTS.md")],
            prompt_files: vec![
                (root.join(".scramjet/SYSTEM.md"), true),
                (base.join("more.md"), false),
            ],
            autonomy: (
                root.join(".scramjet/autonomy.yaml"),
                Some(base.join("config/autonomy.yaml")),
            ),
            problems: Vec::new(),
        };
        let problems = [
            Problem::in_settings("settings: x: wrong"),
            Problem::new("skills: y"),
        ];
        let loaded = Loaded {
            installed: &found,
            commands: commands.list(),
            skills: skills.list(),
            settings: Some(&settings),
            problems: &problems,
        };
        let shown = |path: &Path| path.display().to_string();
        let list = listing(&loaded);
        let project_file = shown(&root.join(".scramjet/autonomy.yaml"));
        let width = project_file.len();
        assert_eq!(
            list.rows,
            [
                "Needs attention".to_owned(),
                "  settings: x: wrong".into(),
                "  skills: y".into(),
                "Command sets".into(),
                format!("  demo  installed  {}", shown(&set)),
                "Commands".into(),
                "  /demo:greet  Greet someone".into(),
                "  /demo:inner  Help".into(),
                "Agents".into(),
                "  mine         user".into(),
                "  demo:helper  demo".into(),
                "Skills".into(),
                format!(
                    "  pdf  {}",
                    shown(&root.join(".scramjet/skills/pdf/SKILL.md"))
                ),
                "System prompt".into(),
                format!(
                    "  {}  replaces the opening",
                    shown(&root.join(".scramjet/SYSTEM.md"))
                ),
                format!(
                    "  {:w$}  after the opening",
                    shown(&base.join("more.md")),
                    w = shown(&root.join(".scramjet/SYSTEM.md")).len()
                ),
                "Context files".into(),
                format!("  {}", shown(&root.join("AGENTS.md"))),
                "Settings files".into(),
                format!("  {:width$}  settings", shown(&settings)),
                format!(
                    "  {:width$}  keys",
                    shown(&base.join("config/keybindings.json"))
                ),
                format!(
                    "  {:width$}  next steps and publishing, all projects",
                    shown(&base.join("config/autonomy.yaml"))
                ),
                format!("  {project_file}  next steps, this project"),
            ]
        );
        let headings: Vec<usize> = (0..list.rows.len())
            .filter(|&at| list.headings[at])
            .collect();
        assert_eq!(headings, [0, 3, 5, 8, 11, 13, 16, 18]);
        assert_eq!(list.notes.len(), list.rows.len());
        assert_eq!(list.files.len(), list.rows.len());
        // Enter opens the files of problems, commands, agents, skills, system prompt, context
        // and settings files that exist; not those of headings, problems without one, sets or files not
        // written yet.
        let files: Vec<(usize, &Path)> = (list.files.iter().enumerate())
            .filter_map(|(at, file)| Some((at, file.as_deref()?)))
            .collect();
        assert_eq!(
            files,
            [
                (1, settings.as_path()),
                (6, &set.join("commands/demo:greet.md")),
                (7, &set.join("commands/demo:inner.md")),
                (9, &base.join("user/mine.md")),
                (10, &set.join("agents/demo:helper.md")),
                (12, &root.join(".scramjet/skills/pdf/SKILL.md")),
                (14, &root.join(".scramjet/SYSTEM.md")),
                (15, &base.join("more.md")),
                (17, &root.join("AGENTS.md")),
                (19, &settings),
            ]
        );
        assert!(list.notes[15].ends_with(
            "\nIn the instructions after their opening, as \
            Scramjet's APPEND_SYSTEM.md or --append-system-prompt; /reload reads it again."
        ));
        // After an edit, the list comes back on the row Enter was pressed on, not the
        // problem's row of the same file.
        let rows = [19, 1, 11, 3].map(|near| list.row_of(&settings, near));
        assert_eq!(rows, [Some(19), Some(1), Some(19), Some(1)]);
        assert_eq!(list.row_of(&base.join("gone.md"), 4), None);
        let opens = format!("settings: x: wrong\nEnter opens {}.", shown(&settings));
        assert_eq!((&list.notes[1], &list.notes[2][..]), (&opens, "skills: y"));
        assert_eq!(
            list.notes[4],
            format!(
                "{}\n2 commands, 1 agent. Installed by reference, listed in {} (commandSets); \
                 `scramjet-rs remove demo` takes it out.",
                shown(&set),
                shown(&settings)
            )
        );
        assert_eq!(
            list.notes[7],
            format!(
                "{}\nRun by other commands, so not in the / list; typed in full, it runs.\nHelp",
                shown(&set.join("commands/demo:inner.md"))
            )
        );
        assert!(list.notes[9].ends_with("\nIn your agent directory. Does mine."));
        assert!(list.notes[10].ends_with("\nFrom the command set demo. Does demo:helper."));
        assert!(
            list.notes[12]
                .ends_with("\nOffered to the model; /skill:pdf runs it. Fills PDF forms.")
        );
        // The settings file is there; the autonomy.yaml files are not yet.
        assert!(list.notes[19].ends_with("saved as you change them."));
        assert!(list.notes[20].ends_with(
            "those /hotkeys changes. Not there yet: changing a key in /hotkeys writes it."
        ));
        assert!(list.notes[21].ends_with(
            " Not there yet: changing a next step or publishing for all projects writes it."
        ));
        assert!(
            list.notes[22]
                .ends_with(" Not there yet: changing a next step for this project writes it.")
        );
        assert_eq!(
            summary(&loaded),
            "2 problems · 1 command set, 2 commands, 2 agents, 1 skill"
        );
        let nothing = Installed {
            sets: Vec::new(),
            agents: Vec::new(),
            context_files: Vec::new(),
            prompt_files: Vec::new(),
            autonomy: (root.join(".scramjet/autonomy.yaml"), None),
            problems: Vec::new(),
        };
        let empty = Loaded {
            installed: &nothing,
            commands: &[],
            skills: &[],
            settings: None,
            problems: &[],
        };
        assert_eq!(summary(&empty), "no command sets, agents or skills");
        assert_eq!(listing(&empty).rows.len(), 2);
    }

    #[test]
    fn project_agents_say_whether_the_approval_covers_them_and_offer_to_forget_it() {
        let directory = PathBuf::from("/repo/.scramjet/agents");
        let agent = |name: &str, approval| AgentFile {
            name: name.to_owned(),
            description: format!("Does {name}."),
            path: directory.join(format!("{name}.md")),
            source: AgentSource::Project(approval),
        };
        let found = Installed {
            sets: Vec::new(),
            agents: vec![
                agent("kept", ProjectApproval::Approved),
                agent("edited", ProjectApproval::Changed),
                agent("added", ProjectApproval::NotApproved),
            ],
            context_files: Vec::new(),
            prompt_files: Vec::new(),
            autonomy: (PathBuf::from("/repo/.scramjet/autonomy.yaml"), None),
            problems: Vec::new(),
        };
        let loaded = Loaded {
            installed: &found,
            commands: &[],
            skills: &[],
            settings: None,
            problems: &[],
        };
        let list = listing(&loaded);
        assert_eq!(
            list.rows[..4],
            [
                "Agents",
                "  kept    project  approved",
                "  edited  project  changed since approved",
                "  added   project  not approved",
            ]
        );
        assert_eq!(list.approved, [false, true, true, false, false, false]);
        assert_eq!(
            list.notes[1..4],
            [
                "/repo/.scramjet/agents/kept.md\nIn this project's .scramjet/agents. You approved \
                 running this project's agents with this file as it is; Enter opens it or \
                 forgets the approval. Does kept.",
                "/repo/.scramjet/agents/edited.md\nIn this project's .scramjet/agents, changed \
                 since you approved running this project's agents: a subagent call naming one \
                 asks again. Enter opens it or forgets the approval. Does edited.",
                "/repo/.scramjet/agents/added.md\nIn this project's .scramjet/agents, not \
                 approved: a subagent call naming one of this project's agents asks before it \
                 runs. Does added.",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_problem_opens_its_file_or_offers_to_remove_a_link_that_leads_nowhere() {
        let base = std::env::temp_dir().join(format!("scramjet-problems-{}", std::process::id()));
        let _scratch = Scratch(base.clone());
        let (file, link, gone) = (base.join("a.md"), base.join("b.md"), base.join("gone.md"));
        write(&file, "x");
        std::os::unix::fs::symlink(&gone, &link).unwrap();
        let working = base.join("c.md");
        std::os::unix::fs::symlink(&file, &working).unwrap();
        assert_eq!(broken_link(&link), Some(gone.clone()));
        for not_broken in [&file, &working, &gone] {
            assert_eq!(broken_link(not_broken), None, "{not_broken:?}");
        }
        let problems = [
            Problem::in_file("agents: a", &file),
            Problem::in_file("agents: b", &link),
            Problem::in_file("agents: gone", &gone),
            Problem::in_settings("skills: listed"),
            Problem::new("scramjet: directory"),
        ];
        let found = Installed {
            sets: Vec::new(),
            agents: Vec::new(),
            context_files: Vec::new(),
            prompt_files: Vec::new(),
            autonomy: (base.join(".scramjet/autonomy.yaml"), None),
            problems: Vec::new(),
        };
        let loaded = Loaded {
            installed: &found,
            commands: &[],
            skills: &[],
            settings: None,
            problems: &problems,
        };
        let list = listing(&loaded);
        let shown = |path: &Path| path.display().to_string();
        assert_eq!(
            (&list.files[1..6], &list.notes[1..6]),
            (
                &[Some(file.clone()), Some(link.clone()), None, None, None][..],
                &[
                    format!("agents: a\nEnter opens {}.", shown(&file)),
                    "agents: b\nA link that leads nowhere: Enter removes it once you confirm."
                        .to_owned(),
                    "agents: gone".to_owned(),
                    "skills: listed".to_owned(),
                    "scramjet: directory".to_owned(),
                ][..]
            )
        );
        // With a settings file, what it lists opens it.
        let settings = base.join("settings.json");
        write(&settings, "{}");
        let loaded = Loaded {
            settings: Some(&settings),
            ..loaded
        };
        assert_eq!(listing(&loaded).files[4], Some(settings));
    }

    #[test]
    fn launch_says_one_line_at_most() {
        assert_eq!(attention(&[]), None);
        assert_eq!(
            attention(&[Problem::new("skills: y")]).as_deref(),
            Some("skills: y")
        );
        let three = ["a", "b", "c"].map(Problem::new);
        assert_eq!(
            attention(&three).as_deref(),
            Some("3 problems in what was loaded: see Installed in /settings")
        );
    }
}
