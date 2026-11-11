//! What the full screen shows as a session starts (the `banner` setting): the plain lines, or
//! one of the variants made for the user to pick from. Each renders again at the screen's
//! width, and Ctrl+O expands it to every key and what was loaded.

use super::installed::count;
use super::keymap::{self, Binding as B};
use super::style::{self, RESET, Style};
use super::text::{FROM, RULE, UNTIL, clip, columns};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Banner {
    /// The name, the model and the directory on one line, then where the session is saved.
    #[default]
    Plain,
    /// As Scramjet's header: the name and version, the main keys, then what was loaded.
    Header,
    /// A box of the model, the project and what was loaded, the name and version on its border.
    Card,
    /// Scramjet's logo, with the version, the model and the project beside it or under it.
    Logo,
}

impl Banner {
    pub(super) const ALL: [Banner; 4] = [Self::Plain, Self::Header, Self::Card, Self::Logo];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Header => "header",
            Self::Card => "card",
            Self::Logo => "logo",
        }
    }

    pub(super) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|banner| banner.name() == name)
    }

    pub(super) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|&banner| banner == self);
        Self::ALL[(at.unwrap_or(0) + 1) % Self::ALL.len()]
    }
}

/// What a banner tells of the session starting.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Opening {
    /// As `interactive::model_line` says it.
    pub(super) model: String,
    pub(super) root: PathBuf,
    /// The session's file; `None`: not saved (`--no-session`).
    pub(super) session: Option<PathBuf>,
    /// The context files, as Scramjet lists them: relative to the project when in it.
    pub(super) context: Vec<PathBuf>,
    /// The command sets, the packages bringing only providers and the skills, by name, with
    /// where each is.
    pub(super) sets: Vec<(String, PathBuf)>,
    pub(super) providers: Vec<(String, PathBuf)>,
    pub(super) skills: Vec<(String, PathBuf)>,
    pub(super) commands: usize,
    pub(super) agents: usize,
}

const NAME: &str = "scramjet";
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

/// Scramjet's logo (`packages/scramjet/assets/scramjet-logo.png` in its repository) in quarter
/// cells: its strokes are as tall as wide, so each takes one quarter's row and two quarters'
/// columns, its bevels a quarter cut from a corner.
const LOGO: [&str; 5] = [
    "                                        ▄",
    " ▄▄▄▄▄▄  ▄▄▄▄▄▖ ▗▄▄▄▖▗▄▄▄▄▄▖  ▄▄▄▄▄▄▄▖  ▄  ▄▄▄▄▄  █▄▄▄▄▖",
    "▝▙▄▄▄▄▖ ▐▌      █     ▄▄▄▄▄█ ▐▌  █   █  █ ▐▙▄▄▄▟▌ █",
    "▄▄▄▄▄▄▛ ▝▙▄▄▄▄▖ █    ▝▙▄▄▄▄▛ ▐▌  █   █  █ ▝▙▄▄▄▄▖ ▜▄▄▄▖",
    "                                    ▄▄▄▄▛",
];
const LOGO_COLUMNS: usize = 56;
/// Between the logo and what is beside it.
const LOGO_GAP: usize = 3;
/// The columns left beside the logo below which what goes there goes under it.
const BESIDE: usize = 30;

/// The plain banner, as the session said it before the variants.
pub(super) fn plain(opening: &Opening) -> String {
    let saved = match &opening.session {
        Some(path) => format!("session {}", path.display()),
        None => "session not saved (--no-session)".to_owned(),
    };
    format!(
        "{NAME} · {} · {}\n{saved}",
        opening.model,
        opening.root.display()
    )
}

/// The rows of `banner`, but the plain one, at most `width` columns wide; all keys and what was
/// loaded if `full`; marked for the full screen's copy if `marked`.
pub(super) fn rows(
    banner: Banner,
    opening: &Opening,
    width: usize,
    full: bool,
    marked: bool,
) -> Vec<String> {
    let width = width.max(20);
    let marks = Marks(marked);
    let mut rows = match banner {
        Banner::Plain | Banner::Header => vec![name_line()],
        Banner::Card => card(opening, width, full, marks),
        Banner::Logo => logo(opening, width, marks),
    };
    if opening.session.is_none() && banner != Banner::Card {
        rows.push(paint(style::WARNING, "Session not saved (--no-session)"));
    }
    if banner == Banner::Logo {
        rows.push(String::new());
    }
    match full {
        true => rows.extend(all_keys()),
        false => rows.extend(flow(
            &main_keys(),
            &paint(style::BANNER_ACTION, " · "),
            width,
        )),
    }
    if banner == Banner::Header || (full && banner == Banner::Logo) {
        rows.extend(sections(opening, width, full));
    }
    rows.iter().map(|row| fit(row, width)).collect()
}

/// `row` cut to `width` columns, which its copy marks take none of.
fn fit(row: &str, width: usize) -> String {
    let unmarked: String = row
        .chars()
        .filter(|c| ![FROM, UNTIL, RULE].contains(c))
        .collect();
    match columns(&unmarked) <= width {
        true => row.to_owned(),
        false => clip(row, width),
    }
}

/// Where a row's copy starts and ends, when marked.
#[derive(Clone, Copy)]
struct Marks(bool);

impl Marks {
    fn from(self) -> String {
        self.mark(FROM)
    }
    fn until(self) -> String {
        self.mark(UNTIL)
    }
    /// A row a copy leaves out.
    fn rule(self) -> String {
        self.mark(RULE)
    }
    fn mark(self, mark: char) -> String {
        match self.0 {
            true => mark.to_string(),
            false => String::new(),
        }
    }
}

fn paint(style: Style, text: &str) -> String {
    format!("{style}{text}{RESET}")
}

fn name_line() -> String {
    let name = paint(style::BANNER_NAME, NAME);
    format!("{name} {}", paint(style::BANNER_VERSION, VERSION))
}

/// A key and what it does, as Scramjet's hints show them; `None` without a key.
fn hint(key: Option<String>, action: &str) -> Option<String> {
    let key = paint(style::BANNER_KEY, &key?);
    Some(format!("{key} {}", paint(style::BANNER_ACTION, action)))
}

fn main_keys() -> Vec<String> {
    let typed = |text: &str| Some(text.to_owned());
    [
        hint(keymap::key(B::Interrupt), "interrupt"),
        hint(keymap::keys(&[B::Clear, B::Exit]), "clear/exit"),
        hint(typed("/"), "commands"),
        hint(typed("!"), "bash"),
        hint(keymap::key(B::ToolsExpand), "more"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Scramjet's expanded list of keys, each on its row, and where to find the others.
fn all_keys() -> Vec<String> {
    let typed = |text: &str| Some(text.to_owned());
    let twice = keymap::key(B::Clear).map(|key| format!("{key} twice"));
    [
        hint(keymap::key(B::Interrupt), "to interrupt"),
        hint(keymap::key(B::Clear), "to clear"),
        hint(twice, "to exit"),
        hint(keymap::key(B::Exit), "to exit (empty)"),
        hint(keymap::key(B::Suspend), "to suspend"),
        hint(keymap::key(B::DeleteToLineEnd), "to delete to end"),
        hint(keymap::key(B::ThinkingCycle), "to cycle thinking level"),
        hint(
            keymap::keys(&[B::ModelCycleForward, B::ModelCycleBackward]),
            "to cycle models",
        ),
        hint(keymap::key(B::ModelSelect), "to select model"),
        hint(keymap::key(B::ToolsExpand), "to expand tools"),
        hint(keymap::key(B::ThinkingToggle), "to show or hide thinking"),
        hint(keymap::key(B::External), "for external editor"),
        hint(typed("/"), "for commands"),
        hint(typed("!"), "to run bash"),
        hint(typed("!!"), "to run bash (no context)"),
        hint(keymap::key(B::FollowUp), "to queue follow-up"),
        hint(keymap::key(B::Dequeue), "to restore queued messages"),
        hint(keymap::key(B::PasteImage), "to paste image"),
        hint(typed("/hotkeys"), "for every key"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// `parts` joined by `separator` into rows of at most `width` columns.
fn flow(parts: &[String], separator: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    for part in parts {
        if !row.is_empty() && columns(&row) + columns(separator) + columns(part) <= width {
            row = row + separator + part;
            continue;
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        row = part.clone();
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

fn shown(path: &Path) -> String {
    scramjet::agent::shortened(&path.display().to_string())
}

/// A context file as Scramjet's list names it: relative to the project when in it.
fn context_name(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
        _ => shown(path),
    }
}

/// How many command sets, commands, agents and skills were loaded.
fn counts(opening: &Opening) -> Vec<String> {
    [
        (opening.sets.len(), "command set"),
        (opening.commands, "command"),
        (opening.agents, "agent"),
        (opening.skills.len(), "skill"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| count(n, what))
    .collect()
}

/// As Scramjet's loaded resources: a heading over each list, its names on rows under it, or
/// each with where it is if `full`.
fn sections(opening: &Opening, width: usize, full: bool) -> Vec<String> {
    let context: Vec<_> = (opening.context.iter())
        .map(|path| (context_name(path, &opening.root), shown(path)))
        .collect();
    let named = |listed: &[(String, PathBuf)]| -> Vec<(String, String)> {
        let listed = listed
            .iter()
            .map(|(name, path)| (name.clone(), shown(path)));
        listed.collect()
    };
    let mut rows = Vec::new();
    for (heading, listed) in [
        ("Context", context),
        ("Command sets", named(&opening.sets)),
        ("Providers", named(&opening.providers)),
        ("Skills", named(&opening.skills)),
    ] {
        if listed.is_empty() {
            continue;
        }
        rows.push(String::new());
        rows.push(paint(style::BANNER_HEADING, &format!("[{heading}]")));
        if full {
            let widest = listed.iter().map(|(name, _)| columns(name)).max();
            for (name, place) in &listed {
                let row = match heading {
                    "Context" => place.clone(),
                    _ => format!("{name:<w$}  {place}", w = widest.unwrap_or(0)),
                };
                rows.push(paint(style::BANNER_LIST, &format!("  {row}")));
            }
            continue;
        }
        let names: Vec<_> = listed.into_iter().map(|(name, _)| name).collect();
        let separator = ", ";
        for row in flow(&names, separator, width.saturating_sub(2)) {
            rows.push(paint(style::BANNER_LIST, &format!("  {row}")));
        }
    }
    rows
}

/// The card: what is loaded, in a box with the name and version on its top border; every
/// context file, set and skill on its row if `full`. A list too wide for the box goes on in
/// the rows under it.
fn card(opening: &Opening, width: usize, full: bool, marks: Marks) -> Vec<String> {
    let one = |value: String| vec![value];
    let mut fields: Vec<(&str, Vec<String>, &str, Style)> = vec![
        ("model", one(opening.model.clone()), "", style::NONE),
        ("project", one(shown(&opening.root)), "", style::NONE),
    ];
    let context: Vec<_> = (opening.context.iter())
        .map(|path| context_name(path, &opening.root))
        .collect();
    match full {
        true => {
            for (index, path) in opening.context.iter().enumerate() {
                let label = if index == 0 { "context" } else { "" };
                fields.push((label, one(shown(path)), "", style::BANNER_LIST));
            }
        }
        false if !context.is_empty() => fields.push(("context", context, ", ", style::NONE)),
        false => {}
    }
    let counts = counts(opening);
    if !counts.is_empty() {
        fields.push(("loaded", counts, " · ", style::NONE));
    }
    if full {
        let listed = (opening.sets.iter())
            .chain(&opening.providers)
            .chain(&opening.skills);
        let widest = listed.map(|(name, _)| columns(name)).max().unwrap_or(0);
        let lists = [
            ("sets", &opening.sets),
            ("providers", &opening.providers),
            ("skills", &opening.skills),
        ];
        for (label, listed) in lists {
            for (index, (name, path)) in listed.iter().enumerate() {
                let label = if index == 0 { label } else { "" };
                let row = format!("{name:<widest$}  {}", shown(path));
                fields.push((label, one(row), "", style::BANNER_LIST));
            }
        }
    }
    if opening.session.is_none() {
        let not_saved = one("not saved (--no-session)".to_owned());
        fields.push(("session", not_saved, "", style::WARNING));
    }
    let labels = fields
        .iter()
        .map(|(label, ..)| label.len())
        .max()
        .unwrap_or(0);
    let title = format!(" {NAME} {VERSION} ");
    let content = (fields.iter())
        .map(|(_, parts, separator, _)| labels + 2 + columns(&parts.join(separator)))
        .max()
        .unwrap_or(0)
        .max(columns(&title) + 2);
    // The box's sides and the space inside each.
    let inner = content.min(width.saturating_sub(4));
    let room = inner.saturating_sub(labels + 2);
    let border = |text: &str| paint(style::BANNER_BORDER, text);
    let rule = marks.rule();
    let mut rows = vec![format!(
        "{rule}{}{}{}{}",
        border("╭─"),
        paint(style::BANNER_NAME, &format!(" {NAME}")),
        paint(style::BANNER_VERSION, &format!(" {VERSION} ")),
        border(&format!(
            "{}╮",
            "─".repeat((inner + 1).saturating_sub(columns(&title)))
        )),
    )];
    for (label, parts, separator, style) in &fields {
        for (index, value) in flow(parts, separator, room).iter().enumerate() {
            let label = if index == 0 { label } else { "" };
            let label = paint(style::BANNER_LABEL, &format!("{label:<labels$}  "));
            let value = clip(&paint(*style, value), room);
            let pad = " ".repeat(room.saturating_sub(columns(&value)));
            let (from, until) = (marks.from(), marks.until());
            let (left, right) = (border("│ "), border(" │"));
            rows.push(format!("{left}{from}{label}{value}{until}{pad}{right}"));
        }
    }
    rows.push(format!(
        "{rule}{}",
        border(&format!("╰{}╯", "─".repeat(inner + 2)))
    ));
    rows
}

/// Scramjet's logo, the version, the model and the project beside it, else under it, else the
/// name alone where the logo does not fit.
fn logo(opening: &Opening, width: usize, marks: Marks) -> Vec<String> {
    let version = format!(
        "{} {}",
        paint(style::BANNER_NAME, "rs"),
        paint(style::BANNER_VERSION, VERSION)
    );
    let said = vec![
        version,
        opening.model.clone(),
        paint(style::MUTED, &shown(&opening.root)),
    ];
    if width < LOGO_COLUMNS {
        let mut rows = vec![name_line()];
        rows.extend(said.into_iter().skip(1));
        return rows;
    }
    let art = |row: &str| paint(style::BANNER_LOGO, row);
    let room = width.saturating_sub(LOGO_COLUMNS + LOGO_GAP);
    if room >= BESIDE {
        // Beside the letters' rows, the first by the dot of the `j`.
        let mut said = said.into_iter();
        return (LOGO.iter().enumerate())
            .map(|(index, row)| {
                let text = (index > 0).then(|| said.next()).flatten();
                let pad = " ".repeat(LOGO_COLUMNS + LOGO_GAP - columns(row));
                match text {
                    Some(text) => format!("{}{pad}{}{}", art(row), marks.from(), clip(&text, room)),
                    None => format!("{}{}", marks.rule(), art(row)),
                }
            })
            .collect();
    }
    let mut rows: Vec<String> = (LOGO.iter())
        .map(|row| format!("{}{}", marks.rule(), art(row)))
        .collect();
    rows.extend(said);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::text::plain;

    fn sample() -> Opening {
        Opening {
            model: "gpt-6-luna · thinking medium".into(),
            root: "/work/words".into(),
            session: Some("/state/sessions/1.jsonl".into()),
            context: vec!["/home/AGENTS.md".into(), "/work/words/CLAUDE.md".into()],
            sets: vec![("mach12".into(), "/sets/mach12".into())],
            providers: Vec::new(),
            skills: vec![("writing".into(), "/skills/writing/SKILL.md".into())],
            commands: 14,
            agents: 3,
        }
    }

    fn shown(banner: Banner, opening: &Opening, width: usize, full: bool) -> Vec<String> {
        let rows = rows(banner, opening, width, full, false);
        rows.iter()
            .map(|row| plain(row).trim_end().to_owned())
            .collect()
    }

    #[test]
    fn rows_marked_for_the_copy_are_as_wide_as_unmarked_ones() {
        for banner in [Banner::Header, Banner::Card, Banner::Logo] {
            for full in [false, true] {
                let marked = rows(banner, &sample(), 70, full, true);
                let unmarked = rows(banner, &sample(), 70, full, false);
                let unmark = |row: &String| row.replace([FROM, UNTIL, RULE], "");
                assert_eq!(
                    marked.iter().map(unmark).collect::<Vec<_>>(),
                    unmarked,
                    "{banner:?}"
                );
            }
        }
        // The card's text is copied without its borders, the logo's without the logo.
        let card = rows(Banner::Card, &sample(), 70, false, true);
        assert!(card[0].starts_with(RULE) && card[1].contains(FROM) && card[1].contains(UNTIL));
        let logo = rows(Banner::Logo, &sample(), 100, false, true);
        assert!(logo[0].starts_with(RULE) && plain(&logo[1]).contains(&format!("   {FROM}rs ")));
    }

    #[test]
    fn each_banner_has_a_name_and_the_next_follows_in_turn() {
        for banner in Banner::ALL {
            assert_eq!(Banner::named(banner.name()), Some(banner));
        }
        assert_eq!(Banner::named("big"), None);
        assert_eq!(Banner::Logo.next(), Banner::Plain);
        assert_eq!(Banner::Plain.next(), Banner::Header);
    }

    #[test]
    fn the_plain_banner_says_the_model_the_directory_and_the_session() {
        let mut opening = sample();
        assert_eq!(
            super::plain(&opening),
            "scramjet · gpt-6-luna · thinking medium · /work/words\nsession /state/sessions/1.jsonl"
        );
        opening.session = None;
        assert!(super::plain(&opening).ends_with("\nsession not saved (--no-session)"));
    }

    #[test]
    fn the_header_shows_the_main_keys_then_what_was_loaded_and_every_key_expanded() {
        let rows = shown(Banner::Header, &sample(), 100, false);
        assert_eq!(rows[0], format!("scramjet {VERSION}"));
        assert_eq!(
            rows[1],
            "Esc interrupt · Ctrl+C/D clear/exit · / commands · ! bash · Ctrl+O more"
        );
        assert_eq!(
            rows[2..],
            [
                "",
                "[Context]",
                "  /home/AGENTS.md, CLAUDE.md",
                "",
                "[Command sets]",
                "  mach12",
                "",
                "[Skills]",
                "  writing"
            ]
        );
        // Narrow, the keys take more rows.
        let rows = shown(Banner::Header, &sample(), 40, false);
        assert_eq!(
            rows[1..3],
            [
                "Esc interrupt · Ctrl+C/D clear/exit",
                "/ commands · ! bash · Ctrl+O more"
            ]
        );
        let rows = shown(Banner::Header, &sample(), 100, true);
        assert_eq!(rows[1], "Esc to interrupt");
        assert!(
            rows.contains(&"/hotkeys for every key".to_owned()),
            "{rows:#?}"
        );
        assert!(
            rows.contains(&"  mach12  /sets/mach12".to_owned()),
            "{rows:#?}"
        );
    }

    #[test]
    fn packages_bringing_only_providers_are_listed_apart_from_command_sets() {
        let mut opening = sample();
        opening.providers = vec![("company".into(), "/packages/company".into())];
        let rows = shown(Banner::Header, &opening, 100, false);
        assert_eq!(
            rows[6..],
            [
                "[Command sets]",
                "  mach12",
                "",
                "[Providers]",
                "  company",
                "",
                "[Skills]",
                "  writing"
            ]
        );
        let rows = shown(Banner::Card, &opening, 100, true);
        assert!(
            rows.iter().any(|row| row.contains("providers  company")),
            "{rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("1 command set ·")),
            "{rows:#?}"
        );
    }

    #[test]
    fn the_card_boxes_what_was_loaded_within_the_width() {
        let rows = shown(Banner::Card, &sample(), 100, false);
        let title = format!("─ scramjet {VERSION} ");
        let top = format!("╭{title}{}╮", "─".repeat(59 - title.chars().count()));
        assert_eq!(
            rows[..6],
            [
                top,
                "│ model    gpt-6-luna · thinking medium                     │".into(),
                "│ project  /work/words                                      │".into(),
                "│ context  /home/AGENTS.md, CLAUDE.md                       │".into(),
                "│ loaded   1 command set · 14 commands · 3 agents · 1 skill │".into(),
                "╰───────────────────────────────────────────────────────────╯".into(),
            ]
        );
        // Narrower, the values are cut; expanded, each file, set and skill has its row.
        let rows = shown(Banner::Card, &sample(), 40, false);
        assert!(rows.iter().all(|row| columns(row) <= 40), "{rows:#?}");
        assert_eq!(
            rows[4..7],
            [
                "│ loaded   1 command set · 14 commands │",
                "│          3 agents · 1 skill          │",
                "╰──────────────────────────────────────╯"
            ]
        );
        let rows = shown(Banner::Card, &sample(), 100, true);
        assert!(
            rows.contains(
                &"│ sets     mach12   /sets/mach12                            │".to_owned()
            ),
            "{rows:#?}"
        );
    }

    #[test]
    fn the_logo_goes_beside_its_text_or_over_it_or_gives_way_to_the_name() {
        let rows = shown(Banner::Logo, &sample(), 100, false);
        assert_eq!(rows[0], LOGO[0]);
        assert_eq!(rows[1], format!("{}   rs {VERSION}", LOGO[1]));
        assert!(
            rows[2].ends_with("   gpt-6-luna · thinking medium"),
            "{rows:#?}"
        );
        assert_eq!(rows[4], LOGO[4]);
        let rows = shown(Banner::Logo, &sample(), 60, false);
        assert_eq!(rows[..5], LOGO.map(|row| row.trim_end().to_owned()));
        assert_eq!(rows[5], format!("rs {VERSION}"));
        assert_eq!(
            rows[8..10],
            [
                "",
                "Esc interrupt · Ctrl+C/D clear/exit · / commands · ! bash"
            ]
        );
        let rows = shown(Banner::Logo, &sample(), 40, false);
        assert_eq!(
            rows[..3],
            [
                format!("scramjet {VERSION}"),
                "gpt-6-luna · thinking medium".into(),
                "/work/words".into()
            ]
        );
        for row in LOGO {
            assert!(columns(row) <= LOGO_COLUMNS);
        }
    }

    #[test]
    fn a_session_not_saved_is_said() {
        let mut opening = sample();
        opening.session = None;
        let rows = shown(Banner::Logo, &opening, 100, false);
        assert!(
            rows.contains(&"Session not saved (--no-session)".to_owned()),
            "{rows:#?}"
        );
        let rows = shown(Banner::Card, &opening, 100, false);
        assert!(
            rows.iter()
                .any(|row| row.contains("session  not saved (--no-session)")),
            "{rows:#?}"
        );
    }
}
