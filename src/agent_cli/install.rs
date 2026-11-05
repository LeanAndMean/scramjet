//! `scramjet-rs install`, `remove` and `list`: command sets installed by reference, recorded in
//! the settings' `commandSets` without being copied, as Pi installs a local path; `list` also
//! shows the skills.
use super::settings::Settings;
use scramjet_rs::agent::{Inventory, Problem, Set, inventory, shortened};
use std::path::Path;

pub(super) const USAGE: &str = "Usage: scramjet-rs install DIR | remove NAME|DIR | list";

/// Runs `command` with its `arguments` when it is one of these; `None` when it is not.
pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    let result = match (command, arguments) {
        ("install", [directory]) => install(Path::new(directory)),
        ("remove", [reference]) => remove(reference),
        ("list", []) => list(),
        ("install" | "remove" | "list", _) => Err(USAGE.to_owned()),
        _ => return None,
    };
    Some(result)
}

fn install(directory: &Path) -> Result<(), String> {
    let mut settings = Settings::load();
    let set = settings.install_set(directory)?;
    let found = here(&settings)?;
    println!(
        "Installed {} from {}: {}",
        set.name,
        shown(&set.directory),
        contents(&found, &set)
    );
    let directory = set.directory.display().to_string();
    let concerned: Vec<&Problem> = (found.warnings.iter())
        .filter(|warning| warning.text.contains(&directory))
        .collect();
    print_warnings(&concerned);
    Ok(())
}

fn remove(reference: &str) -> Result<(), String> {
    let mut settings = Settings::load();
    let directory = settings.remove_set(reference)?;
    let name = directory.file_name().unwrap_or_default().to_string_lossy();
    println!("Removed {name}, installed from {}", shown(&directory));
    Ok(())
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
        println!("No command sets. `scramjet-rs install DIR` installs one.");
    }
    let rows: Vec<[String; 4]> = (sets.iter())
        .map(|set| {
            [
                set.name.clone(),
                set.origin.name().to_owned(),
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
        println!("Installed sets are listed in {path} (commandSets).");
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
    if warnings.is_empty() {
        return;
    }
    println!("\nWarnings:");
    let home = std::env::home_dir().map(|home| format!("{}/", home.display()));
    for warning in warnings {
        match &home {
            Some(home) if home != "/" => println!("  {}", warning.text.replace(home, "~/")),
            _ => println!("  {warning}"),
        }
    }
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}
