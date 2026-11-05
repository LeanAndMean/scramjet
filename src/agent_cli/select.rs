//! A list to choose from with the arrow keys, drawn in the prompt's place on the full screen, as
//! Scramjet's selectors for `/resume`, `/fork`, `/model` and `/tree` are; those for `/resume`,
//! `/model` and `/tree` search as you type, `/resume`'s renames and deletes sessions, and
//! `/tree`'s filters its entries, folds their branches and labels them.

use super::editor::terminal_size;
use super::fuzzy::{self, Haystack};
use super::keymap::{self, Binding};
use super::keys::Key;
use super::settings::scramjet_level;
use super::style::{self, Style};
use super::text;
use super::tree::{Filter, View};
use scramjet_rs::agent::{Choice, Label, ModelOption};
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Rows shown at once, as in Scramjet's selectors other than its tree.
pub(super) const VISIBLE: usize = 10;
/// The tree's filters' actions; `None`: cycling through them.
const TREE_FILTERS: [(Binding, Option<Filter>); 7] = [
    (Binding::TreeFilterDefault, Some(Filter::Default)),
    (Binding::TreeFilterNoTools, Some(Filter::NoTools)),
    (Binding::TreeFilterUserOnly, Some(Filter::UserOnly)),
    (Binding::TreeFilterLabeledOnly, Some(Filter::Labeled)),
    (Binding::TreeFilterAll, Some(Filter::All)),
    (Binding::TreeFilterCycleForward, None),
    (Binding::TreeFilterCycleBackward, None),
];
/// A list's hint: `↑↓ move · Enter {confirm}`, then `others`' keys, then `Esc {cancel}`.
pub(super) fn hint(confirm: &str, others: &[(Binding, &str)], cancel: &str) -> String {
    let others = (others.iter()).map(|&(binding, what)| keymap::part(&[binding], what));
    let first = [
        keymap::part(&[Binding::SelectUp, Binding::SelectDown], "move"),
        keymap::part(&[Binding::SelectConfirm], confirm),
    ];
    let last = keymap::part(&[Binding::SelectCancel], cancel);
    keymap::hint(first.into_iter().chain(others).chain([last]))
}

fn tree_hint() -> String {
    let filters: Vec<_> = (TREE_FILTERS.iter())
        .filter_map(|&(binding, filter)| filter.map(|_| binding))
        .collect();
    let cycle = keymap::key(Binding::TreeFilterCycleForward);
    let filters = match (keymap::keys(&filters), cycle) {
        (Some(keys), Some(cycle)) => Some(format!("{keys} filters ({cycle} cycle)")),
        (keys, cycle) => keys.or(cycle).map(|keys| format!("{keys} filters")),
    };
    keymap::hint([
        keymap::part(&[Binding::SelectUp, Binding::SelectDown], "move"),
        keymap::part(&[Binding::CursorLeft, Binding::CursorRight], "page"),
        keymap::part(
            &[Binding::TreeFoldOrUp, Binding::TreeUnfoldOrDown],
            "fold/branch",
        ),
        keymap::part(&[Binding::TreeEditLabel], "label"),
        filters,
        keymap::part(&[Binding::TreeToggleLabelTimestamp], "label time"),
        keymap::part(&[Binding::SelectCancel], "cancel"),
    ])
}

fn confirm_delete() -> String {
    let keys = keymap::hint([
        keymap::part(&[Binding::SelectConfirm], "confirm"),
        keymap::part(&[Binding::SelectCancel], "cancel"),
    ]);
    format!("Delete session? {keys}")
}

/// Options shown at once in a question's list, as in Scramjet.
const QUESTION_VISIBLE: usize = 8;
/// Rows an option's label or description takes at most, as in Scramjet.
const OPTION_ROWS: usize = 4;
/// Rows the note under a list takes at most, enough for a long problem in `/settings`'
/// Installed with what Enter does about it.
const NOTE_ROWS: usize = 8;
const DESCRIPTION_INDENT: &str = "     ";

/// The columns a selector's row has, the mark after the current one included, as `lines` gives
/// them on the terminal.
fn row_width() -> usize {
    terminal_size().0.saturating_sub(1).max(8) - 2
}

fn question_visible(options: &[Choice]) -> usize {
    options.len().min(QUESTION_VISIBLE)
}

/// What a list's own keys ask for besides a row, as in Scramjet's session selector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Act {
    /// Ctrl+R.
    Rename,
    /// Ctrl+D, then Enter.
    Delete,
    /// `/scoped-models`' keys (`Picker::scoping`): Enter, Ctrl+A, Ctrl+X, Ctrl+P, Alt+Up,
    /// Alt+Down and Ctrl+S.
    Toggle,
    EnableAll,
    ClearAll,
    Provider,
    Earlier,
    Later,
    Save,
}

/// How the session list's keys ask to show it, as in Scramjet's session selector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Toggle {
    /// Ctrl+S: the next order.
    Sort,
    /// Ctrl+N: only the named sessions, or all again.
    Named,
    /// Ctrl+P: with the sessions' paths, or without.
    Paths,
}

/// Where a list was when one of its own keys ended it, so that it shows again there.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Spot {
    pub(super) query: String,
    /// The selected row's position among those shown.
    pub(super) position: usize,
    /// The rows shown, those matching the query.
    pub(super) shown: Vec<usize>,
}

/// The model a next step runs with and its effort, which Left/Right and Shift+Tab choose in its
/// list, as in Scramjet's selector.
pub(super) struct Route {
    models: Vec<ModelOption>,
    /// The chosen model's index in `models`, and the session's.
    at: usize,
    current: usize,
    /// The effort set: the session's, until Shift+Tab chose one.
    effort: Option<String>,
    cycled: bool,
}

/// What a next step's list chose besides the step: `None` keeps the session's.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Routed {
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
}

impl Route {
    /// `models`, the session's at `current`, its effort `effort`.
    pub(super) fn new(models: Vec<ModelOption>, current: usize, effort: Option<String>) -> Self {
        let current = current.min(models.len().saturating_sub(1));
        Self {
            models,
            at: current,
            current,
            effort,
            cycled: false,
        }
    }

    pub(super) fn routed(&self) -> Routed {
        Routed {
            model: (self.at != self.current).then(|| self.models[self.at].id.clone()),
            effort: self.effort.clone().filter(|_| self.cycled),
        }
    }

    /// Takes Left, Right and Shift+Tab; whether it took `key`.
    fn key(&mut self, key: &Key) -> bool {
        let count = self.models.len();
        match key {
            _ if count > 1 && key.is(Binding::CursorLeft) => {
                self.at = (self.at + count - 1) % count;
            }
            _ if count > 1 && key.is(Binding::CursorRight) => self.at = (self.at + 1) % count,
            _ if key.is(Binding::ThinkingCycle) => {
                let model = &self.models[self.at];
                if let Some(level) = model.next_effort(self.effort.as_deref()) {
                    (self.effort, self.cycled) = (Some(level), true);
                }
            }
            _ => return false,
        }
        true
    }

    /// The chosen model, when there are others, and its effort, with the keys that change them.
    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let Some(model) = self.models.get(self.at) else {
            return lines;
        };
        if self.models.len() > 1 {
            let name = match &model.name {
                Some(name) => format!("{name} ({})", model.id),
                None => model.id.clone(),
            };
            let change = keymap::part(&[Binding::CursorLeft, Binding::CursorRight], "change");
            let change = change.map_or_else(String::new, |change| format!(" · {change}"));
            lines.push(format!("model: {name}{change}"));
        }
        let effort = match model.effort(self.effort.as_deref()) {
            Some(level) => scramjet_level(&level).to_owned(),
            None if model.reasoning => "model default".to_owned(),
            None => "off".to_owned(),
        };
        let cycle = (model.next_effort(self.effort.as_deref()))
            .and_then(|_| keymap::part(&[Binding::ThinkingCycle], "cycle"))
            .map_or_else(String::new, |cycle| format!(" · {cycle}"));
        lines.push(format!("effort: {effort}{cycle}"));
        lines
    }
}

/// What a key ended a list with.
#[derive(Debug, PartialEq)]
pub(super) enum Picked {
    /// The row chosen with Enter; `None`: cancelled.
    Row(Option<usize>),
    Act(Act, usize, Spot),
    /// One of the session list's keys (`Picker::acting`), whatever row is selected.
    Toggle(Toggle, Spot),
    /// Tab, in a list that shows one of two sets of rows (`Picker::tabbing`).
    Tab(Spot),
    /// Shift+L in `/tree`, to label the entry, an index into the tree's entries.
    Label(usize),
    /// The row chosen with Enter in a list that is shown again after it (`Picker::placing`).
    Placed(usize, Spot),
}

/// A list for a screen that draws it and passes it the keys; it returns the row chosen with
/// Enter, nothing when cancelled with Escape or Ctrl-C, or what one of its own keys asks for.
pub(super) struct Picker {
    list: List,
    shape: Shape,
    /// When a question's recommended option is chosen, unless `stop` is called first.
    deadline: Option<Instant>,
    /// Stays shown once answered, until the session shows another list or none.
    held: bool,
    /// Whether the session list's keys do (`acting`).
    acts: bool,
    /// Whether `/scoped-models`' keys do (`scoping`).
    scoping: bool,
    /// Whether Tab ends the list (`tabbing`).
    tabs: bool,
    /// Whether Enter returns where the list was (`placing`).
    places: bool,
    /// Said in place of the keys' hint until the next key: what a key did, or why it could not
    /// (`true`: a warning).
    status: Option<(String, bool)>,
}

enum Shape {
    /// A selector's rows, given at the width they take on the terminal when the list opens.
    Rows {
        title: String,
        hint: String,
        current: Option<usize>,
        rows: Vec<String>,
        /// Shown under the list for the selected row, when there is one for each row.
        notes: Vec<String>,
        /// The rows shown without a query, under tree lines (`threaded`); empty: `rows`.
        threaded: Vec<String>,
    },
    Question {
        title: String,
        options: Vec<Choice>,
        recommended: Option<usize>,
        /// A next step's model and effort (`routing`).
        route: Option<Route>,
    },
    /// `/tree`'s entries, which its keys filter, search and fold, and the entry selected last,
    /// which the selection goes back to after a filter or a query that showed none.
    Tree(Box<View>, Option<usize>),
}

impl Picker {
    /// `title` over `count` rows, at most `visible` at once, `row(index, width)` giving each
    /// one's text in at most `width` columns, starting at `initial`; the `current` row is marked.
    pub(super) fn select(
        title: &str,
        visible: usize,
        count: usize,
        initial: usize,
        current: Option<usize>,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        let list = List::new(count, initial, visible);
        Self::rows(title, list, current, row)
    }

    /// As `select`, with a line to type a query on, as Scramjet's session and model selectors
    /// have: the query shows only the rows whose `texts` match it, best first (see `fuzzy`), and
    /// `none` when no row does.
    pub(super) fn search(
        title: &str,
        visible: usize,
        texts: Vec<Haystack>,
        initial: usize,
        current: Option<usize>,
        none: &str,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        let list = List::searching(texts, initial, visible, none);
        Self::rows(title, list, current, row)
    }

    /// As Scramjet's list for a question the model asks: each option's label with its
    /// description under it, at most `QUESTION_VISIBLE` options at once, framed with `title` and
    /// the keys to use.
    pub(super) fn question(title: &str, options: &[Choice], recommended: Option<usize>) -> Self {
        Self {
            list: List::new(
                options.len(),
                recommended.unwrap_or(0),
                question_visible(options),
            ),
            shape: Shape::Question {
                title: title.to_owned(),
                options: options.to_vec(),
                recommended,
                route: None,
            },
            deadline: None,
            held: false,
            acts: false,
            scoping: false,
            tabs: false,
            places: false,
            status: None,
        }
    }

    /// Scramjet's tree selector: the entries `view` shows, at most `visible` at once, from
    /// `initial` (an index into the tree's entries) or its nearest shown ancestor. It returns the
    /// entry chosen, as such an index.
    pub(super) fn tree(view: View, visible: usize, initial: Option<usize>) -> Self {
        let list = List::new(view.rows().len(), view.row_of(initial), visible);
        Self {
            list,
            shape: Shape::Tree(Box::new(view), None),
            deadline: None,
            held: false,
            acts: false,
            scoping: false,
            tabs: false,
            places: false,
            status: None,
        }
    }

    /// Whether the list takes `key`, which the screen takes for itself otherwise: the tree's
    /// Ctrl+T and Ctrl+O choose its filters.
    pub(super) fn takes(&self, key: &Key) -> bool {
        matches!(self.shape, Shape::Tree(..))
            && TREE_FILTERS.iter().any(|&(binding, _)| key.is(binding))
    }

    /// The label of the tree's `entry` (`Picker::tree`).
    pub(super) fn tree_label(&self, entry: usize) -> Option<&str> {
        match &self.shape {
            Shape::Tree(view, _) => view.label(entry),
            _ => None,
        }
    }

    /// Shows `label` on the tree's `entry` (`Some(None)`: none; `None` leaves it as it was), and
    /// says `status` in place of the keys' hint until the next key, when given (`true`: a
    /// warning).
    pub(super) fn relabel(
        &mut self,
        entry: usize,
        label: Option<Option<Label>>,
        status: Option<(String, bool)>,
    ) {
        if let Shape::Tree(view, last) = &mut self.shape {
            if let Some(label) = label {
                let selected = view.rows().get(self.list.selected).map(|&(entry, _)| entry);
                view.set_label(entry, label);
                reselect(view, &mut self.list, last, selected.or(*last));
            }
            self.status = status;
        }
    }

    /// Says `hint` on the keys after the title, and under the rows the selected one's note.
    pub(super) fn noting(mut self, keys: String, row_notes: Vec<String>) -> Self {
        if let Shape::Rows { hint, notes, .. } = &mut self.shape {
            (*hint, *notes) = (keys, row_notes);
        }
        self
    }

    /// Makes the rows `headings` marks headings of the rows under them, which the selection
    /// passes over.
    pub(super) fn headed(mut self, headings: Vec<bool>) -> Self {
        self.list.headings = headings;
        self.list.settle(false);
        self
    }

    /// Keeps the list shown once answered, so that a session showing another in its place at
    /// once (`/settings`) does not show the prompt between them.
    pub(super) fn held(mut self) -> Self {
        self.held = true;
        self
    }

    pub(super) fn is_held(&self) -> bool {
        self.held
    }

    /// Gives the list the keys of Scramjet's session selector: Ctrl+R ends it to rename the
    /// selected row; Ctrl+D asks to confirm, and Enter then ends it to delete the row, unless it
    /// is the current one; Ctrl+S, Ctrl+N and Ctrl+P end it to show its rows another way
    /// (`Toggle`). It shows even with no rows, which those keys can bring back.
    pub(super) fn acting(mut self) -> Self {
        self.acts = true;
        self
    }

    /// Makes `/scoped-models`' keys end the list with what they ask for on the selected row, as
    /// in Scramjet's list (`Act`); without a row they do nothing.
    pub(super) fn scoping(mut self) -> Self {
        self.scoping = true;
        self
    }

    /// Makes Tab end the list where it is, for the session to show the other of two sets of
    /// rows, as Tab switches between all models and the scope in Scramjet's model selector.
    pub(super) fn tabbing(mut self) -> Self {
        self.tabs = true;
        self
    }

    /// Makes Enter end the list with where it was, its query and selection, for the session to
    /// show it there again (`at`).
    pub(super) fn placing(mut self) -> Self {
        self.places = true;
        self
    }

    /// Keeps the rows a query matches in their order, rather than best first.
    pub(super) fn in_order(mut self) -> Self {
        if let Some(search) = &mut self.list.search {
            search.ordered = true;
        }
        self
    }

    /// Selects `row`, when it is shown.
    pub(super) fn on_row(mut self, row: usize) -> Self {
        if let Some(position) = self.list.rows.iter().position(|&shown| shown == row) {
            self.list.selected = position;
        }
        self
    }

    /// Shows the rows without a query after `prefixes`, the tree lines of Scramjet's threaded
    /// session list, `row` giving each one's text in the width left, as for `select`.
    pub(super) fn threaded(
        mut self,
        prefixes: Vec<String>,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        let width = row_width();
        if let Shape::Rows {
            current, threaded, ..
        } = &mut self.shape
        {
            *threaded = (prefixes.iter().enumerate())
                .map(|(index, prefix)| {
                    let mark = if *current == Some(index) { 2 } else { 0 };
                    row(index, width.saturating_sub(prefix.width() + mark).max(1))
                })
                .collect();
        }
        self.list.tree = prefixes;
        self
    }

    /// Shows the list at `spot`, as one of its own keys left it, saying `status` in place of the
    /// keys' hint until the next key (`true`: a warning).
    pub(super) fn at(mut self, spot: Spot, status: Option<(String, bool)>) -> Self {
        if let Some(search) = &mut self.list.search {
            search.query = spot.query;
            self.list.filter();
        }
        let last = self.list.rows.len().saturating_sub(1);
        self.list.selected = spot.position.min(last);
        self.list.settle(true);
        self.status = status;
        self
    }

    /// Chooses the recommended option of a question after `wait`, unless `stop` is called first,
    /// and says so under the options, as Scramjet's next-step selector counts down.
    pub(super) fn counting_down(mut self, wait: Duration) -> Self {
        if let Shape::Question {
            recommended: Some(_),
            ..
        } = self.shape
        {
            self.deadline = Some(Instant::now() + wait);
        }
        self
    }

    /// Shows a next step's model and effort under a question's options, Left/Right and Shift+Tab
    /// choosing them.
    pub(super) fn routing(mut self, chosen: Route) -> Self {
        if let Shape::Question { route, .. } = &mut self.shape {
            *route = Some(chosen);
        }
        self
    }

    /// The model and effort chosen in a next step's list.
    pub(super) fn routed(&self) -> Option<Routed> {
        match &self.shape {
            Shape::Question {
                route: Some(route), ..
            } => Some(route.routed()),
            _ => None,
        }
    }

    /// Stops the countdown, as any key or click in Scramjet's selector does.
    pub(super) fn stop(&mut self) {
        self.deadline = None;
    }

    /// The option the countdown chose, once it has ended.
    pub(super) fn expired(&self) -> Option<usize> {
        let ended = self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        match &self.shape {
            Shape::Question { recommended, .. } if ended => *recommended,
            _ => None,
        }
    }

    /// How long until the countdown shows another second or ends, if it runs.
    pub(super) fn next_change(&self) -> Option<Duration> {
        let left = self.deadline?.saturating_duration_since(Instant::now());
        let within = (left.as_nanos() % 1_000_000_000) as u64;
        let wait = match within {
            0 => Duration::from_secs(1).min(left),
            within => Duration::from_nanos(within),
        };
        // A poll's milliseconds would otherwise wake it just before.
        Some(wait + Duration::from_millis(1))
    }

    fn rows(
        title: &str,
        list: List,
        current: Option<usize>,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        let width = row_width();
        let rows = (0..list.rows.len())
            .map(|index| row(index, width - if current == Some(index) { 2 } else { 0 }))
            .collect();
        Self {
            list,
            shape: Shape::Rows {
                title: title.to_owned(),
                hint: hint("select", &[], "cancel"),
                current,
                rows,
                notes: Vec::new(),
                threaded: Vec::new(),
            },
            deadline: None,
            held: false,
            acts: false,
            scoping: false,
            tabs: false,
            places: false,
            status: None,
        }
    }

    /// Whether there is nothing to choose from, whatever the query: the screen then returns
    /// nothing at once.
    pub(super) fn is_empty(&self) -> bool {
        if self.acts {
            return false;
        }
        match &self.list.search {
            Some(search) => search.texts.is_empty(),
            None => self.list.rows.is_empty(),
        }
    }

    /// Moves the selection, edits the query or asks to confirm a deletion, or returns what the
    /// key ended the list with.
    pub(super) fn key(&mut self, key: &Key) -> Option<Picked> {
        self.status = None;
        if let Shape::Tree(view, last) = &mut self.shape {
            return tree_key(view, &mut self.list, last, key);
        }
        let row = self.list.rows.get(self.list.selected).copied();
        if self.list.warned {
            if key.is(Binding::SelectConfirm) {
                self.list.warned = false;
                return Some(self.act(Act::Delete, row?));
            }
            if key.is(Binding::SelectCancel) {
                self.list.warned = false;
            }
            return None;
        }
        if let Shape::Question {
            route: Some(route), ..
        } = &mut self.shape
            && route.key(key)
        {
            return None;
        }
        let scoped = [
            (Binding::SelectConfirm, Act::Toggle),
            (Binding::ModelsEnableAll, Act::EnableAll),
            (Binding::ModelsClearAll, Act::ClearAll),
            (Binding::ModelsToggleProvider, Act::Provider),
            (Binding::ModelsReorderUp, Act::Earlier),
            (Binding::ModelsReorderDown, Act::Later),
            (Binding::ModelsSave, Act::Save),
        ];
        if self.scoping
            && let Some(&(_, act)) = scoped.iter().find(|&&(binding, _)| key.is(binding))
        {
            return row.map(|row| self.act(act, row));
        }
        let toggles = [
            (Binding::SessionToggleSort, Toggle::Sort),
            (Binding::SessionToggleNamedFilter, Toggle::Named),
            (Binding::SessionTogglePath, Toggle::Paths),
        ];
        match key {
            _ if self.tabs && key.is(Binding::Tab) => return Some(Picked::Tab(self.spot())),
            _ if self.acts && key.is(Binding::SessionRename) => {
                return row.map(|row| self.act(Act::Rename, row));
            }
            _ if self.acts
                && let Some(&(_, toggle)) =
                    toggles.iter().find(|&&(binding, _)| key.is(binding)) =>
            {
                return Some(Picked::Toggle(toggle, self.spot()));
            }
            _ if self.acts && key.is(Binding::SessionDelete) => {
                let current = match &self.shape {
                    Shape::Rows { current, .. } => *current,
                    Shape::Question { .. } | Shape::Tree(..) => None,
                };
                if row.is_some() && row == current {
                    let why = "Cannot delete the currently active session";
                    self.status = Some((why.to_owned(), true));
                } else {
                    self.list.warned = row.is_some();
                }
                return None;
            }
            _ => {}
        }
        match self.list.key(key) {
            Some(Some(row)) if self.places => Some(Picked::Placed(row, self.spot())),
            picked => picked.map(Picked::Row),
        }
    }

    fn act(&self, act: Act, row: usize) -> Picked {
        Picked::Act(act, row, self.spot())
    }

    fn spot(&self) -> Spot {
        let query =
            (self.list.search.as_ref()).map_or_else(String::new, |search| search.query.clone());
        let (position, shown) = (self.list.selected, self.list.rows.clone());
        Spot {
            query,
            position,
            shown,
        }
    }

    /// The list's lines for a screen of `(width, height)`.
    pub(super) fn lines(&self, size: (usize, usize)) -> Vec<String> {
        match &self.shape {
            Shape::Rows {
                title,
                hint,
                current,
                rows,
                notes,
                threaded,
            } => {
                let (width, height) = size;
                let note =
                    (self.list.rows.get(self.list.selected)).and_then(|&index| notes.get(index));
                let mut under = Vec::new();
                if let Some(note) = note {
                    under.push(String::new());
                    let note_width = width.saturating_sub(1).max(8) - 2;
                    for row in wrapped(note, note_width, NOTE_ROWS) {
                        under.push(format!("  {}{row}{}", style::MUTED, style::RESET));
                    }
                }
                let room = (width, height.saturating_sub(under.len()));
                let rows = match self.list.filtered || threaded.is_empty() {
                    true => rows,
                    false => threaded,
                };
                let confirm;
                let header = match &self.status {
                    _ if self.list.warned => {
                        confirm = confirm_delete();
                        (title.as_str(), confirm.as_str(), style::SELECTOR_WARNING)
                    }
                    Some((status, true)) => {
                        (title.as_str(), status.as_str(), style::SELECTOR_WARNING)
                    }
                    Some((status, false)) => {
                        (title.as_str(), status.as_str(), style::SELECTOR_DONE)
                    }
                    None => (title.as_str(), hint.as_str(), style::MUTED),
                };
                let search = (self.list.search.as_ref())
                    .map(|search| (search.query.as_str(), search.none.as_str()));
                let row = |index: usize, width| text::clip(&rows[index], width);
                let mut shown = lines(header, &self.list, search, None, *current, room, &row);
                shown.extend(under);
                shown
            }
            Shape::Tree(view, _) => {
                let tree_hint = tree_hint();
                let header = match &self.status {
                    Some((status, true)) => {
                        ("Session Tree", status.as_str(), style::SELECTOR_WARNING)
                    }
                    Some((status, false)) => {
                        ("Session Tree", status.as_str(), style::SELECTOR_DONE)
                    }
                    None => ("Session Tree", tree_hint.as_str(), style::MUTED),
                };
                let search = Some((view.query(), "No entries found"));
                let rows = view.rows();
                let row = |index: usize, width| text::clip(&rows[index].1, width);
                // As in Scramjet, the position shows with the filter, every row shown or not.
                let status = view.status();
                lines(header, &self.list, search, Some(&status), None, size, &row)
            }
            Shape::Question {
                title,
                options,
                recommended,
                route,
            } => {
                let mut tail = route.as_ref().map_or_else(Vec::new, Route::lines);
                let keys = hint("select", &[], "cancel");
                let hint = match self.deadline {
                    Some(deadline) => {
                        let left = deadline.saturating_duration_since(Instant::now());
                        let seconds = left.as_secs_f64().ceil() as u64;
                        // Shorter than Scramjet's `auto-selects recommendation in 3s`, so that
                        // the seconds show on a narrow terminal.
                        format!("{keys} · auto-selects in {seconds}s")
                    }
                    None => keys,
                };
                tail.push(hint);
                let question = (title.as_str(), options.as_slice(), *recommended);
                question_lines(question, &tail, &self.list, size)
            }
        }
    }
}

/// What a key does in Scramjet's tree selector: typing searches, Ctrl+D, Ctrl+T, Ctrl+U, Ctrl+L
/// and Ctrl+A choose a filter and Ctrl+O cycles through them, Ctrl+Left and Ctrl+Right (or Alt)
/// fold or jump between branches, Left and Right page, Shift+L asks to label the entry, Shift+T
/// shows when labels were given, and Escape clears the query before it cancels.
fn tree_key(
    view: &mut View,
    list: &mut List,
    last: &mut Option<usize>,
    key: &Key,
) -> Option<Picked> {
    let shown = view.rows().get(list.selected).map(|&(entry, _)| entry);
    let selected = shown.or(*last);
    let mut query = view.query().to_owned();
    let filter = TREE_FILTERS.iter().find(|&&(binding, _)| key.is(binding));
    match key {
        // As in Scramjet, where Shift+L and Shift+T are these keys, not text.
        _ if key.is(Binding::TreeEditLabel) => return shown.map(Picked::Label),
        _ if key.is(Binding::TreeToggleLabelTimestamp) => view.toggle_times(),
        Key::Text(text) | Key::Paste(text) => {
            let text = text
                .chars()
                .map(|ch| if ch.is_whitespace() { ' ' } else { ch });
            query.extend(text.filter(|ch| !ch.is_control()));
            view.set_query(query);
        }
        _ if key.is(Binding::DeleteCharBackward) => {
            query.pop()?;
            view.set_query(query);
        }
        _ if !query.is_empty() && key.is(Binding::SelectCancel) => view.set_query(String::new()),
        _ if let Some(&(_, filter)) = filter => match filter {
            Some(filter) => view.toggle(filter),
            None => view.cycle(key.is(Binding::TreeFilterCycleForward)),
        },
        _ if key.is(Binding::TreeFoldOrUp) || key.is(Binding::TreeUnfoldOrDown) => {
            let row = match key.is(Binding::TreeFoldOrUp) {
                true => view.fold_or_up(list.selected),
                false => view.unfold_or_down(list.selected),
            };
            *list = List::new(view.rows().len(), row, list.visible);
            return None;
        }
        // As in Scramjet, Left and Right page.
        _ if key.is(Binding::CursorLeft) => {
            return list.page(true).map(|_| Picked::Row(None));
        }
        _ if key.is(Binding::CursorRight) => {
            return list.page(false).map(|_| Picked::Row(None));
        }
        _ if key.is(Binding::SelectConfirm) => return shown.map(|entry| Picked::Row(Some(entry))),
        _ if key.is(Binding::SelectCancel) => return Some(Picked::Row(None)),
        _ => return list.key(key).map(|_| Picked::Row(None)),
    }
    reselect(view, list, last, selected);
    None
}

/// Selects `selected` among the rows `view` shows now, or its nearest shown ancestor, keeping it
/// as the entry selected last while none is shown, as Scramjet's tree does.
fn reselect(view: &View, list: &mut List, last: &mut Option<usize>, selected: Option<usize>) {
    *list = List::new(view.rows().len(), view.row_of(selected), list.visible);
    *last = (view.rows().get(list.selected).map(|&(entry, _)| entry)).or(selected);
}

struct List {
    /// The rows shown, in order: all of them, or those that match the query.
    rows: Vec<usize>,
    /// The selected row's position in `rows`.
    selected: usize,
    /// Rows shown at once, and moved by a page.
    visible: usize,
    search: Option<Box<Search>>,
    /// Whether each row is a heading, which cannot be selected; empty when none is.
    headings: Vec<bool>,
    /// The tree lines before each row without a query (`Picker::threaded`); empty when none.
    tree: Vec<String>,
    /// Whether a query chose the rows shown.
    filtered: bool,
    /// The selected row is about to be removed, and shows so.
    warned: bool,
}

struct Search {
    query: String,
    texts: Vec<Haystack>,
    /// Shown when no row matches.
    none: String,
    /// The selection without a query.
    initial: usize,
    /// Whether the rows a query matches keep their order (`Picker::in_order`).
    ordered: bool,
}

impl List {
    fn new(count: usize, initial: usize, visible: usize) -> Self {
        Self {
            rows: (0..count).collect(),
            selected: initial.min(count.saturating_sub(1)),
            visible,
            search: None,
            headings: Vec::new(),
            tree: Vec::new(),
            filtered: false,
            warned: false,
        }
    }

    fn is_heading(&self, position: usize) -> bool {
        (self.headings.get(self.rows[position])).is_some_and(|&heading| heading)
    }

    /// Moves the selection off a heading: up, or down to the first row under it.
    fn settle(&mut self, up: bool) {
        let count = self.rows.len();
        for _ in 0..count {
            if !self.is_heading(self.selected) {
                return;
            }
            self.selected = match up {
                true => (self.selected + count - 1) % count,
                false => (self.selected + 1) % count,
            };
        }
    }

    fn searching(texts: Vec<Haystack>, initial: usize, visible: usize, none: &str) -> Self {
        let mut list = Self::new(texts.len(), initial, visible);
        list.search = Some(Box::new(Search {
            query: String::new(),
            texts,
            none: none.into(),
            initial: list.selected,
            ordered: false,
        }));
        list
    }

    /// Moves the selection or edits the query, or returns what was chosen: `Some(None)` when
    /// cancelled.
    fn key(&mut self, key: &Key) -> Option<Option<usize>> {
        if let Some(search) = &mut self.search {
            let edited = match key {
                Key::Text(text) | Key::Paste(text) => {
                    let text = text
                        .chars()
                        .map(|ch| if ch.is_whitespace() { ' ' } else { ch });
                    search.query.extend(text.filter(|ch| !ch.is_control()));
                    true
                }
                _ if key.is(Binding::DeleteCharBackward) => search.query.pop().is_some(),
                _ => false,
            };
            if edited {
                self.filter();
                return None;
            }
        }
        let Some(last) = self.rows.len().checked_sub(1) else {
            return key.is(Binding::SelectCancel).then_some(None);
        };
        let up = key.is(Binding::SelectUp);
        self.selected = match key {
            _ if up && self.selected == 0 => last,
            _ if up => self.selected - 1,
            _ if key.is(Binding::SelectDown) && self.selected == last => 0,
            _ if key.is(Binding::SelectDown) => self.selected + 1,
            _ if key.is(Binding::SelectPageUp) => return self.page(true),
            _ if key.is(Binding::SelectPageDown) => return self.page(false),
            _ if key.is(Binding::SelectFirst) => 0,
            _ if key.is(Binding::SelectLast) => last,
            _ if key.is(Binding::SelectConfirm) => return Some(Some(self.rows[self.selected])),
            _ if key.is(Binding::SelectCancel) => return Some(None),
            _ => self.selected,
        };
        self.settle(up);
        None
    }

    /// Moves the selection a page up, or down.
    fn page(&mut self, up: bool) -> Option<Option<usize>> {
        let last = self.rows.len().checked_sub(1)?;
        self.selected = match up {
            true => self.selected.saturating_sub(self.visible),
            false => (self.selected + self.visible).min(last),
        };
        self.settle(false);
        None
    }

    /// Shows the rows that match the query, the best one selected, or all rows without one.
    fn filter(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        let tokens = fuzzy::parse(&search.query);
        self.filtered = !tokens.is_empty();
        if tokens.is_empty() {
            self.rows = (0..search.texts.len()).collect();
            self.selected = search.initial;
        } else {
            self.rows = fuzzy::filter(&tokens, &search.texts);
            if search.ordered {
                self.rows.sort_unstable();
            }
            self.selected = 0;
        }
    }
}

/// The selector's lines for a terminal of `(width, height)`: the title with the hint in its
/// style, the `query` being typed (with no row matching it, `none`), the rows around the selected
/// one and, when some are not shown or there is a `filter` to name after it, the selected row's
/// position.
fn lines(
    (title, hint, hint_style): (&str, &str, Style),
    list: &List,
    search: Option<(&str, &str)>,
    filter: Option<&str>,
    current: Option<usize>,
    (width, height): (usize, usize),
    row: &dyn Fn(usize, usize) -> String,
) -> Vec<String> {
    use style::{CURRENT, MUTED, RESET, SEARCH_CURSOR, SELECTED, SELECTOR_HEADING};
    use style::{SELECTOR_MARK, SELECTOR_TITLE, SELECTOR_TREE, SELECTOR_WARNING};
    // The last column stays empty, so that a full row does not move the cursor to the next.
    let width = width.saturating_sub(1).max(8);
    let shown = list.rows.len();
    let search_rows = usize::from(search.is_some());
    let visible = list
        .visible
        .min(shown)
        .min(height.saturating_sub(3 + search_rows).max(1));
    // As in Scramjet, the selected row stays in the middle where it can.
    let top = list
        .selected
        .saturating_sub(visible / 2)
        .min(shown - visible);
    let hint_room = width.saturating_sub(title.width() + 2);
    let hint = text::clip(hint, hint_room);
    let mut lines = vec![format!(
        "{SELECTOR_TITLE}{title}{RESET}  {hint_style}{hint}{RESET}"
    )];
    if let Some((query, none)) = search {
        let query = tail(query, width - 3);
        lines.push(format!("> {query}{SEARCH_CURSOR} {RESET}"));
        if shown == 0 {
            let none = text::clip(none, width - 2);
            lines.push(format!("  {MUTED}{none}{RESET}"));
        }
    }
    // Rows scrolled away from their heading show it in place of the first of them.
    let heading = (top..list.selected)
        .next()
        .filter(|&at| !list.is_heading(at))
        .and_then(|at| (0..at).rev().find(|&above| list.is_heading(above)));
    for position in top..top + visible {
        let position = match heading {
            Some(heading) if position == top => heading,
            _ => position,
        };
        let index = list.rows[position];
        let is_current = current == Some(index);
        let tree = match list.filtered {
            false => list.tree.get(index).map_or("", String::as_str),
            true => "",
        };
        let room = width - 2 - if is_current { 2 } else { 0 };
        let text = row(index, room.saturating_sub(tree.width()).max(1));
        let tree = match tree {
            "" => String::new(),
            tree => format!("{SELECTOR_TREE}{tree}{RESET}"),
        };
        let mark = if is_current {
            format!(" {CURRENT}✓{RESET}")
        } else {
            String::new()
        };
        let selected = if list.warned {
            SELECTOR_WARNING
        } else {
            SELECTED
        };
        lines.push(if list.is_heading(position) {
            format!("  {SELECTOR_HEADING}{text}{RESET}")
        } else if position == list.selected {
            // A styled row's resets would end the selection's style.
            let text = text.replace(RESET, &format!("{RESET}{selected}"));
            format!("{SELECTOR_MARK}›{RESET} {tree}{selected}{text}{RESET}{mark}")
        } else {
            format!("  {tree}{text}{mark}")
        });
    }
    if visible < shown {
        // Of the rows that can be selected.
        let rows = |end| (0..end).filter(|&at| !list.is_heading(at)).count();
        let (at, of) = (rows(list.selected + 1), rows(shown));
        lines.push(format!(
            "  {MUTED}({at}/{of}){}{RESET}",
            filter.unwrap_or("")
        ));
    } else if let Some(filter) = filter {
        let at = usize::from(shown > 0) * (list.selected + 1);
        lines.push(format!("  {MUTED}({at}/{shown}){filter}{RESET}"));
    }
    lines
}

/// A question's lines for a terminal of `(width, height)`: a rule, the title, the options around
/// the selected one, the selected one's position when some are not shown, the `tail` (the hint
/// on the keys last), a rule.
fn question_lines(
    (title, options, recommended): (&str, &[Choice], Option<usize>),
    tail: &[String],
    list: &List,
    (width, height): (usize, usize),
) -> Vec<String> {
    use style::{MUTED, QUESTION_BORDER, QUESTION_TITLE, RESET};
    let width = width.saturating_sub(1).max(8);
    let count = options.len();
    let selected = list.rows[list.selected];
    let rows: Vec<Vec<String>> = (0..count)
        .map(|index| {
            let option = &options[index];
            option_rows(option, index == selected, recommended == Some(index), width)
        })
        .collect();
    // The rules, the title and the tail; the rest is for the options and their position.
    let room = height.saturating_sub(3 + tail.len()).max(1);
    let shown = list.visible.clamp(1, count);
    let mut top = selected.saturating_sub(shown / 2).min(count - shown);
    let mut end = top + shown;
    let needed = |top: usize, end: usize| {
        let position = usize::from(top > 0 || end < count);
        rows[top..end].iter().map(Vec::len).sum::<usize>() + position
    };
    // Options farthest from the selected one go first when they do not all fit.
    while end - top > 1 && needed(top, end) > room {
        if selected - top > end - 1 - selected {
            top += 1;
        } else {
            end -= 1;
        }
    }
    let rule = format!("{QUESTION_BORDER}{}{RESET}", "─".repeat(width));
    let mut lines = vec![rule.clone(), format!("{QUESTION_TITLE}{title}{RESET}")];
    let hidden = top > 0 || end < count;
    let option_room = room.saturating_sub(usize::from(hidden)).max(1);
    let options = rows[top..end].iter().flatten().take(option_room).cloned();
    lines.extend(options);
    if hidden {
        lines.push(format!("  {MUTED}({}/{count}){RESET}", selected + 1));
    }
    for line in tail {
        lines.push(format!("{MUTED}{}{RESET}", text::clip(line, width)));
    }
    lines.push(rule);
    lines
}

/// An option as Scramjet's question list shows it: `→` before the selected one, its label, then
/// its description indented under it, each wrapped to at most `OPTION_ROWS` rows.
fn option_rows(option: &Choice, selected: bool, recommended: bool, width: usize) -> Vec<String> {
    use style::{MUTED, NONE, QUESTION_SELECTED, RESET};
    let style = |text: String, other: Style| match (selected, other) {
        (true, _) => format!("{QUESTION_SELECTED}{text}{RESET}"),
        (false, NONE) => text,
        (false, other) => format!("{other}{text}{RESET}"),
    };
    let mark = if selected { "→ " } else { "  " };
    let mut rows: Vec<String> = wrapped(&option.label, width - 2, OPTION_ROWS)
        .into_iter()
        .enumerate()
        .map(|(row, text)| {
            style(
                format!("{}{text}", if row == 0 { mark } else { "  " }),
                NONE,
            )
        })
        .collect();
    if let Some(description) = &option.description {
        // As in Scramjet, only a recommended option's description says it is recommended.
        let text = match recommended {
            true => format!("[recommended] {description}"),
            false => description.clone(),
        };
        let width = width - DESCRIPTION_INDENT.len();
        for text in wrapped(&text, width, OPTION_ROWS) {
            rows.push(style(format!("{DESCRIPTION_INDENT}{text}"), MUTED));
        }
    }
    rows
}

/// `text` wrapped between words to `width` columns, in at most `most` rows, the last one ending
/// with `…` when there is more.
fn wrapped(text: &str, width: usize, most: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut wrap = text::Wrap::new(width.max(1));
        let line = wrap.push(&text::printable(line)) + &wrap.finish();
        rows.extend(line.split('\n').map(str::to_owned));
    }
    if rows.len() > most {
        rows.truncate(most);
        let last = &mut rows[most - 1];
        *last = text::clip(&format!("{last}…"), width);
    }
    rows
}

/// The end of `text` that fits in `width` columns, so that the end of a long query or path shows.
pub(super) fn tail(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (at, ch) in text.char_indices().rev() {
        used += ch.width().unwrap_or(0);
        if used > width {
            return &text[at + ch.len_utf8()..];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::keys::tests::key;

    /// A list's hint with the default keys, which the tests run with.
    const HINT: &str = "↑↓ move · Enter select · Esc cancel";

    fn searched(list: &List) -> Option<(&str, &str)> {
        (list.search.as_ref()).map(|search| (search.query.as_str(), search.none.as_str()))
    }

    fn plain(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| text::printable(&line))
            .collect()
    }

    fn haystacks(texts: &[String]) -> Vec<Haystack> {
        texts.iter().map(|text| Haystack::new(text)).collect()
    }

    #[test]
    fn keys_move_the_selection_around_and_choose_or_cancel() {
        let mut list = List::new(25, 0, 10);
        let mut moved = Vec::new();
        for key in [
            key("up"),
            key("up"),
            key("down"),
            key("down"),
            key("pageUp"),
            key("pageUp"),
            key("pageDown"),
            key("home"),
            key("end"),
            Key::Text("j".into()),
        ] {
            assert_eq!(list.key(&key), None);
            moved.push(list.selected);
        }
        assert_eq!(moved, [24, 23, 24, 0, 0, 0, 10, 0, 24, 24]);
        assert_eq!(list.key(&key("enter")), Some(Some(24)));
        assert_eq!(list.key(&key("escape")), Some(None));
        assert_eq!(list.key(&key("ctrl+c")), Some(None));
    }

    #[test]
    fn a_styled_row_stays_bold_where_selected_and_is_clipped_by_its_text() {
        let styled = "\x1b[36muser: \x1b[0mhello world";
        let row = |_: usize, width: usize| text::clip(styled, width);
        let shown = lines(
            ("Pick", HINT, style::MUTED),
            &List::new(2, 0, 10),
            searched(&List::new(2, 0, 10)),
            None,
            None,
            (12, 9),
            &row,
        );
        let (bold, end) = ("\x1b[1m", "\x1b[0m");
        assert_eq!(
            shown[1],
            format!("\x1b[36m›{end} {bold}\x1b[36muser: {end}{bold}he…{end}{bold}{end}")
        );
        assert_eq!(shown[2], format!("  \x1b[36muser: {end}he…{end}"));
    }

    #[test]
    fn shows_ten_rows_around_the_selection_with_its_position() {
        let row = |index: usize, width: usize| text::clip(&format!("row {index}"), width);
        let list = |selected| List::new(25, selected, 10);
        let shown = plain(lines(
            ("Pick", HINT, style::MUTED),
            &list(12),
            searched(&list(12)),
            None,
            Some(9),
            (50, 24),
            &row,
        ));
        assert_eq!(
            shown,
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "  row 7",
                "  row 8",
                "  row 9 ✓",
                "  row 10",
                "  row 11",
                "› row 12",
                "  row 13",
                "  row 14",
                "  row 15",
                "  row 16",
                "  (13/25)",
            ]
        );
        let first = plain(lines(
            ("Pick", HINT, style::MUTED),
            &list(0),
            searched(&list(0)),
            None,
            None,
            (50, 24),
            &row,
        ));
        assert_eq!(first[1], "› row 0");
        assert_eq!(first[10], "  row 9");
        let last = plain(lines(
            ("Pick", HINT, style::MUTED),
            &list(24),
            searched(&list(24)),
            None,
            None,
            (50, 24),
            &row,
        ));
        assert_eq!(last[1], "  row 15");
        assert_eq!(last[10], "› row 24");
        // A short terminal shows fewer rows; a narrow one cuts the hint and the rows.
        let short = plain(lines(
            ("Pick", HINT, style::MUTED),
            &list(3),
            searched(&list(3)),
            None,
            None,
            (12, 6),
            &row,
        ));
        assert_eq!(
            short,
            ["Pick  ↑↓ m…", "  row 2", "› row 3", "  row 4", "  (4/25)"]
        );
        let few = List::new(2, 1, 10);
        assert_eq!(
            plain(lines(
                ("Pick", HINT, style::MUTED),
                &few,
                searched(&few),
                None,
                None,
                (50, 24),
                &row
            )),
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "  row 0",
                "› row 1"
            ]
        );
    }

    #[test]
    fn headings_group_rows_and_are_passed_over() {
        // Two groups: a heading, then three rows under each.
        let headings: Vec<bool> = (0..8).map(|index| index % 4 == 0).collect();
        let row = |index: usize, width: usize| {
            let text = match index % 4 {
                0 => format!("After {}", index / 4),
                at => format!("  row {}.{at}", index / 4),
            };
            text::clip(&text, width)
        };
        let mut list = List::new(8, 0, 3);
        list.headings = headings;
        list.settle(false);
        assert_eq!(list.selected, 1);
        let mut moved = Vec::new();
        for key in [
            key("up"),
            key("down"),
            key("up"),
            key("home"),
            key("down"),
            key("down"),
            key("down"),
            key("pageUp"),
        ] {
            assert_eq!(list.key(&key), None);
            moved.push(list.selected);
        }
        assert_eq!(moved, [7, 1, 7, 1, 2, 3, 5, 2]);
        list.selected = 6;
        // The first rows shown lost their heading above: it shows in the first one's place.
        assert_eq!(
            plain(lines(
                ("Pick", HINT, style::MUTED),
                &list,
                searched(&list),
                None,
                None,
                (40, 24),
                &row
            )),
            [
                "Pick  ↑↓ move · Enter select · Esc can…",
                "  After 1",
                "›   row 1.2",
                "    row 1.3",
                "  (5/6)",
            ]
        );
    }

    #[test]
    fn question_options_show_their_descriptions_under_them() {
        let option = |label: &str, description: Option<&str>| Choice {
            label: label.into(),
            description: description.map(str::to_owned),
            value: String::new(),
        };
        let options = [
            option("Ship it", Some("Commit and push the change now")),
            option("Wait", None),
            option("Ask again later", Some(&"long words ".repeat(20))),
        ];
        let list = List::new(3, 2, 8);
        assert_eq!(
            plain(question_lines(
                ("Choose", &options, Some(2)),
                &[HINT.to_owned()],
                &list,
                (31, 24)
            )),
            [
                "─".repeat(30).as_str(),
                "Choose",
                "  Ship it",
                "     Commit and push the",
                "     change now",
                "  Wait",
                "→ Ask again later",
                "     [recommended] long words",
                "     long words long words",
                "     long words long words",
                "     long words long words…",
                "↑↓ move · Enter select · Esc …",
                "─".repeat(30).as_str(),
            ]
        );
        // A short terminal keeps the selected option and the ones nearest it, with its position.
        let first = List::new(3, 0, 8);
        let short = plain(question_lines(
            ("Choose", &options, None),
            &[HINT.to_owned()],
            &first,
            (31, 8),
        ));
        assert_eq!(
            short[1..6],
            [
                "Choose",
                "→ Ship it",
                "     Commit and push the",
                "     change now",
                "  (1/3)"
            ]
        );
    }

    #[test]
    fn a_countdown_chooses_the_recommended_option_unless_stopped() {
        let options = ["/rs:loop", "/rs:steer"].map(|label| Choice {
            label: label.into(),
            description: None,
            value: String::new(),
        });
        let hint = |picker: &Picker| {
            let lines = plain(picker.lines((80, 24)));
            lines[lines.len() - 2].clone()
        };
        let mut picker = Picker::question("Select next step", &options, Some(1))
            .counting_down(Duration::from_secs(3));
        assert_eq!(
            hint(&picker),
            "↑↓ move · Enter select · Esc cancel · auto-selects in 3s"
        );
        assert_eq!(picker.expired(), None);
        assert!(picker.next_change().unwrap() <= Duration::from_millis(1001));
        picker.stop();
        assert_eq!(hint(&picker), HINT);
        assert_eq!((picker.expired(), picker.next_change()), (None, None));

        let ended =
            Picker::question("Select next step", &options, Some(1)).counting_down(Duration::ZERO);
        assert_eq!(ended.expired(), Some(1));
        // Without a recommendation there is nothing to count down to.
        let unrecommended =
            Picker::question("Select next step", &options, None).counting_down(Duration::ZERO);
        assert_eq!(
            (unrecommended.expired(), unrecommended.next_change()),
            (None, None)
        );
    }

    #[test]
    fn a_next_steps_list_chooses_the_model_and_effort_the_step_runs_with() {
        let model = |id: &str, name: Option<&str>, levels: &[&str]| ModelOption {
            id: id.into(),
            name: name.map(str::to_owned),
            provider: None,
            reasoning: !levels.is_empty(),
            levels: levels.iter().map(|level| level.to_string()).collect(),
            window: None,
            output: None,
            images: true,
            rate: None,
        };
        let models = vec![
            model("gpt-6-luna", Some("GPT-6 Luna"), &["none", "low", "high"]),
            model(
                "claude-opus-5-5",
                Some("Claude Opus 5.5"),
                &["low", "medium", "max"],
            ),
            model("claude-haiku-4-5", None, &[]),
        ];
        let options = ["/rs:loop", "/rs:steer"].map(|label| Choice {
            label: label.into(),
            description: None,
            value: String::new(),
        });
        let route = Route::new(models.clone(), 0, Some("none".into()));
        let mut picker = Picker::question("Select next step", &options, Some(1)).routing(route);
        // The lines between the options and the hint.
        let tail = |picker: &Picker| {
            let lines = plain(picker.lines((80, 24)));
            lines[4..lines.len() - 2].to_vec()
        };
        assert_eq!(
            tail(&picker),
            [
                "model: GPT-6 Luna (gpt-6-luna) · ←→ change",
                "effort: off · Shift+Tab cycle"
            ]
        );
        assert_eq!(picker.routed(), Some(Routed::default()));
        // The next model has the session's effort fitted to its levels; Shift+Tab cycles them.
        assert_eq!(picker.key(&key("right")), None);
        assert_eq!(
            tail(&picker),
            [
                "model: Claude Opus 5.5 (claude-opus-5-5) · ←→ change",
                "effort: low · Shift+Tab cycle"
            ]
        );
        assert_eq!(picker.key(&key("shift+tab")), None);
        assert_eq!(tail(&picker)[1], "effort: medium · Shift+Tab cycle");
        // Left wraps around to the last, which does not reason: nothing to cycle there.
        for key in [key("left"), key("left"), key("shift+tab")] {
            assert_eq!(picker.key(&key), None);
        }
        assert_eq!(
            tail(&picker),
            ["model: claude-haiku-4-5 · ←→ change", "effort: off"]
        );
        for key in [key("right"), key("right")] {
            assert_eq!(picker.key(&key), None);
        }
        assert_eq!(picker.key(&key("enter")), Some(Picked::Row(Some(1))));
        let chosen = Routed {
            model: Some("claude-opus-5-5".into()),
            effort: Some("medium".into()),
        };
        assert_eq!(picker.routed(), Some(chosen));
        // Back at the session's model, only the effort changes.
        picker.key(&key("left"));
        let effort = Some("medium".to_owned());
        let only_effort = Routed {
            model: None,
            effort,
        };
        assert_eq!(picker.routed(), Some(only_effort));
        // With one model there is no model line.
        let alone = model("gpt-6-luna", None, &["low", "high"]);
        let route = Route::new(vec![alone], 0, Some("high".into()));
        let mut single = Picker::question("Select next step", &options, None).routing(route);
        assert_eq!(tail(&single), ["effort: high · Shift+Tab cycle"]);
        assert_eq!(single.key(&key("right")), None);
        assert_eq!(single.routed(), Some(Routed::default()));
        // Other questions have neither.
        let question = Picker::question("Which?", &options, None);
        assert_eq!(question.routed(), None);
        assert_eq!(plain(question.lines((80, 24))).len(), 6);
    }

    #[test]
    fn scoped_models_keys_act_on_the_selected_row_with_the_rows_shown() {
        let texts = ["gpt-6-luna", "gpt-6-terra", "claude-haiku-5-5"].map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let picker = || Picker::search("Models", 10, haystacks(&texts), 0, None, "None", row);
        let mut list = picker().scoping();
        let spot = |query: &str, position, shown: &[usize]| Spot {
            query: query.into(),
            position,
            shown: shown.to_vec(),
        };
        let acts = [
            (key("enter"), Act::Toggle),
            (key("ctrl+a"), Act::EnableAll),
            (key("ctrl+x"), Act::ClearAll),
            (key("ctrl+p"), Act::Provider),
            (key("alt+up"), Act::Earlier),
            (key("alt+down"), Act::Later),
            (key("ctrl+s"), Act::Save),
        ];
        for (key, act) in acts {
            let acted = Some(Picked::Act(act, 0, spot("", 0, &[0, 1, 2])));
            assert_eq!(list.key(&key), acted, "{key:?}");
        }
        for key in [Key::Text("gpt".into()), key("down")] {
            assert_eq!(list.key(&key), None);
        }
        let acted = Some(Picked::Act(Act::Toggle, 1, spot("gpt", 1, &[0, 1])));
        assert_eq!(list.key(&key("enter")), acted);
        // Nothing shown, nothing to act on; Escape closes.
        assert_eq!(list.key(&Key::Text("zzz".into())), None);
        assert_eq!(list.key(&key("ctrl+a")), None);
        assert_eq!(list.key(&key("escape")), Some(Picked::Row(None)));
        // Elsewhere Ctrl+A goes to the first row, as Home does, and Enter chooses.
        let mut plain_list = picker();
        plain_list.key(&key("down"));
        assert_eq!(plain_list.key(&key("ctrl+a")), None);
        assert_eq!(plain_list.key(&key("enter")), Some(Picked::Row(Some(0))));
        // A list shown again selects the row acted on, where it is now.
        let again = picker().scoping().on_row(2);
        assert_eq!(plain(again.lines((40, 24)))[4], "› claude-haiku-5-5");
    }

    #[test]
    fn tab_ends_a_tabbing_list_where_it_is_even_with_no_row_shown() {
        let texts = ["gpt-6-luna", "gpt-6-terra", "claude-haiku-5-5"].map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let picker = || Picker::search("Models", 10, haystacks(&texts), 0, None, "None", row);
        let tab = key("tab");
        let mut list = picker().tabbing();
        list.key(&Key::Text("gpt".into()));
        list.key(&key("down"));
        let spot = |query: &str, position, shown: Vec<usize>| Spot {
            query: query.into(),
            position,
            shown,
        };
        assert_eq!(
            list.key(&tab),
            Some(Picked::Tab(spot("gpt", 1, vec![0, 1])))
        );
        list.key(&Key::Text("zzz".into()));
        assert_eq!(list.key(&tab), Some(Picked::Tab(spot("gptzzz", 0, vec![]))));
        // Elsewhere Tab is typed into the query, as a space.
        let mut plain_list = picker();
        assert_eq!(plain_list.key(&tab), None);
        assert_eq!(plain_list.key(&key("escape")), Some(Picked::Row(None)));
    }

    #[test]
    fn a_placing_list_ends_with_where_it_was_and_shows_there_again() {
        let texts = ["gpt-6-luna", "gpt-6-terra", "claude-haiku-5-5"].map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let picker = || Picker::search("Models", 10, haystacks(&texts), 0, None, "None", row);
        let mut list = picker().placing();
        list.key(&Key::Text("gpt".into()));
        list.key(&key("down"));
        let Some(Picked::Placed(1, spot)) = list.key(&key("enter")) else {
            panic!("not placed");
        };
        assert_eq!((spot.query.as_str(), spot.position), ("gpt", 1));
        let again = picker().at(spot, None).on_row(1);
        let lines = plain(again.lines((40, 24)));
        assert_eq!(lines[1..], ["> gpt ", "  gpt-6-luna", "› gpt-6-terra"]);
        // Cancelling is still cancelling.
        let mut list = picker().placing();
        assert_eq!(list.key(&key("escape")), Some(Picked::Row(None)));
    }

    #[test]
    fn session_keys_rename_and_delete_after_confirming_with_tree_lines_without_a_query() {
        let texts = ["root parser", "fork parser", "other"].map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let picker = || {
            Picker::search("Resume", 10, haystacks(&texts), 0, Some(2), "None", row)
                .threaded(vec![String::new(), "   └─ ".into(), String::new()], row)
                .acting()
        };
        let shown = |picker: &Picker| plain(picker.lines((60, 24)));
        let mut list = picker();
        assert_eq!(
            shown(&list)[2..],
            ["› root parser", "     └─ fork parser", "  other ✓"]
        );
        assert_eq!(list.key(&key("down")), None);
        let place = |query: &str, position, shown: &[usize]| Spot {
            query: query.into(),
            position,
            shown: shown.to_vec(),
        };
        assert_eq!(
            list.key(&key("ctrl+r")),
            Some(Picked::Act(Act::Rename, 1, place("", 1, &[0, 1, 2])))
        );
        // Ctrl+D asks first; Escape takes the question back, and other keys wait for an answer.
        assert_eq!(list.key(&key("ctrl+d")), None);
        let asked = shown(&list);
        assert_eq!(
            asked[0],
            "Resume  Delete session? Enter confirm · Esc cancel"
        );
        for key in [key("down"), Key::Text("x".into()), key("ctrl+r")] {
            assert_eq!(list.key(&key), None);
        }
        assert_eq!(list.key(&key("escape")), None);
        assert_eq!(
            shown(&list)[0],
            "Resume  ↑↓ move · Enter select · Esc cancel"
        );
        assert_eq!(shown(&list)[3], "›    └─ fork parser");
        // A query shows the rows without their tree lines; the current row is not deleted.
        for key in [Key::Text("pars".into()), key("down"), key("ctrl+d")] {
            assert_eq!(list.key(&key), None);
        }
        assert_eq!(
            list.key(&key("enter")),
            Some(Picked::Act(Act::Delete, 1, place("pars", 1, &[0, 1])))
        );
        let mut current = picker();
        current.key(&key("up"));
        assert_eq!(current.key(&key("ctrl+d")), None);
        let refused = shown(&current);
        assert!(
            refused[0].ends_with("Cannot delete the currently active session"),
            "{refused:?}"
        );
        assert_eq!(current.key(&key("up")), None);
        assert_eq!(
            shown(&current)[0],
            "Resume  ↑↓ move · Enter select · Esc cancel"
        );
        // Shown again where it was, with what the key did; without `acting` the keys do nothing.
        let again = picker().at(
            place("pars", 5, &[]),
            Some(("Session deleted".into(), false)),
        );
        let lines = shown(&again);
        let unmatched = picker().at(place("zzz", 0, &[]), None);
        assert!(!unmatched.is_empty());
        assert_eq!(shown(&unmatched)[2], "  None");
        assert_eq!(lines[..2], ["Resume  Session deleted", "> pars "]);
        assert_eq!(lines[2..], ["  root parser", "› fork parser"]);
        let mut plain_list = Picker::search("Resume", 10, haystacks(&texts), 0, None, "None", row);
        assert_eq!(plain_list.key(&key("ctrl+r")), None);
        assert_eq!(plain_list.key(&key("ctrl+d")), None);
        assert_eq!(plain_list.key(&key("enter")), Some(Picked::Row(Some(0))));
    }

    #[test]
    fn session_view_keys_end_the_list_where_it_is_and_a_query_can_keep_the_rows_order() {
        let texts = ["parser one", "other", "parse"].map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let picker = || Picker::search("Resume", 10, haystacks(&texts), 0, None, "None", row);
        let place = |query: &str, position, shown: &[usize]| Spot {
            query: query.into(),
            position,
            shown: shown.to_vec(),
        };
        let mut list = picker().acting();
        list.key(&Key::Text("pars".into()));
        // Best first: `parse` is the shorter match.
        assert_eq!(list.spot(), place("pars", 0, &[2, 0]));
        for (key, toggle) in [
            (key("ctrl+s"), Toggle::Sort),
            (key("ctrl+n"), Toggle::Named),
            (key("ctrl+p"), Toggle::Paths),
        ] {
            let ended = Some(Picked::Toggle(toggle, place("pars", 0, &[2, 0])));
            assert_eq!(list.key(&key), ended);
        }
        // In order, the first row matching comes first.
        let mut ordered = picker().acting().in_order();
        ordered.key(&Key::Text("pars".into()));
        assert_eq!(ordered.spot(), place("pars", 0, &[0, 2]));
        // Elsewhere Ctrl+N moves down and the other keys do nothing.
        let mut plain_list = picker();
        for key in [key("ctrl+n"), key("ctrl+s"), key("ctrl+p")] {
            assert_eq!(plain_list.key(&key), None);
        }
        assert_eq!(plain_list.key(&key("enter")), Some(Picked::Row(Some(1))));
        // A session list without rows shows, saying so, for Ctrl+N to bring them back.
        let none = |_: usize, _: usize| String::new();
        let empty = || Picker::search("Resume", 10, Vec::new(), 0, None, "No named", none);
        assert!(empty().is_empty());
        let mut named = empty().acting();
        assert!(!named.is_empty());
        assert_eq!(plain(named.lines((60, 24)))[2], "  No named");
        assert_eq!(named.key(&key("enter")), None);
        let ended = Some(Picked::Toggle(Toggle::Named, place("", 0, &[])));
        assert_eq!(named.key(&key("ctrl+n")), ended);
    }

    #[test]
    fn the_tree_searches_filters_and_folds_and_returns_the_entry_chosen() {
        let view = View::new(super::super::tree::sample(), false, Filter::Default);
        let mut picker = Picker::tree(view, 4, Some(6));
        let text = |picker: &Picker| plain(picker.lines((80, 24)));
        let keys = "↑↓ move · ←→ page · Ctrl+←→ fold/branch · Shift+L label · \
            Ctrl+D/T/U/L/A filters (Ctrl+O cycle) · Shift+T label time · Esc cancel";
        assert_eq!(tree_hint(), keys);
        let hint = format!("Session Tree  {}", text::clip(keys, 79 - 14));
        assert_eq!(
            text(&picker),
            [
                hint.as_str(),
                ">  ",
                "  ├⊟ • user: add tests",
                "  │     • [bash: cargo test]",
                "› │     • assistant: tests pass",
                "  └⊟ user: other way",
                "  (6/8)",
            ]
        );
        assert!(picker.takes(&key("ctrl+o")) && picker.takes(&key("ctrl+t")));
        assert_eq!(picker.key(&key("ctrl+u")), None);
        assert_eq!(
            text(&picker)[2..],
            [
                "  • user: fix the bug",
                "› ├─ • user: add tests",
                "  └─ user: other way",
                "  (2/3) [user]"
            ]
        );
        // Through the labeled entries, none here, the selection stays on its entry.
        assert_eq!(picker.key(&key("ctrl+o")), None);
        assert_eq!(
            text(&picker)[1..],
            [">  ", "  No entries found", "  (0/0) [labeled]"]
        );
        assert_eq!(picker.key(&key("enter")), None);
        assert_eq!(picker.key(&key("ctrl+o")), None);
        assert_eq!(text(&picker).last().unwrap(), "  (5/9) [all]");
        assert_eq!(picker.key(&key("ctrl+d")), None);
        for key in ["o", "t", "h"] {
            assert_eq!(picker.key(&Key::Text(key.into())), None);
        }
        assert_eq!(
            text(&picker)[1..],
            ["> oth ", "› user: other way", "  (1/1)"]
        );
        // Escape clears the query, keeping the row; again, it cancels.
        assert_eq!(picker.key(&key("escape")), None);
        assert_eq!(text(&picker)[1], ">  ");
        assert_eq!(picker.key(&key("enter")), Some(Picked::Row(Some(7))));
        assert_eq!(picker.key(&key("alt+left")), None);
        assert_eq!(text(&picker).last().unwrap(), "  (7/7)");
        assert_eq!(picker.key(&key("left")), None);
        assert_eq!(picker.key(&key("enter")), Some(Picked::Row(Some(3))));
        assert_eq!(picker.key(&key("escape")), Some(Picked::Row(None)));
        let mut empty = Picker::tree(
            View::new(super::super::tree::sample(), false, Filter::Default),
            4,
            None,
        );
        for key in ["z", "z"] {
            empty.key(&Key::Text(key.into()));
        }
        assert_eq!(
            text(&empty)[1..],
            ["> zz ", "  No entries found", "  (0/0)"]
        );
        assert_eq!(empty.key(&key("enter")), None);
    }

    #[test]
    fn shift_l_asks_to_label_the_entry_and_the_list_shows_it_once_given() {
        let view = View::new(super::super::tree::sample(), false, Filter::Default);
        let mut picker = Picker::tree(view, 4, Some(6));
        let text = |picker: &Picker| plain(picker.lines((80, 24)));
        assert_eq!(picker.key(&Key::Text("L".into())), Some(Picked::Label(6)));
        assert_eq!(picker.tree_label(6), None);
        let label = Label {
            text: "green".into(),
            time: 0,
        };
        picker.relabel(6, Some(Some(label)), None);
        assert_eq!(picker.tree_label(6), Some("green"));
        assert_eq!(text(&picker)[4], "› │     • [green] assistant: tests pass");
        // Ctrl+L shows the labeled entries, and again the others.
        assert_eq!(picker.key(&key("ctrl+l")), None);
        assert_eq!(
            text(&picker)[1..],
            [
                ">  ",
                "› • [green] assistant: tests pass",
                "  (1/1) [labeled]"
            ]
        );
        assert_eq!(picker.key(&Key::Text("T".into())), None);
        assert_eq!(text(&picker)[3], "  (1/1) [labeled] [+label time]");
        assert_eq!(picker.key(&key("ctrl+l")), None);
        assert_eq!(text(&picker)[6], "  (6/8) [+label time]");
        // Taken away while the labeled entries show, the selection goes back to its entry after.
        picker.key(&key("ctrl+l"));
        picker.relabel(6, Some(None), None);
        assert_eq!(text(&picker)[3], "  (0/0) [labeled] [+label time]");
        assert_eq!(picker.key(&Key::Text("L".into())), None);
        picker.key(&key("ctrl+l"));
        assert_eq!(text(&picker)[6], "  (6/8) [+label time]");
        // A label that could not be saved is not shown; why shows until the next key.
        picker.relabel(
            6,
            None,
            Some(("Failed to save the label: full".into(), true)),
        );
        assert_eq!(
            text(&picker)[0],
            "Session Tree  Failed to save the label: full"
        );
        assert_eq!(picker.tree_label(6), None);
        picker.key(&key("up"));
        assert!(text(&picker)[0].starts_with("Session Tree  ↑↓ move"));
        // Lowercase is text, as in Scramjet.
        assert_eq!(picker.key(&Key::Text("l".into())), None);
        assert_eq!(text(&picker)[1], "> l ");
    }

    #[test]
    fn typing_shows_the_matching_rows_best_first() {
        let texts = [
            "fix the parser",
            "write a haiku",
            "parser tests",
            "release notes",
        ]
        .map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let mut list = List::searching(haystacks(&texts), 1, 10, "No matching rows");
        let shown = |list: &List| {
            plain(lines(
                ("Pick", HINT, style::MUTED),
                list,
                searched(list),
                None,
                Some(0),
                (50, 24),
                &row,
            ))
        };
        assert_eq!(shown(&list)[1..3], [">  ", "  fix the parser ✓"]);
        assert_eq!(shown(&list)[3], "› write a haiku");
        for key in [Key::Text("pa".into()), Key::Text("rs".into())] {
            assert_eq!(list.key(&key), None);
        }
        assert_eq!(
            shown(&list),
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "> pars ",
                "› parser tests",
                "  fix the parser ✓"
            ]
        );
        assert_eq!(list.key(&key("down")), None);
        assert_eq!(list.key(&key("enter")), Some(Some(0)));
        // Nothing matches: Enter and the arrows do nothing, Escape still cancels.
        assert_eq!(list.key(&Key::Paste("\nzz".into())), None);
        assert_eq!(shown(&list)[1..], ["> pars zz ", "  No matching rows"]);
        for key in [key("enter"), key("up"), key("end")] {
            assert_eq!(list.key(&key), None);
        }
        for _ in 0..3 {
            assert_eq!(list.key(&key("backspace")), None);
        }
        assert_eq!(list.rows, [2, 0]);
        // Without a query every row is back, with the first selection.
        for _ in 0..4 {
            assert_eq!(list.key(&key("backspace")), None);
        }
        assert_eq!((&list.rows[..], list.selected), (&[0, 1, 2, 3][..], 1));
        assert_eq!(list.key(&key("backspace")), None);
        assert_eq!(list.key(&key("escape")), Some(None));
        // A query wider than the line shows its end.
        list.key(&Key::Text("a very long query".into()));
        let narrow = plain(lines(
            ("Pick", HINT, style::MUTED),
            &list,
            searched(&list),
            None,
            None,
            (12, 24),
            &row,
        ));
        assert_eq!(narrow[1], "> ng query ");
    }
}
