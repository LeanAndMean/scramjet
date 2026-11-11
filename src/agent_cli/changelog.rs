//! What changed in each version, from the `CHANGELOG.md` built in: `/changelog`, and What's new
//! at the first start after the version changed, as Scramjet's.
use super::report::{ReportView, Tab};

const CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// A released version's section.
#[derive(Debug, PartialEq)]
struct Version<'a> {
    number: [u64; 3],
    /// Its heading without `## `: `0.123.0 — Title`.
    heading: &'a str,
    /// The part (a `# ` heading and its text) this version is the first of, as `# scramjet-rs`.
    part: Option<&'a str>,
    /// The whole section, its heading included.
    text: &'a str,
}

/// The built-in changelog's released versions, newest first, without its own heading and the
/// section of what is not released yet.
pub(super) fn released() -> &'static str {
    released_in(CHANGELOG)
}

fn released_in(changelog: &str) -> &str {
    let start = (changelog.match_indices("\n## ").map(|(at, _)| at + 1))
        .find(|&at| changelog[at + 3..].starts_with(|c: char| c.is_ascii_digit()));
    start.map_or("", |at| changelog[at..].trim_end())
}

fn versions(changelog: &str) -> Vec<Version<'_>> {
    let released = released_in(changelog);
    let starts: Vec<usize> = std::iter::once(0)
        .chain(
            (released.match_indices('\n').map(|(at, _)| at + 1)).filter(|&at| {
                released[at..].starts_with("## ") || released[at..].starts_with("# ")
            }),
        )
        .collect();
    let ends = starts.iter().skip(1).copied().chain([released.len()]);
    let mut part = None;
    let mut versions = Vec::new();
    for (&start, end) in starts.iter().zip(ends) {
        let text = released[start..end].trim_end();
        if text.starts_with("# ") {
            part = Some(text);
            continue;
        }
        let Some(heading) = text
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("## "))
        else {
            continue;
        };
        let number = heading.split_whitespace().next().unwrap_or_default();
        if number.starts_with(|c: char| c.is_ascii_digit()) {
            versions.push(Version {
                number: parsed(number),
                heading,
                part: part.take(),
                text,
            });
        }
    }
    versions
}

/// `0.123.0` as its three numbers, a missing or unreadable one 0, as Scramjet reads them.
fn parsed(version: &str) -> [u64; 3] {
    let mut parts = version.split('.').map(|part| part.parse().unwrap_or(0));
    [(); 3].map(|()| parts.next().unwrap_or(0))
}

/// A view of `versions`: each whole, or with `collapse` its title line first.
fn view(title: &str, versions: &[Version], collapse: bool) -> ReportView {
    let mut summary = String::new();
    let mut details = String::new();
    for version in versions {
        if let Some(part) = version.part {
            let name = part
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches("# ");
            summary.push_str(&format!("\n\n**{name}**\n"));
            details.push_str(&format!("\n\n{part}"));
        }
        let item = match version.heading.split_once(' ') {
            Some((number, rest)) => format!("- **{number}** {}", rest.trim()),
            None => format!("- **{}**", version.heading),
        };
        summary.push_str(&format!("\n{item}"));
        details.push_str(&format!("\n\n{}", version.text));
    }
    let tabs = vec![Tab {
        name: title.to_owned(),
        summary: summary.trim().to_owned(),
        details: details.trim().to_owned(),
    }];
    ReportView::new(title, "titles", tabs, 0, !collapse).markdown()
}

/// `/changelog` on the full screen: every released version.
pub(super) fn whole(collapse: bool) -> ReportView {
    view("Changelog", &versions(CHANGELOG), collapse)
}

/// What's new since `last`, the version last run: the versions after it, if any.
pub(super) fn whats_new(last: &str, collapse: bool) -> Option<ReportView> {
    newer(CHANGELOG, last).map(|versions| view("What's new", &versions, collapse))
}

fn newer<'a>(changelog: &'a str, last: &str) -> Option<Vec<Version<'a>>> {
    let last = parsed(last);
    let newer: Vec<Version> = (versions(changelog).into_iter())
        .filter(|version| version.number > last)
        .collect();
    (!newer.is_empty()).then_some(newer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGELOG: &str = "# Changelog\n\nIntro.\n\n## Unreleased\n\n### Added\n\n- c\n\n\
                             ## 1.10.0 — Three\n\n- c\n\n## 1.9.1 — Two\n\n- b\n\n\
                             ## 1.2.0 — One\n\n- a\n";

    #[test]
    fn release_notes_start_at_the_first_released_version() {
        let changelog = "# Changelog\n\nIntro.\n\n## Unreleased\n\n## 1.2.0 — Two\n\n\
                         - b\n\n## 1.1.0 — One\n\n- a\n";
        assert_eq!(
            released_in(changelog),
            "## 1.2.0 — Two\n\n- b\n\n## 1.1.0 — One\n\n- a"
        );
        assert_eq!(released_in("# Changelog\n\n## Unreleased\n"), "");
        let version = format!("## {} — ", env!("CARGO_PKG_VERSION"));
        assert!(released().starts_with(&version), "{}", &released()[..80]);
    }

    #[test]
    fn whats_new_holds_the_versions_after_the_last_one_run_compared_as_numbers() {
        let headings = |last| {
            newer(CHANGELOG, last).map(|versions| {
                (versions.into_iter())
                    .map(|version| version.heading)
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            headings("1.2.0").unwrap(),
            ["1.10.0 — Three", "1.9.1 — Two"]
        );
        assert_eq!(headings("1.9.1").unwrap(), ["1.10.0 — Three"]);
        assert_eq!(headings("1.10.0"), None);
        assert_eq!(headings("2.0.0"), None);
        assert_eq!(headings("nonsense").unwrap().len(), 3);
        let shown = view("What's new", &newer(CHANGELOG, "1.2.0").unwrap(), false);
        assert_eq!(
            shown.report(),
            "## 1.10.0 — Three\n\n- c\n\n## 1.9.1 — Two\n\n- b"
        );
        let collapsed = view("What's new", &newer(CHANGELOG, "1.2.0").unwrap(), true);
        assert_eq!(
            collapsed.report(),
            "- **1.10.0** — Three\n- **1.9.1** — Two"
        );
    }

    #[test]
    fn a_part_heading_goes_with_the_first_version_under_it() {
        let changelog = "# Changelog\n\n## Unreleased\n\n## 2.0.0 — Two\n\n- b\n\n\
                         # Old\n\nBefore 2.0.\n\n## 1.1.0 — One\n\n- a\n\n## 1.0.0 — Zero\n";
        let versions = versions(changelog);
        let parts: Vec<_> = versions.iter().map(|version| version.part).collect();
        assert_eq!(parts, [None, Some("# Old\n\nBefore 2.0."), None]);
        assert_eq!(versions[0].text, "## 2.0.0 — Two\n\n- b");
        let shown = view("Changelog", &versions, false);
        assert_eq!(
            shown.report(),
            "## 2.0.0 — Two\n\n- b\n\n# Old\n\nBefore 2.0.\n\n## 1.1.0 — One\n\n- a\n\n\
             ## 1.0.0 — Zero"
        );
        let collapsed = view("Changelog", &versions, true);
        assert_eq!(
            collapsed.report(),
            "- **2.0.0** — Two\n\n**Old**\n\n- **1.1.0** — One\n- **1.0.0** — Zero"
        );
        let newer = newer(changelog, "1.1.0").unwrap();
        assert_eq!(
            view("What's new", &newer, false).report(),
            "## 2.0.0 — Two\n\n- b"
        );
    }

    #[test]
    fn the_built_in_changelog_reads_as_versions_newest_first() {
        let versions = versions(super::CHANGELOG);
        assert_eq!(versions[0].number, parsed(env!("CARGO_PKG_VERSION")));
        assert!(versions.len() > 100);
        assert!(
            versions
                .windows(2)
                .all(|pair| pair[0].number > pair[1].number)
        );
        assert!(
            versions
                .iter()
                .all(|version| version.text.starts_with("## "))
        );
        let old = versions
            .iter()
            .find(|version| version.number[0] == 0)
            .unwrap();
        assert!(old.part.unwrap().starts_with("# scramjet-rs\n"));
        assert_eq!(
            versions
                .iter()
                .filter(|version| version.part.is_some())
                .count(),
            1
        );
    }
}
