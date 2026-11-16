//! `scramjet catalog add`, `remove` and `list`: the model catalogs providers come from, kept in
//! the settings' `modelCatalogs`, for a user or another program's setup to run; each says what
//! it did, and adding or removing a URL twice does nothing more.
use super::settings::Settings;
use scramjet::agent::login::Logins;
use scramjet::agent::{Catalog, Providers, check_catalog_url};

pub(super) const USAGE: &str = "Usage: scramjet catalog add URL | remove URL | list";

/// Runs `catalog` with its `arguments`; `None` for another command.
pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    if command != "catalog" {
        return None;
    }
    Some(match arguments {
        [action, url] if action == "add" => add(url),
        [action, url] if action == "remove" => remove(url),
        [action] if action == "list" => list(),
        _ => Err(USAGE.to_owned()),
    })
}

fn add(url: &str) -> Result<(), String> {
    check_catalog_url(url).map_err(|problem| format!("{url}: {problem}"))?;
    let mut settings = Settings::load();
    let shown = settings.shown_path().unwrap_or_default();
    match settings.add_catalog(url)? {
        true => outln!("Added {url} to modelCatalogs in {shown}"),
        false => outln!("{url} is in modelCatalogs already ({shown})"),
    }
    let logins = Logins::user();
    let catalog = Catalog::new(url, logins.as_ref());
    if let Err(problem) = catalog.relist() {
        outln!("Warning: {problem}");
        match catalog.age() {
            Some(_) => outln!("Its models are offered as it last listed them"),
            None => outln!("Its models are offered once it can be read"),
        }
        return Ok(());
    }
    let offered = catalog.offers();
    let models = offered.models();
    let names: Vec<&str> = (offered.providers.iter())
        .map(|(name, _)| name.as_str())
        .collect();
    outln!(
        "It lists {models} model{} from {} provider{}{}",
        if models == 1 { "" } else { "s" },
        names.len(),
        if names.len() == 1 { "" } else { "s" },
        match names.is_empty() {
            true => String::new(),
            false => format!(": {}", few(&names)),
        }
    );
    let providers = Providers::read(settings.sources(), logins);
    let read = providers.catalogs().iter().find(|read| read.url() == url);
    let shadowed = read.map(|read| read.shadowed()).unwrap_or_default();
    if !shadowed.is_empty() {
        let shadowed: Vec<&str> = shadowed.iter().map(String::as_str).collect();
        outln!(
            "Not offered, as a models.json, a login or a catalog before names them: {}",
            few(&shadowed)
        );
    }
    let skipped = offered.skipped.len();
    if skipped > 0 {
        let first =
            (offered.skipped.iter().take(SHOWN)).map(|(name, why)| format!("{name}: {why}"));
        outln!(
            "Not offered, {skipped} provider{}: {}{}",
            if skipped == 1 { "" } else { "s" },
            first.collect::<Vec<_>>().join("; "),
            match skipped > SHOWN {
                true => format!("; and {} more", skipped - SHOWN),
                false => String::new(),
            }
        );
    }
    let ours = format!("model catalog {url}: ");
    for problem in providers.problems() {
        if let Some(problem) = problem.strip_prefix(&ours) {
            outln!("Warning: {problem}");
        }
    }
    Ok(())
}

/// How many names are shown of a longer list.
const SHOWN: usize = 10;

/// `names`, the first ten of a longer list followed by how many more there are.
fn few(names: &[&str]) -> String {
    match names.len() > SHOWN + 1 {
        true => format!(
            "{} and {} more",
            names[..SHOWN].join(", "),
            names.len() - SHOWN
        ),
        false => names.join(", "),
    }
}

fn remove(url: &str) -> Result<(), String> {
    let mut settings = Settings::load();
    let shown = settings.shown_path().unwrap_or_default();
    match settings.remove_catalog(url)? {
        true => outln!("Removed {url} from modelCatalogs in {shown}"),
        false => outln!("{url} is not in modelCatalogs ({shown}); nothing to remove"),
    }
    Catalog::new(url, Logins::user().as_ref()).forget();
    Ok(())
}

fn list() -> Result<(), String> {
    let settings = Settings::load();
    for warning in &settings.warnings {
        eprintln!("Warning: {warning}");
    }
    if settings.model_catalogs.is_empty() {
        let shown = settings.shown_path().unwrap_or_default();
        outln!("No model catalogs in {shown} (scramjet catalog add URL)");
        return Ok(());
    }
    let logins = Logins::user();
    for url in &settings.model_catalogs {
        let catalog = Catalog::new(url, logins.as_ref());
        let read = match catalog.age() {
            None => "not read yet".to_owned(),
            Some(age) => {
                let models = catalog.offers().models();
                let minutes = age.as_secs() / 60;
                format!(
                    "{models} model{}, read {}",
                    if models == 1 { "" } else { "s" },
                    match minutes {
                        0 => "just now".to_owned(),
                        1 => "a minute ago".to_owned(),
                        minutes if minutes < 120 => format!("{minutes} minutes ago"),
                        minutes => format!("{} hours ago", minutes / 60),
                    }
                )
            }
        };
        outln!("{url}  {read}");
    }
    Ok(())
}
