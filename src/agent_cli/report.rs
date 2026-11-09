//! A report shown in the conversation's place until Escape, `/usage`'s and `/changelog`'s: one
//! or more tabs, each with a short form and its details a key away.
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
        match self.details {
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
        } else if matches!(key, Key::Text(text) if text == DETAILS_KEY) {
            self.details = !self.details;
            Took::Moved
        } else if key.is(Binding::SelectCancel) || key.is(Binding::SelectConfirm) {
            Took::Closed
        } else {
            Took::Nothing
        }
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
            Some(format!("{DETAILS_KEY} {shows}")),
            several
                .then(|| keymap::part(&[Binding::Tab], "provider"))
                .flatten(),
            keymap::part(&[Binding::PageUp, Binding::PageDown], "scroll"),
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
