//! `scramjet install`, `remove`, `update` and `list`: command sets installed by reference,
//! recorded in the settings' `commandSets` without being copied, as Pi installs a local path,
//! and packages from git, cloned and recorded in `packages` (`super::packages`); `list` also
//! shows the skills.
use super::packages::{self, Source};
use super::settings::{self, Installed, Settings};
use super::theme;
use scramjet::agent::{Inventory, Problem, Set, inventory, sets_in, shortened};
use std::path::{Path, PathBuf};

pub(super) const USAGE: &str =
    "Usage: scramjet install DIR|REPO | remove NAME|DIR|REPO | update [REPO|NAME|self] [--force] | list
REPO: git:github.com/USER/REPO[@REF], git:git@github.com:USER/REPO or a git URL";

/// Runs `command` with its `arguments` when it is one of these; `None` when it is not.
pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    let result = match (command, arguments) {
        ("install", [source]) => match Source::parse(source) {
            None => install(Path::new(source)),
            Some(source) => source.and_then(install_package),
        },
        ("remove" | "uninstall", [reference]) => remove(reference),
        ("update", arguments) => match update_target(arguments) {
            Some((packages, program, force)) => {
                let packages = packages.map(|reference| update(reference.as_deref()));
                let program = program.then(|| super::update::update_self(force));
                (packages.unwrap_or(Ok(()))).and(program.unwrap_or(Ok(())))
            }
            None => Err(USAGE.to_owned()),
        },
        ("list", []) => list(),
        ("install" | "remove" | "uninstall" | "list", _) => Err(USAGE.to_owned()),
        _ => return None,
    };
    Some(result)
}

/// What `update`'s `arguments` update, as Scramjet 1's: the packages (all, or the one named),
/// the program, and whether to replace it even when no release is newer (`--force`). `None`
/// when they make no sense together.
fn update_target(arguments: &[String]) -> Option<(Option<Option<String>>, bool, bool)> {
    let (mut named, mut one, mut program, mut packages, mut force) =
        (None, None, false, false, false);
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--self" => program = true,
            "--extensions" => packages = true,
            "--force" => force = true,
            "--extension" if one.is_none() => one = Some(arguments.next()?.clone()),
            option if option.starts_with('-') => return None,
            _ if named.is_some() => return None,
            reference => named = Some(reference.to_owned()),
        }
    }
    match (one, named.as_deref()) {
        (None, Some("self")) => Some((packages.then_some(None), true, force)),
        (Some(_), Some(_)) => None,
        (Some(_), None) | (None, Some(_)) if program || packages => None,
        (Some(one), None) => Some((Some(Some(one)), false, force)),
        (None, Some(named)) => Some((Some(Some(named.to_owned())), false, force)),
        (None, None) if program || packages => Some((packages.then_some(None), program, force)),
        (None, None) => Some((Some(None), true, force)),
    }
}

fn install(directory: &Path) -> Result<(), String> {
    let mut settings = Settings::load();
    let installed = settings.install_directory(directory)?;
    let found = here(&settings)?;
    let concerned = match installed {
        Installed::Set(set) => {
            outln!(
                "Installed {} from {}: {}",
                set.name,
                shown(&set.directory),
                contents(&found, &set)
            );
            set.directory
        }
        Installed::Package {
            directory,
            sets,
            moved,
            skills,
            themes,
        } => {
            outln!("Installed the package in {}:", shown(&directory));
            for set in &sets {
                let path = set
                    .directory
                    .strip_prefix(&directory)
                    .unwrap_or(&set.directory);
                let (name, path) = (&set.name, path.display());
                outln!("  {name} ({path}): {}", contents(&found, set));
            }
            print_extras(&found, skills, themes);
            if !moved.is_empty() {
                outln!(
                    "It now brings {}, taken out of commandSets.",
                    moved.join(", ")
                );
            }
            directory
        }
    };
    let concerned = concerned.display().to_string();
    let concerned: Vec<&Problem> = (found.warnings.iter())
        .chain(found.skills.warnings())
        .filter(|warning| warning.text.contains(&concerned))
        .collect();
    print_warnings(&concerned);
    Ok(())
}

/// What a package's `skills` and `themes` directories bring.
fn print_extras(found: &Inventory, skills: Option<PathBuf>, themes: Option<PathBuf>) {
    if let Some(skills) = skills {
        let names: Vec<&str> = (found.skills.list().iter())
            .filter(|skill| skill.path.starts_with(&skills))
            .map(|skill| skill.name.as_str())
            .collect();
        outln!("  skills: {}", listing(&names, "skill"));
    }
    if let Some(themes) = themes {
        let names = theme::names_in(&themes);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        outln!("  themes: {}", listing(&names, "theme"));
    }
}

/// `names` counted, then listed: `2 skills (a, b)`.
fn listing(names: &[&str], what: &str) -> String {
    match names {
        [] => format!("no {what}s"),
        [name] => format!("1 {what} ({name})"),
        names => format!("{} {what}s ({})", names.len(), names.join(", ")),
    }
}

/// Clones `source` and adds it to the settings' `packages`, with what its sets bring and the
/// programs it runs. A clone left in its place is used, as Scramjet uses it.
fn install_package(source: Source) -> Result<(), String> {
    let mut settings = Settings::load();
    let root = settings.clones()?.to_path_buf();
    if let Some(known) = settings.package(&source.written) {
        return Err(match known.written == source.written {
            true => format!("{} is already installed", source.written),
            false => format!(
                "{} is installed as {}; remove it first",
                source.identity(),
                known.written
            ),
        });
    }
    let directory = source.clone_directory(&root);
    let nested = (settings.packages.iter()).find(|known| {
        let other = known.clone_directory(&root);
        other.starts_with(&directory) || directory.starts_with(&other)
    });
    if let Some(other) = nested {
        return Err(format!(
            "{} would be cloned into {}, which nests with the clone of {}; remove that first",
            source.written,
            shown(&directory),
            other.written
        ));
    }
    let fresh = !directory.exists();
    if !fresh && !packages::is_clone(&directory) {
        return Err(format!(
            "{} exists and is not a clone; left as it is",
            shown(&directory)
        ));
    }
    match (fresh, &source.reference) {
        (true, _) => {
            outln!("Cloning {} into {}", source.written, shown(&directory));
            packages::clone(&source, &directory, &root)?
        }
        (false, Some(reference)) => packages::checkout(&directory, reference)?,
        (false, None) => {}
    }
    let undo = |error: String| {
        if fresh && let Err(problem) = packages::delete(&directory, &root) {
            eprintln!("{problem}");
        }
        error
    };
    let sets = sets_in(&directory);
    let (skills, themes) = settings::extras(&directory);
    if sets.is_empty() && skills.is_none() && themes.is_none() {
        return Err(undo(format!(
            "{} holds no command set (no commands, agents or models.json at its top or in its \
             top directories, and no set.yaml deeper) and no skills or themes directory",
            source.written
        )));
    }
    for set in &sets {
        let name = set.file_name();
        if let Some(known) = (settings.command_sets.iter()).find(|known| known.file_name() == name)
        {
            return Err(undo(format!(
                "{} brings a command set named {}, which is installed from {}; remove it first",
                source.written,
                name.unwrap_or_default().display(),
                shown(known)
            )));
        }
    }
    settings
        .add_package(source.clone(), sets.clone())
        .map_err(undo)?;
    let found = here(&settings)?;
    let reused = match fresh {
        true => "",
        false => " (the clone already there)",
    };
    outln!(
        "Installed {} from {}{reused}:",
        source.written,
        shown(&directory)
    );
    for set in found.sets.list() {
        if sets.contains(&set.directory) {
            let path = set
                .directory
                .strip_prefix(&directory)
                .unwrap_or(&set.directory);
            let path = match path.as_os_str().is_empty() {
                true => String::new(),
                false => format!(" ({})", path.display()),
            };
            outln!("  {}{path}: {}", set.name, contents(&found, set));
        }
    }
    print_extras(&found, skills, themes);
    let (downloaded, failed) = super::programs::fetch(&directory);
    for line in downloaded {
        outln!("{line}");
    }
    for why in failed {
        outln!(
            "Not downloaded: {why}; `scramjet update {}` tries again",
            source.written
        );
    }
    let programs: Vec<String> = sets
        .iter()
        .filter_map(|set| program(set, &directory))
        .collect();
    match programs.is_empty() {
        true => outln!("It runs no program of its own."),
        false => {
            outln!("It runs at each start:");
            for program in programs {
                outln!("  {program}");
            }
        }
    }
    let concerned: Vec<&Problem> = (found.warnings.iter())
        .filter(|warning| warning.text.contains(&directory.display().to_string()))
        .collect();
    print_warnings(&concerned);
    Ok(())
}

/// The program the `models.json` in `set` runs (`providersCommand`), as the user would read it.
fn program(set: &Path, package: &Path) -> Option<String> {
    let file = set.join("models.json");
    let text = std::fs::read(&file).ok()?;
    let document: serde_json::Value = serde_json::from_slice(&text).ok()?;
    let words = document.get("providersCommand")?.as_array()?;
    let words: Vec<&str> = words.iter().filter_map(|word| word.as_str()).collect();
    let file = file.strip_prefix(package).unwrap_or(&file);
    Some(format!(
        "{} (providersCommand in {}, run in its directory)",
        words.join(" "),
        file.display()
    ))
}

/// Removes the set named `reference` or installed from that directory, or the package from
/// that repository with its clone. A set that came with a package goes only with the package.
fn remove(reference: &str) -> Result<(), String> {
    let mut settings = Settings::load();
    if let Some(source) = Source::parse(reference) {
        let source = source?;
        let Some(known) = settings.package(&source.written).cloned() else {
            return Err(format!(
                "no package from {} is installed",
                source.identity()
            ));
        };
        let root = settings.clones()?.to_path_buf();
        let directory = known.clone_directory(&root);
        let held = (settings.packages.iter()).find(|other| {
            other.identity() != known.identity()
                && other.clone_directory(&root).starts_with(&directory)
        });
        if let Some(held) = held {
            return Err(format!(
                "{}'s clone in {} holds the clone of {}; remove that first",
                known.written,
                shown(&directory),
                held.written
            ));
        }
        let directory = settings.remove_package(&known)?;
        packages::delete(&directory, &root).map_err(|problem| {
            format!(
                "{} is removed from the settings, but its clone is left: {problem}",
                known.written
            )
        })?;
        outln!(
            "Removed {} and its clone in {}",
            known.written,
            shown(&directory)
        );
        return Ok(());
    }
    if let Some(directory) = settings.remove_local_package(reference)? {
        outln!("Removed the package in {}", shown(&directory));
        return Ok(());
    }
    match settings.remove_set(reference) {
        Ok(directories) => {
            for directory in directories {
                let name = directory.file_name().unwrap_or_default().to_string_lossy();
                outln!("Removed {name}, installed from {}", shown(&directory));
            }
            Ok(())
        }
        Err(error) => {
            let local = (settings.command_sets.iter())
                .find(|set| set.file_name().is_some_and(|name| name == reference))
                .and_then(|set| settings.local_package(set));
            let package = match (settings.package(reference), local) {
                (Some(package), _) => package.written.clone(),
                (None, Some(directory)) => shown(directory),
                (None, None) => return Err(error),
            };
            Err(format!(
                "{reference} came with {package}; `scramjet remove {package}` removes it with \
                 the package's other command sets"
            ))
        }
    }
}

/// Pulls each package not pinned to a ref, or the one `reference` names (its repository or a
/// command set it brings), and clones those missing.
fn update(reference: Option<&str>) -> Result<(), String> {
    let settings = Settings::load();
    for warning in &settings.warnings {
        eprintln!("settings: {warning}");
    }
    let root = settings.clones()?.to_path_buf();
    let chosen = match reference {
        None => settings.packages.clone(),
        Some(reference) => match (settings.package(reference), Source::parse(reference)) {
            (_, Some(Err(why))) => return Err(why),
            (Some(package), _) => vec![package.clone()],
            (None, _) => {
                return Err(format!(
                    "no package is installed from {reference} or brings a command set of that name"
                ));
            }
        },
    };
    if chosen.is_empty() {
        outln!("No packages. `scramjet install git:github.com/USER/REPO` installs one.");
        return Ok(());
    }
    let (mut failed, mut changed) = (0, Vec::new());
    for source in &chosen {
        let directory = source.clone_directory(&root);
        let written = &source.written;
        let result = match (directory.exists(), &source.reference) {
            (false, _) => packages::clone(source, &directory, &root)
                .map(|()| (format!("Cloned {written} into {}", shown(&directory)), true)),
            (true, Some(reference)) => Ok((
                format!("{written}: pinned to {reference}; left as it is"),
                false,
            )),
            (true, None) => {
                packages::pull(&directory).map(|(before, after)| match before == after {
                    true => (format!("{written}: up to date ({after})"), false),
                    false => (format!("Updated {written}: {before}..{after}"), true),
                })
            }
        };
        let result = result.and_then(|(line, updated)| {
            let (downloaded, failed) = super::programs::fetch(&directory);
            let updated = updated || !downloaded.is_empty();
            let lines = std::iter::once(line).chain(downloaded).collect::<Vec<_>>();
            match failed.is_empty() {
                true => Ok((lines.join("\n"), updated)),
                false => {
                    outln!("{}", lines.join("\n"));
                    Err(format!("not downloaded: {}", failed.join("; ")))
                }
            }
        });
        match result {
            Ok((line, updated)) => {
                outln!("{line}");
                if updated {
                    changed.push(directory.display().to_string());
                }
            }
            Err(problem) => {
                eprintln!("{written}: {problem}");
                failed += 1;
            }
        }
    }
    if !changed.is_empty() {
        let settings = Settings::load();
        let found = here(&settings)?;
        let warnings = settings.warnings.iter().map(String::as_str);
        let warnings = warnings.chain(found.warnings.iter().map(|w| w.text.as_str()));
        let concerned = warnings.filter(|text| changed.iter().any(|clone| text.contains(clone)));
        print_texts(concerned);
    }
    match failed {
        0 => Ok(()),
        _ => Err(format!("{failed} of {} packages not updated", chosen.len())),
    }
}

/// The sets and skills a session in the current directory loads, in their order, and what is
/// wrong.
fn list() -> Result<(), String> {
    let settings = Settings::load();
    for warning in &settings.warnings {
        eprintln!("settings: {warning}");
    }
    let found = here(&settings)?;
    let sets = found.sets.list();
    if sets.is_empty() {
        outln!("No command sets. `scramjet install DIR|REPO` installs one.");
    }
    let rows: Vec<[String; 4]> = (sets.iter())
        .map(|set| {
            [
                set.name.clone(),
                origin(&settings, set),
                shown(&set.directory),
                contents(&found, set),
            ]
        })
        .collect();
    let width = |column: usize| rows.iter().map(|row| row[column].chars().count()).max();
    let widths = [0, 1, 2].map(|column| width(column).unwrap_or(0));
    for [name, origin, directory, contents] in &rows {
        outln!(
            "{name:<0$}  {origin:<1$}  {directory:<2$}  {contents}",
            widths[0],
            widths[1],
            widths[2]
        );
    }
    if let Some(path) = settings.shown_path() {
        outln!("Installed sets are listed in {path} (commandSets, packages).");
    }
    let skills = found.skills.list();
    if !skills.is_empty() {
        outln!("\nSkills:");
        let width = (skills.iter())
            .map(|skill| skill.name.chars().count())
            .max();
        for skill in skills {
            let only_typed = match skill.listed {
                true => "",
                false => " (not offered to the model)",
            };
            let (name, path) = (&skill.name, shown(&skill.path));
            outln!("  {name:<0$}  {path}{only_typed}", width.unwrap_or(0));
        }
    }
    let warnings = found.warnings.iter().chain(found.skills.warnings());
    print_warnings(&warnings.collect::<Vec<_>>());
    Ok(())
}

/// Where `set` comes from: the package that brought it, else its origin.
fn origin(settings: &Settings, set: &Set) -> String {
    let root = settings.clones().ok();
    let package = root.and_then(|root| {
        (settings.packages.iter())
            .find(|package| set.directory.starts_with(package.clone_directory(root)))
    });
    match (package, settings.local_package(&set.directory)) {
        (Some(package), _) => package.written.clone(),
        (None, Some(directory)) => shown(directory),
        (None, None) => set.origin.name().to_owned(),
    }
}

/// What a session in the current directory would load.
fn here(settings: &Settings) -> Result<Inventory, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(inventory(&settings.command_sets, &settings.skills, &root))
}

/// How many of `found`'s commands, agents and skills come from `set`.
fn contents(found: &Inventory, set: &Set) -> String {
    let commands = (found.commands.list().iter())
        .filter(|command| command.path.starts_with(&set.directory))
        .count();
    let agents = (found.agents.iter())
        .filter(|(_, path)| path.starts_with(&set.directory))
        .count();
    let count = |n: usize, what: &str| match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    };
    let skills = match (found.skills.list().iter())
        .filter(|skill| skill.path.starts_with(&set.directory))
        .count()
    {
        0 => String::new(),
        n => format!(", {}", count(n, "skill")),
    };
    let providers = match set.directory.join("models.json").is_file() {
        true => ", and providers (models.json)",
        false => "",
    };
    format!(
        "{}, {}{skills}{providers}",
        count(commands, "command"),
        count(agents, "agent")
    )
}

fn print_warnings(warnings: &[&Problem]) {
    print_texts(warnings.iter().map(|warning| warning.text.as_str()));
}

/// `texts` under a heading, with the home directory in them as `~`.
fn print_texts<'a>(texts: impl IntoIterator<Item = &'a str>) {
    let home = scramjet::os::home_prefix();
    let tilde = format!("~{}", std::path::MAIN_SEPARATOR);
    for (number, text) in texts.into_iter().enumerate() {
        if number == 0 {
            outln!("\nWarnings:");
        }
        match &home {
            Some(home) => outln!("  {}", text.replace(home, &tilde)),
            None => outln!("  {text}"),
        }
    }
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::update_target;

    #[test]
    fn update_takes_scramjet_1s_options() {
        let target = |arguments: &[&str]| {
            let arguments: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
            update_target(&arguments)
        };
        let one = |name: &str| Some(Some(name.to_owned()));
        assert_eq!(target(&[]), Some((Some(None), true, false)));
        assert_eq!(target(&["self"]), Some((None, true, false)));
        assert_eq!(target(&["--self"]), Some((None, true, false)));
        assert_eq!(
            target(&["self", "--self", "--force"]),
            Some((None, true, true))
        );
        assert_eq!(target(&["--extensions"]), Some((Some(None), false, false)));
        assert_eq!(
            target(&["--self", "--extensions"]),
            Some((Some(None), true, false))
        );
        assert_eq!(
            target(&["self", "--extensions"]),
            Some((Some(None), true, false))
        );
        assert_eq!(target(&["mach12"]), Some((one("mach12"), false, false)));
        assert_eq!(
            target(&["--extension", "mach12"]),
            Some((one("mach12"), false, false))
        );
        for refused in [
            &["a", "b"][..],
            &["mach12", "--self"],
            &["mach12", "--extensions"],
            &["--extension", "a", "b"],
            &["--extension", "a", "--extensions"],
            &["--extension", "a", "--extension", "b"],
            &["--extension"],
            &["--local"],
        ] {
            assert_eq!(target(refused), None, "{refused:?}");
        }
    }
}
