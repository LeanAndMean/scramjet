//! `scramjet-rs install`, `remove`, `update` and `list`: command sets installed by reference,
//! recorded in the settings' `commandSets` without being copied, as Pi installs a local path,
//! and packages from git, cloned and recorded in `packages` (`super::packages`); `list` also
//! shows the skills.
use super::packages::{self, Source};
use super::settings::Settings;
use scramjet_rs::agent::{Inventory, Problem, Set, inventory, sets_in, shortened};
use std::path::Path;

pub(super) const USAGE: &str =
    "Usage: scramjet-rs install DIR|REPO | remove NAME|DIR|REPO | update [REPO|NAME] | list
REPO: git:github.com/USER/REPO[@REF], git:git@github.com:USER/REPO or a git URL";

/// Runs `command` with its `arguments` when it is one of these; `None` when it is not.
pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    let result = match (command, arguments) {
        ("install", [source]) => match Source::parse(source) {
            None => install(Path::new(source)),
            Some(source) => source.and_then(install_package),
        },
        ("remove" | "uninstall", [reference]) => remove(reference),
        ("update", []) => update(None),
        ("update", [reference]) => update(Some(reference)),
        ("list", []) => list(),
        ("install" | "remove" | "uninstall" | "update" | "list", _) => Err(USAGE.to_owned()),
        _ => return None,
    };
    Some(result)
}

fn install(directory: &Path) -> Result<(), String> {
    let mut settings = Settings::load();
    let sets = settings.install_sets(directory)?;
    let found = here(&settings)?;
    let given = std::path::absolute(directory).map_err(|e| e.to_string())?;
    match sets.as_slice() {
        [set] if set.directory == given => println!(
            "Installed {} from {}: {}",
            set.name,
            shown(&set.directory),
            contents(&found, set)
        ),
        sets => {
            println!("Installed from {}:", shown(&given));
            for set in sets {
                let path = set.directory.strip_prefix(&given).unwrap_or(&set.directory);
                let (name, path) = (&set.name, path.display());
                println!("  {name} ({path}): {}", contents(&found, set));
            }
        }
    }
    let directories: Vec<String> = (sets.iter())
        .map(|set| set.directory.display().to_string())
        .collect();
    let concerned: Vec<&Problem> = (found.warnings.iter())
        .filter(|warning| directories.iter().any(|d| warning.text.contains(d)))
        .collect();
    print_warnings(&concerned);
    Ok(())
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
            println!("Cloning {} into {}", source.written, shown(&directory));
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
    if sets.is_empty() {
        return Err(undo(format!(
            "{} holds no command set: no commands, agents or models.json at its top or in its \
             top directories, and no set.yaml deeper",
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
    println!(
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
            println!("  {}{path}: {}", set.name, contents(&found, set));
        }
    }
    let programs: Vec<String> = sets
        .iter()
        .filter_map(|set| program(set, &directory))
        .collect();
    match programs.is_empty() {
        true => println!("It runs no program of its own."),
        false => {
            println!("It runs at each start:");
            for program in programs {
                println!("  {program}");
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
        println!(
            "Removed {} and its clone in {}",
            known.written,
            shown(&directory)
        );
        return Ok(());
    }
    match settings.remove_set(reference) {
        Ok(directories) => {
            for directory in directories {
                let name = directory.file_name().unwrap_or_default().to_string_lossy();
                println!("Removed {name}, installed from {}", shown(&directory));
            }
            Ok(())
        }
        Err(error) => match settings.package(reference) {
            Some(package) => Err(format!(
                "{reference} came with {0}; `scramjet-rs remove {0}` removes it with the \
                 package's other command sets",
                package.written
            )),
            None => Err(error),
        },
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
        println!("No packages. `scramjet-rs install git:github.com/USER/REPO` installs one.");
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
        match result {
            Ok((line, updated)) => {
                println!("{line}");
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
        println!("No command sets. `scramjet-rs install DIR|REPO` installs one.");
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
        println!(
            "{name:<0$}  {origin:<1$}  {directory:<2$}  {contents}",
            widths[0], widths[1], widths[2]
        );
    }
    if let Some(path) = settings.shown_path() {
        println!("Installed sets are listed in {path} (commandSets, packages).");
    }
    let skills = found.skills.list();
    if !skills.is_empty() {
        println!("\nSkills:");
        let width = (skills.iter())
            .map(|skill| skill.name.chars().count())
            .max();
        for skill in skills {
            let only_typed = match skill.listed {
                true => "",
                false => " (not offered to the model)",
            };
            let (name, path) = (&skill.name, shown(&skill.path));
            println!("  {name:<0$}  {path}{only_typed}", width.unwrap_or(0));
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
    package.map_or_else(
        || set.origin.name().to_owned(),
        |package| package.written.clone(),
    )
}

/// What a session in the current directory would load.
fn here(settings: &Settings) -> Result<Inventory, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(inventory(&settings.command_sets, &settings.skills, &root))
}

/// How many of `found`'s commands and agents come from `set`.
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
    let providers = match set.directory.join("models.json").is_file() {
        true => ", and providers (models.json)",
        false => "",
    };
    format!(
        "{}, {}{providers}",
        count(commands, "command"),
        count(agents, "agent")
    )
}

fn print_warnings(warnings: &[&Problem]) {
    print_texts(warnings.iter().map(|warning| warning.text.as_str()));
}

/// `texts` under a heading, with the home directory in them as `~`.
fn print_texts<'a>(texts: impl IntoIterator<Item = &'a str>) {
    let home = scramjet_rs::os::home_prefix();
    let tilde = format!("~{}", std::path::MAIN_SEPARATOR);
    for (number, text) in texts.into_iter().enumerate() {
        if number == 0 {
            println!("\nWarnings:");
        }
        match &home {
            Some(home) => println!("  {}", text.replace(home, &tilde)),
            None => println!("  {text}"),
        }
    }
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}
