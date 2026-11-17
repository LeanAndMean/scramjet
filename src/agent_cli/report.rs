//! A report shown in the conversation's place until Escape, `/usage`'s, `/changelog`'s,
//! `/context`'s and `/system-prompt`'s: one or more tabs, each with a short form and, unless they
//! are empty, its details a key away.
use super::keymap::{self, Binding};
use super::keys::Key;
use super::style;

pub(super) struct ReportView {
    title: String,
    /// What the details key calls the short form, as it shows it again.
    short: &'static str,
    pub(super) tabs: Vec<Tab>,
    at: usize,
    details: bool,
    /// The reports are markdown, shown as a reply's.
    pub(super) markdown: bool,
    /// Another view a key opens in this one's place, with the key and what it shows.
    link: Option<(&'static str, &'static str, Box<ReportView>)>,
}

pub(super) struct Tab {
    pub(super) name: String,
    pub(super) summary: String,
    pub(super) details: String,
}

/// What a key did in the view.
#[derive(Debug, PartialEq)]
pub(super) enum Took {
    Moved,
    /// Scrolls by this many lines, `isize::MIN` to the start and `isize::MAX` to the end.
    Scrolled(isize),
    Closed,
    Nothing,
}

/// The key that shows a tab's details, and its short form again.
const DETAILS_KEY: &str = "d";

impl ReportView {
    /// Opens on tab `at`, its details first with `details`.
    pub(super) fn new(
        title: &str,
        short: &'static str,
        tabs: Vec<Tab>,
        at: usize,
        details: bool,
    ) -> Self {
        Self {
            title: title.to_owned(),
            short,
            tabs,
            at,
            details,
            markdown: false,
            link: None,
        }
    }

    /// The same view, `key` opening `view` in its place, `what` shown in the keys' hint.
    pub(super) fn linking(self, key: &'static str, what: &'static str, view: ReportView) -> Self {
        Self {
            link: Some((key, what, Box::new(view))),
            ..self
        }
    }

    /// The same view of reports written in markdown.
    pub(super) fn markdown(self) -> Self {
        Self {
            markdown: true,
            ..self
        }
    }

    /// The short form or the details shown.
    pub(super) fn report(&self) -> &str {
        let tab = &self.tabs[self.at];
        match self.details && !tab.details.is_empty() {
            true => tab.details.trim_end(),
            false => tab.summary.trim_end(),
        }
    }

    pub(super) fn key(&mut self, key: &Key) -> Took {
        let count = self.tabs.len();
        if count > 1 && (key.is(Binding::Tab) || key.is(Binding::CursorRight)) {
            self.at = (self.at + 1) % count;
            Took::Moved
        } else if count > 1 && key.is(Binding::CursorLeft) {
            self.at = (self.at + count - 1) % count;
            Took::Moved
        } else if key.is(Binding::CursorUp) || key.is(Binding::CursorDown) {
            Took::Scrolled(if key.is(Binding::CursorUp) { -1 } else { 1 })
        } else if key.is(Binding::CursorLineStart) {
            Took::Scrolled(isize::MIN)
        } else if key.is(Binding::CursorLineEnd) {
            Took::Scrolled(isize::MAX)
        } else if let Some((link, _, _)) = &self.link
            && matches!(key, Key::Text(text) if text == link)
        {
            let (_, _, view) = self.link.take().unwrap();
            *self = *view;
            Took::Moved
        } else if self.has_details() && matches!(key, Key::Text(text) if text == DETAILS_KEY) {
            self.details = !self.details;
            Took::Moved
        } else if key.is(Binding::SelectCancel) || key.is(Binding::SelectConfirm) {
            Took::Closed
        } else {
            Took::Nothing
        }
    }

    fn has_details(&self) -> bool {
        self.tabs.iter().any(|tab| !tab.details.is_empty())
    }

    /// The dock's rows under the report: its title with the keys, then, with more than one tab,
    /// the tabs, the one shown marked, each row within `width`.
    pub(super) fn lines(&self, width: usize) -> Vec<String> {
        use style::{MUTED, RESET, SELECTED, SELECTOR_TITLE};
        let shows = match self.details {
            true => self.short,
            false => "details",
        };
        let several = self.tabs.len() > 1;
        let keys = keymap::hint([
            self.has_details().then(|| format!("{DETAILS_KEY} {shows}")),
            (self.link.as_ref()).map(|(key, what, _)| format!("{key} {what}")),
            several
                .then(|| keymap::part(&[Binding::Tab], "provider"))
                .flatten(),
            keymap::part(
                &[
                    Binding::CursorUp,
                    Binding::CursorDown,
                    Binding::PageUp,
                    Binding::PageDown,
                ],
                "scroll",
            ),
            keymap::part(&[Binding::SelectCancel], "close"),
        ]);
        let title = format!(
            "{SELECTOR_TITLE}{}{RESET}  {MUTED}{keys}{RESET}",
            self.title
        );
        let width = width.saturating_sub(1);
        let mut lines = vec![super::text::clip(&title, width)];
        if several {
            let tabs: Vec<String> = (self.tabs.iter().enumerate())
                .map(|(index, tab)| match index == self.at {
                    true => format!("{SELECTED} {} {RESET}", tab.name),
                    false => format!("{MUTED} {} {RESET}", tab.name),
                })
                .collect();
            lines.push(super::text::clip(&tabs.join(" "), width));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::keys::tests::keys;

    fn view(name: &str, details: &str) -> ReportView {
        let tab = Tab {
            name: name.to_owned(),
            summary: format!("{name}'s report"),
            details: details.to_owned(),
        };
        ReportView::new(name, "summary", vec![tab], 0, false)
    }

    #[test]
    fn arrows_and_home_and_end_scroll_and_a_link_opens_its_view_in_place() {
        let key = |text: &[u8]| keys(text).remove(0);
        let mut shown = view("Context", "more").linking("s", "system prompt", view("Prompt", ""));
        let hint = style::themed(&shown.lines(100)[0], false);
        assert!(hint.contains("d details · s system prompt · "), "{hint}");
        assert_eq!(shown.key(&key(b"\x1b[A")), Took::Scrolled(-1));
        assert_eq!(shown.key(&key(b"\x1b[B")), Took::Scrolled(1));
        assert_eq!(shown.key(&key(b"\x1b[H")), Took::Scrolled(isize::MIN));
        assert_eq!(shown.key(&key(b"\x1b[F")), Took::Scrolled(isize::MAX));
        assert_eq!(shown.key(&key(b"s")), Took::Moved);
        assert_eq!(shown.report(), "Prompt's report");
        // Without details, d does nothing and is not offered.
        assert_eq!(shown.key(&key(b"d")), Took::Nothing);
        assert_eq!(shown.key(&key(b"s")), Took::Nothing);
        let hint = style::themed(&shown.lines(100)[0], false);
        assert!(
            !hint.contains("details") && !hint.contains("system prompt"),
            "{hint}"
        );
        assert_eq!(shown.key(&key(b"\x1b")), Took::Closed);
    }
}
