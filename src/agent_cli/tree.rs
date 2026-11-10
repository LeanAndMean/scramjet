//! `/tree`'s list, as Scramjet's tree selector shows a session: a chain of entries stays in one
//! column, each branch point indents its children under `├─`/`└─` connectors with `│` gutters,
//! the branch with the current entry comes first, `• ` marks that entry's path, and an entry's
//! label shows before it. Its filters, its query and the branches folded choose the entries
//! shown; an entry whose parent is hidden hangs from its nearest shown ancestor.

use super::style::{RESET, SELECTOR_TREE, Style, TREE_ASSISTANT, TREE_COMPACTION, TREE_ERROR};
use super::style::{TREE_LABEL, TREE_LABEL_TIME, TREE_OTHER, TREE_PATH, TREE_SUMMARY, TREE_USER};
use super::text;
use scramjet_rs::agent::{Label, SessionTree, TreeEntry, TreeItem};
use std::collections::{HashMap, HashSet};

/// Which entries the tree shows, as Scramjet's filters choose them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Filter {
    /// All but model and effort changes, unless one is the current entry.
    Default,
    /// Those but tool results.
    NoTools,
    /// Prompts only.
    UserOnly,
    /// Entries with a label.
    Labeled,
    All,
}

impl Filter {
    /// Ctrl+O's order, as Scramjet's.
    pub(super) const CYCLE: [Self; 5] = [
        Self::Default,
        Self::NoTools,
        Self::UserOnly,
        Self::Labeled,
        Self::All,
    ];

    /// What the list's position line says after it, as Scramjet's.
    fn label(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::NoTools => " [no-tools]",
            Self::UserOnly => " [user]",
            Self::Labeled => " [labeled]",
            Self::All => " [all]",
        }
    }

    /// Its name in the settings' `treeFilterMode`, as Scramjet's.
    pub(super) fn setting(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::NoTools => "no-tools",
            Self::UserOnly => "user-only",
            Self::Labeled => "labeled-only",
            Self::All => "all",
        }
    }

    fn shows(self, entry: &TreeEntry, current: bool) -> bool {
        let item = &entry.item;
        let change = matches!(item, TreeItem::Model(_) | TreeItem::Effort(_));
        match self {
            Self::Default => !change || current,
            Self::NoTools => (!change || current) && !matches!(item, TreeItem::Tool(..)),
            Self::UserOnly => matches!(item, TreeItem::Prompt(_)),
            Self::Labeled => entry.label.is_some(),
            Self::All => true,
        }
    }
}

/// Where an ancestor's connector was drawn, and whether its line continues there (`│`).
#[derive(Clone)]
struct Gutter {
    position: usize,
    show: bool,
}

struct Row {
    entry: usize,
    indent: usize,
    connector: bool,
    last: bool,
    gutters: Vec<Gutter>,
    /// A root among several, drawn without a connector.
    virtual_root_child: bool,
}

/// A session's tree as `/tree` shows it: the entries its filter, query and folded branches leave,
/// laid out.
pub(super) struct View {
    tree: SessionTree,
    color: bool,
    filter: Filter,
    query: String,
    /// Whether labels show when they were given (Shift+T).
    times: bool,
    /// The entries whose descendants are hidden, as indices into `tree.entries`.
    folded: HashSet<usize>,
    /// Each entry's parent, as an index into `tree.entries`.
    parents: Vec<Option<usize>>,
    /// Each entry's place when all are shown, which the entries shown keep.
    order: Vec<usize>,
    /// The entries shown, in order, each with its text.
    rows: Vec<(usize, String)>,
    /// The entries shown under each shown entry (`None`: the roots), and each one's shown parent.
    children: HashMap<Option<usize>, Vec<usize>>,
    shown_parents: HashMap<usize, Option<usize>>,
}

impl View {
    /// `tree` with `filter`, styled when `color`.
    pub(super) fn new(tree: SessionTree, color: bool, filter: Filter) -> Self {
        let lines: HashMap<usize, usize> = (tree.entries.iter().enumerate())
            .map(|(index, entry)| (entry.line, index))
            .collect();
        let parents = (tree.entries.iter())
            .map(|entry| entry.parent.and_then(|line| lines.get(&line).copied()))
            .collect();
        let mut view = Self {
            tree,
            color,
            filter,
            query: String::new(),
            times: false,
            folded: HashSet::new(),
            parents,
            order: Vec::new(),
            rows: Vec::new(),
            children: HashMap::new(),
            shown_parents: HashMap::new(),
        };
        let all: Vec<usize> = (0..view.tree.entries.len()).collect();
        let mut order = vec![0; all.len()];
        let laid_out = layout(&view.tree.entries, &view.parents, &all);
        for (place, row) in laid_out.rows.iter().enumerate() {
            order[row.entry] = place;
        }
        view.order = order;
        view.refresh();
        view
    }

    /// The entries shown, in order, as indices into the tree's entries, with each row's text.
    pub(super) fn rows(&self) -> &[(usize, String)] {
        &self.rows
    }

    /// The entry the conversation ends at, nearest it on its path when it is not listed.
    pub(super) fn current(&self) -> Option<usize> {
        let line = self.tree.current?;
        self.tree
            .entries
            .iter()
            .position(|entry| entry.line == line)
    }

    /// The row of `entry`, or of its nearest shown ancestor; else the last row, as in Scramjet.
    pub(super) fn row_of(&self, entry: Option<usize>) -> usize {
        let mut at = entry;
        while let Some(index) = at {
            if let Some(row) = self.rows.iter().position(|&(shown, _)| shown == index) {
                return row;
            }
            at = self.parents[index];
        }
        self.rows.len().saturating_sub(1)
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    /// The position line's end: which filter shows the rows, and whether labels show their times.
    pub(super) fn status(&self) -> String {
        let times = if self.times { " [+label time]" } else { "" };
        format!("{}{times}", self.filter.label())
    }

    /// The label of `entry`, an index into the tree's entries.
    pub(super) fn label(&self, entry: usize) -> Option<&str> {
        let label = self.tree.entries.get(entry)?.label.as_ref();
        label.map(|label| label.text.as_str())
    }

    /// Shows `label` on `entry`, or none, as Scramjet's tree does once it is saved.
    pub(super) fn set_label(&mut self, entry: usize, label: Option<Label>) {
        if let Some(entry) = self.tree.entries.get_mut(entry) {
            entry.label = label;
            self.refresh();
        }
    }

    /// Shows when labels were given, or no longer (Shift+T).
    pub(super) fn toggle_times(&mut self) {
        self.times = !self.times;
        self.refresh();
    }

    /// Shows the entries `filter` chooses, all branches unfolded; as Scramjet's keys for the
    /// filters other than its default, choosing the filter shown goes back to the default.
    pub(super) fn toggle(&mut self, filter: Filter) {
        self.filter = match filter == self.filter {
            true => Filter::Default,
            false => filter,
        };
        self.folded.clear();
        self.refresh();
    }

    /// Shows the next filter in Ctrl+O's order (`forward`), or the one before.
    pub(super) fn cycle(&mut self, forward: bool) {
        let count = Filter::CYCLE.len();
        let at = (Filter::CYCLE.iter()).position(|&filter| filter == self.filter);
        let at = at.unwrap_or(0) + if forward { 1 } else { count - 1 };
        self.filter = Filter::CYCLE[at % count];
        self.folded.clear();
        self.refresh();
    }

    /// Sets the query, all branches unfolded, as Scramjet's search does.
    pub(super) fn set_query(&mut self, query: String) {
        self.query = query;
        self.folded.clear();
        self.refresh();
    }

    /// As Scramjet's Ctrl+Left on the row `row`: folds its branch when it can, else returns the
    /// row where the branch it is in starts, or the one before that.
    pub(super) fn fold_or_up(&mut self, row: usize) -> usize {
        let Some(&(entry, _)) = self.rows.get(row) else {
            return row;
        };
        if self.foldable(entry) && !self.folded.contains(&entry) {
            self.folded.insert(entry);
            self.refresh();
            return self.row_of(Some(entry));
        }
        let mut at = entry;
        loop {
            let Some(&parent) = self.shown_parents.get(&at) else {
                return row;
            };
            let Some(parent) = parent else {
                return self.row_of(Some(at));
            };
            let start = self.row_of(Some(at));
            if self.children.get(&Some(parent)).map_or(0, Vec::len) > 1 && start < row {
                return start;
            }
            at = parent;
        }
    }

    /// As Scramjet's Ctrl+Right on the row `row`: unfolds its branch when folded, else returns
    /// the row where the next branch under it starts, or its last entry when none does.
    pub(super) fn unfold_or_down(&mut self, row: usize) -> usize {
        let Some(&(entry, _)) = self.rows.get(row) else {
            return row;
        };
        if self.folded.remove(&entry) {
            self.refresh();
            return self.row_of(Some(entry));
        }
        let mut at = entry;
        loop {
            match self.children.get(&Some(at)).map(Vec::as_slice) {
                Some([only]) => at = *only,
                Some([first, ..]) => return self.row_of(Some(*first)),
                _ => return self.row_of(Some(at)),
            }
        }
    }

    /// Whether `entry` can be folded: it has entries shown under it, and is a root or starts a
    /// branch.
    fn foldable(&self, entry: usize) -> bool {
        let has_children = self
            .children
            .get(&Some(entry))
            .is_some_and(|c| !c.is_empty());
        let starts_branch = match self.shown_parents.get(&entry) {
            Some(Some(parent)) => self.children.get(&Some(*parent)).map_or(0, Vec::len) > 1,
            _ => true,
        };
        has_children && starts_branch
    }

    /// Chooses the entries shown and lays them out.
    fn refresh(&mut self) {
        let shown = self.chosen();
        let laid_out = layout(&self.tree.entries, &self.parents, &shown);
        (self.children, self.shown_parents) = (laid_out.children, laid_out.parents);
        let multiple_roots = self.children.get(&None).map_or(0, Vec::len) > 1;
        let rows = (laid_out.rows.iter())
            .map(|row| (row.entry, self.text(row, multiple_roots)))
            .collect();
        self.rows = rows;
    }

    /// The entries the filter and the query choose outside folded branches, in the order shown.
    fn chosen(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        let tokens: Vec<&str> = query.split_whitespace().collect();
        let entries = &self.tree.entries;
        let mut hidden = vec![false; entries.len()];
        let mut shown = Vec::new();
        // A parent comes before its children in the file.
        for (index, entry) in entries.iter().enumerate() {
            if let Some(parent) = self.parents[index] {
                hidden[index] = hidden[parent] || self.folded.contains(&parent);
            }
            let current = self.tree.current == Some(entry.line);
            if hidden[index] || !self.filter.shows(entry, current) {
                continue;
            }
            let text = searched(entry).to_lowercase();
            if tokens.iter().all(|token| text.contains(token)) {
                shown.push(index);
            }
        }
        shown.sort_by_key(|&index| self.order[index]);
        shown
    }

    /// A row's text: its tree lines, with its branch's fold mark, then its entry's.
    fn text(&self, row: &Row, multiple_roots: bool) -> String {
        let paint = |style: Style, text: &str| match self.color && !text.is_empty() {
            true => format!("{style}{text}{RESET}"),
            false => text.to_owned(),
        };
        let entry = &self.tree.entries[row.entry];
        let folded = self.folded.contains(&row.entry);
        let fold = match (folded, self.foldable(row.entry)) {
            (true, _) => '⊞',
            (false, true) => '⊟',
            (false, false) => '─',
        };
        let connector = row.connector && !row.virtual_root_child;
        let root_fold = if folded && !connector { "⊞ " } else { "" };
        let marker = if entry.active { "• " } else { "" };
        let (given, time) = match &entry.label {
            Some(label) => (
                format!("[{}] ", text::one_line(&text::visible(&label.text))),
                match self.times {
                    true => format!("{} ", clock(label.time)),
                    false => String::new(),
                },
            ),
            None => Default::default(),
        };
        paint(SELECTOR_TREE, &prefix(row, multiple_roots, fold))
            + &paint(TREE_PATH, root_fold)
            + &paint(TREE_PATH, marker)
            + &paint(TREE_LABEL, &given)
            + &paint(TREE_LABEL_TIME, &time)
            + &label(&entry.item, &paint)
    }
}

/// Rows laid out, with the entries shown under each shown entry (`None`: the roots) and each
/// one's shown parent.
struct Layout {
    rows: Vec<Row>,
    children: HashMap<Option<usize>, Vec<usize>>,
    parents: HashMap<usize, Option<usize>>,
}

/// Lays out `shown`, entries in the order they show, each under its nearest ancestor among them
/// (`parents` gives each one's), the branch with the current entry first among siblings.
fn layout(entries: &[TreeEntry], parents: &[Option<usize>], shown: &[usize]) -> Layout {
    let set: HashSet<usize> = shown.iter().copied().collect();
    let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    let mut shown_parents = HashMap::new();
    for &index in shown {
        let mut parent = parents[index];
        while let Some(at) = parent.filter(|at| !set.contains(at)) {
            parent = parents[at];
        }
        shown_parents.insert(index, parent);
        children.entry(parent).or_default().push(index);
    }
    // Stable, so siblings otherwise keep their order.
    for siblings in children.values_mut() {
        siblings.sort_by_key(|&index| !entries[index].active);
    }
    let roots = children.get(&None).map(Vec::as_slice).unwrap_or_default();
    let multiple_roots = roots.len() > 1;
    let mut stack = Vec::new();
    for (position, &root) in roots.iter().enumerate().rev() {
        stack.push((
            Row {
                entry: root,
                indent: usize::from(multiple_roots),
                connector: multiple_roots,
                last: position == roots.len() - 1,
                gutters: Vec::new(),
                virtual_root_child: multiple_roots,
            },
            multiple_roots,
        ));
    }
    let mut laid_out = Vec::new();
    // Iterative: a long session is a chain of thousands of entries.
    while let Some((row, just_branched)) = stack.pop() {
        let kids = children
            .get(&Some(row.entry))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let branches = kids.len() > 1;
        let child_indent = if branches || (just_branched && row.indent > 0) {
            row.indent + 1
        } else {
            row.indent
        };
        let display_indent = display_indent(row.indent, multiple_roots);
        let mut child_gutters = row.gutters.clone();
        if row.connector && !row.virtual_root_child {
            child_gutters.push(Gutter {
                position: display_indent.saturating_sub(1),
                show: !row.last,
            });
        }
        for (position, &kid) in kids.iter().enumerate().rev() {
            stack.push((
                Row {
                    entry: kid,
                    indent: child_indent,
                    connector: branches,
                    last: position == kids.len() - 1,
                    gutters: child_gutters.clone(),
                    virtual_root_child: false,
                },
                branches,
            ));
        }
        laid_out.push(row);
    }
    Layout {
        rows: laid_out,
        children,
        parents: shown_parents,
    }
}

/// When a label was given, as Scramjet's tree says it: the time today, else the day too, and
/// the year when it was another.
pub(super) fn clock(time: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    match (
        local(time),
        local(now.unwrap_or_default().as_millis() as u64),
    ) {
        (Some(at), Some(now)) => clock_at(at, now),
        _ => String::new(),
    }
}

/// A time's local year, month, day, hour and minute.
type Local = (i32, i32, i32, i32, i32);

fn clock_at(at: Local, now: Local) -> String {
    let (year, month, day, hour, minute) = at;
    let time = format!("{hour:02}:{minute:02}");
    if (year, month, day) == (now.0, now.1, now.2) {
        time
    } else if year == now.0 {
        format!("{month}/{day} {time}")
    } else {
        format!("{:02}/{month}/{day} {time}", year.rem_euclid(100))
    }
}

pub(super) fn local(milliseconds: u64) -> Option<Local> {
    let seconds = libc::time_t::try_from(milliseconds / 1000).ok()?;
    // SAFETY: `tm` is plain data, which `localtime_r` (`localtime_s`) fills in from `seconds` without keeping
    // either pointer.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        #[cfg(unix)]
        if libc::localtime_r(&seconds, &mut tm).is_null() {
            return None;
        }
        #[cfg(windows)]
        if libc::localtime_s(&mut tm, &seconds) != 0 {
            return None;
        }
        tm
    };
    Some((
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
    ))
}

/// What a query looks for in an entry, as in Scramjet: its label, its kind and its text.
fn searched(entry: &TreeEntry) -> String {
    let item = match &entry.item {
        TreeItem::Prompt(text) => format!("user {text}"),
        TreeItem::Reply(text) => format!("assistant {text}"),
        TreeItem::Reasoning => "assistant".to_owned(),
        TreeItem::Stopped(None) => "assistant aborted".to_owned(),
        TreeItem::Stopped(Some(error)) => format!("assistant {error}"),
        TreeItem::Command(command) => format!("bash {command}"),
        TreeItem::Tool(name, summary) => format!("tool {name} {summary}"),
        TreeItem::Call(name, summary) => format!("assistant {name} {summary}"),
        TreeItem::Compaction(_) => "compaction".to_owned(),
        TreeItem::BranchSummary(summary) => format!("branch summary {summary}"),
        TreeItem::Model(model) => format!("model {model}"),
        TreeItem::Effort(level) => format!("thinking {level}"),
    };
    match &entry.label {
        Some(label) => format!("{} {item}", label.text),
        None => item,
    }
}

/// Several roots hang from an unseen root, whose level is not drawn.
fn display_indent(indent: usize, multiple_roots: bool) -> usize {
    if multiple_roots {
        indent.saturating_sub(1)
    } else {
        indent
    }
}

/// Three columns for each level of indent: gutters, and the row's own connector in the last one,
/// `fold` in its middle.
fn prefix(row: &Row, multiple_roots: bool, fold: char) -> String {
    let indent = display_indent(row.indent, multiple_roots);
    let connector = (row.connector && !row.virtual_root_child)
        .then(|| indent.checked_sub(1))
        .flatten();
    let mut prefix = String::new();
    for column in 0..indent * 3 {
        let (level, offset) = (column / 3, column % 3);
        prefix.push(
            match row.gutters.iter().find(|gutter| gutter.position == level) {
                Some(gutter) if offset == 0 && gutter.show => '│',
                Some(_) => ' ',
                None if connector == Some(level) => match offset {
                    0 if row.last => '└',
                    0 => '├',
                    1 => fold,
                    _ => ' ',
                },
                None => ' ',
            },
        );
    }
    prefix
}

/// An entry's text as Scramjet's tree shows it, on one line, each part in its `paint`.
fn label(item: &TreeItem, paint: &dyn Fn(Style, &str) -> String) -> String {
    let clean = |text: &str| text::one_line(&text::visible(text));
    let assistant = |style: Style, text: &str| {
        paint(TREE_ASSISTANT, "assistant: ") + &paint(style, &clean(text))
    };
    match item {
        TreeItem::Prompt(text) => paint(TREE_USER, "user: ") + &clean(text),
        TreeItem::Reply(text) => paint(TREE_ASSISTANT, "assistant: ") + &clean(text),
        TreeItem::Reasoning => assistant(TREE_OTHER, "(no content)"),
        TreeItem::Stopped(None) => assistant(TREE_OTHER, "(aborted)"),
        TreeItem::Stopped(Some(error)) => {
            let error: String = text::one_line(error).chars().take(80).collect();
            assistant(TREE_ERROR, &error)
        }
        TreeItem::Command(command) => paint(TREE_OTHER, &format!("[bash]: {}", clean(command))),
        TreeItem::Tool(name, summary) | TreeItem::Call(name, summary)
            if scramjet_rs::agent::is_shell(name) =>
        {
            let command = text::one_line(summary);
            let call = match command.char_indices().nth(50) {
                Some((end, _)) => format!("[{name}: {}...]", &command[..end]),
                None => format!("[{name}: {command}]"),
            };
            paint(TREE_OTHER, &clean(&call))
        }
        TreeItem::Tool(name, summary) | TreeItem::Call(name, summary) => {
            paint(TREE_OTHER, &clean(&format!("[{name}: {summary}]")))
        }
        TreeItem::Compaction(Some(tokens)) => {
            let tokens = (tokens + 500) / 1000;
            paint(TREE_COMPACTION, &format!("[compaction: {tokens}k tokens]"))
        }
        TreeItem::Compaction(None) => paint(TREE_COMPACTION, "[compaction]"),
        TreeItem::BranchSummary(summary) => {
            paint(TREE_SUMMARY, "[branch summary]: ") + &clean(summary)
        }
        TreeItem::Model(model) => paint(TREE_OTHER, &clean(&format!("[model: {model}]"))),
        TreeItem::Effort(level) => paint(TREE_OTHER, &clean(&format!("[thinking: {level}]"))),
    }
}

/// A session with a model change, a tool result in each turn, and a branch where the second
/// prompt was asked another way; the leaf is the first branch's reply.
#[cfg(test)]
pub(super) fn sample() -> SessionTree {
    let entry = |line, parent, item, active| TreeEntry {
        line,
        parent,
        item,
        active,
        label: None,
    };
    let tool = |name: &str, summary: &str| TreeItem::Tool(name.into(), summary.into());
    SessionTree {
        entries: vec![
            entry(1, None, TreeItem::Model("a".into()), true),
            entry(2, Some(1), TreeItem::Prompt("fix the bug".into()), true),
            entry(3, Some(2), tool("read", "src/a.rs"), true),
            entry(4, Some(3), TreeItem::Reply("done".into()), true),
            entry(5, Some(4), TreeItem::Prompt("add tests".into()), true),
            entry(6, Some(5), tool("bash", "cargo test"), true),
            entry(7, Some(6), TreeItem::Reply("tests pass".into()), true),
            entry(8, Some(4), TreeItem::Prompt("other way".into()), false),
            entry(9, Some(8), TreeItem::Reply("ok".into()), false),
        ],
        current: Some(7),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: usize, parent: Option<usize>, text: &str, active: bool) -> TreeEntry {
        TreeEntry {
            line,
            parent,
            item: TreeItem::Prompt(text.into()),
            active,
            label: None,
        }
    }

    fn texts(view: &View) -> Vec<String> {
        view.rows().iter().map(|(_, text)| text.clone()).collect()
    }

    fn shown(tree: SessionTree) -> Vec<String> {
        texts(&View::new(tree, false, Filter::Default))
    }

    #[test]
    fn branches_indent_under_connectors_with_the_active_one_first() {
        // a → b → {c → d → {e, f}, g → h}, with the leaf at e.
        let tree = SessionTree {
            entries: vec![
                entry(1, None, "a", true),
                entry(2, Some(1), "b", true),
                entry(3, Some(2), "g", false),
                entry(4, Some(3), "h", false),
                entry(5, Some(2), "c", true),
                entry(6, Some(5), "d", true),
                entry(7, Some(6), "f", false),
                entry(8, Some(6), "e", true),
            ],
            current: Some(8),
        };
        let view = View::new(tree, false, Filter::Default);
        assert_eq!(
            texts(&view),
            [
                "• user: a",
                "• user: b",
                "├⊟ • user: c",
                "│     • user: d",
                "│     ├─ • user: e",
                "│     └─ user: f",
                "└⊟ user: g",
                "      user: h",
            ]
        );
        assert_eq!(view.row_of(view.current()), 4);
        assert_eq!(view.rows()[4].0, 7);
    }

    #[test]
    fn filters_and_a_query_choose_the_entries_hanging_each_from_its_nearest_shown_ancestor() {
        let mut view = View::new(sample(), false, Filter::Default);
        let default = [
            "• user: fix the bug",
            "• [read: src/a.rs]",
            "• assistant: done",
            "├⊟ • user: add tests",
            "│     • [bash: cargo test]",
            "│     • assistant: tests pass",
            "└⊟ user: other way",
            "      assistant: ok",
        ];
        assert_eq!(texts(&view), default);
        assert_eq!((view.row_of(view.current()), view.status()), (5, "".into()));
        view.toggle(Filter::UserOnly);
        assert_eq!(
            texts(&view),
            [
                "• user: fix the bug",
                "├─ • user: add tests",
                "└─ user: other way"
            ]
        );
        // The current entry is hidden: its nearest shown ancestor stands for it.
        assert_eq!(
            (view.row_of(view.current()), view.status()),
            (1, " [user]".into())
        );
        view.toggle(Filter::UserOnly);
        assert_eq!(texts(&view), default);
        view.toggle(Filter::NoTools);
        assert_eq!(
            texts(&view),
            [
                "• user: fix the bug",
                "• assistant: done",
                "├⊟ • user: add tests",
                "│     • assistant: tests pass",
                "└⊟ user: other way",
                "      assistant: ok",
            ]
        );
        view.cycle(true);
        assert_eq!(view.status(), " [user]");
        view.cycle(true);
        assert_eq!(view.status(), " [labeled]");
        assert!(texts(&view).is_empty());
        view.cycle(true);
        assert_eq!(view.status(), " [all]");
        assert_eq!(texts(&view)[..2], ["• [model: a]", "• user: fix the bug"]);
        view.cycle(true);
        assert_eq!(texts(&view), default);
        view.cycle(false);
        assert_eq!(view.status(), " [all]");
        view.toggle(Filter::Default);
        // Every word, in any case, in what each entry says or is.
        view.set_query("TEST".into());
        assert_eq!(
            texts(&view),
            [
                "• user: add tests",
                "• [bash: cargo test]",
                "• assistant: tests pass"
            ]
        );
        view.set_query("assistant way".into());
        assert!(texts(&view).is_empty());
        assert_eq!(view.row_of(None), 0);
        view.set_query("user way".into());
        assert_eq!(texts(&view), ["user: other way"]);
    }

    #[test]
    fn labels_show_before_their_entries_which_their_filter_and_a_query_choose() {
        let label = |text: &str, time| {
            Some(Label {
                text: text.into(),
                time,
            })
        };
        let mut tree = sample();
        tree.entries[4].label = label("checkpoint", 0);
        let mut view = View::new(tree, false, Filter::Labeled);
        assert_eq!(texts(&view), ["• [checkpoint] user: add tests"]);
        assert_eq!(
            (view.status(), view.label(4)),
            (" [labeled]".into(), Some("checkpoint"))
        );
        view.set_label(7, label("other\nway", 0));
        assert_eq!(
            texts(&view),
            [
                "• [checkpoint] user: add tests",
                "[other way] user: other way"
            ]
        );
        view.set_label(4, None);
        assert_eq!(texts(&view), ["[other way] user: other way"]);
        assert_eq!(view.label(4), None);
        // A query looks in labels too.
        view.toggle(Filter::Labeled);
        view.set_query("OTHER WAY".into());
        assert_eq!(texts(&view), ["[other way] user: other way"]);
        view.set_query("other assistant".into());
        assert!(texts(&view).is_empty());
        // Shift+T shows when each was given.
        view.set_query(String::new());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        view.set_label(7, label("now", now.unwrap().as_millis() as u64));
        view.toggle_times();
        assert_eq!(view.status(), " [+label time]");
        let row = &texts(&view)[6];
        let time = row.strip_prefix("└⊟ [now] ").unwrap();
        let (clock, rest) = time.split_at(6);
        assert_eq!(rest, "user: other way");
        assert!(clock.as_bytes()[2] == b':' && clock.ends_with(' '), "{row}");
        view.toggle_times();
        assert_eq!(texts(&view)[6], "└⊟ [now] user: other way");
    }

    #[test]
    fn a_label_s_time_is_the_time_today_else_with_the_day_and_another_year() {
        let now = (2026, 11, 4, 18, 30);
        assert_eq!(clock_at((2026, 11, 4, 9, 5), now), "09:05");
        assert_eq!(clock_at((2026, 3, 14, 23, 59), now), "3/14 23:59");
        assert_eq!(clock_at((2005, 12, 31, 0, 0), now), "05/12/31 00:00");
        assert!(local(0).is_some());
    }

    #[test]
    fn a_branch_folds_and_the_keys_jump_between_branch_starts() {
        let mut view = View::new(sample(), false, Filter::Default);
        // From a branch's last entry up to where it starts, then down to the next branch.
        assert_eq!(view.fold_or_up(5), 3);
        assert_eq!(view.unfold_or_down(0), 3);
        assert_eq!(view.unfold_or_down(4), 5);
        assert_eq!(view.fold_or_up(3), 3);
        assert_eq!(
            texts(&view),
            [
                "• user: fix the bug",
                "• [read: src/a.rs]",
                "• assistant: done",
                "├⊞ • user: add tests",
                "└⊟ user: other way",
                "      assistant: ok",
            ]
        );
        // A folded branch is not folded again: the key goes up, here to the root.
        assert_eq!(view.fold_or_up(3), 0);
        assert_eq!(view.unfold_or_down(3), 3);
        assert_eq!(texts(&view).len(), 8);
        assert_eq!(view.fold_or_up(0), 0);
        assert_eq!(texts(&view), ["⊞ • user: fix the bug"]);
        // A filter or a query unfolds every branch.
        view.toggle(Filter::NoTools);
        assert_eq!(texts(&view).len(), 6);
    }

    #[test]
    fn a_chain_stays_in_one_column_and_several_roots_have_no_connector() {
        let chain = SessionTree {
            entries: vec![entry(1, None, "a", true), entry(2, Some(1), "b", true)],
            current: Some(2),
        };
        assert_eq!(shown(chain), ["• user: a", "• user: b"]);
        let roots = SessionTree {
            entries: vec![
                entry(1, None, "old", false),
                entry(2, Some(1), "reply", false),
                entry(4, None, "new", true),
            ],
            current: Some(4),
        };
        assert_eq!(shown(roots), ["• user: new", "user: old", "   user: reply"]);
    }

    #[test]
    fn rows_are_colored_as_scramjets_tree() {
        let item = |line, parent, item, active| TreeEntry {
            line,
            parent,
            item,
            active,
            label: None,
        };
        let mut labeled = item(2, Some(1), TreeItem::Reply("b".into()), true);
        labeled.label = Some(Label {
            text: "x".into(),
            time: 0,
        });
        let tree = SessionTree {
            entries: vec![
                item(1, None, TreeItem::Prompt("a".into()), true),
                labeled,
                item(3, Some(1), TreeItem::Stopped(Some("boom".into())), false),
                item(4, Some(2), TreeItem::BranchSummary("went".into()), true),
                item(5, Some(4), TreeItem::Compaction(Some(2000)), true),
                item(6, Some(5), TreeItem::Model("m".into()), true),
            ],
            current: Some(6),
        };
        let shown = texts(&View::new(tree, true, Filter::Default));
        let (lines, path, error) = ("\x1b[2m", "\x1b[36m• \x1b[0m", "\x1b[31mboom\x1b[0m");
        let (assistant, end) = ("\x1b[32massistant: \x1b[0m", "\x1b[0m");
        assert_eq!(
            shown,
            [
                format!("{path}\x1b[36muser: {end}a"),
                format!("{lines}├⊟ {end}{path}\x1b[33m[x] {end}{assistant}b"),
                format!("{lines}│     {end}{path}\x1b[33m[branch summary]: {end}went"),
                format!("{lines}│     {end}{path}\x1b[36m[compaction: 2k tokens]{end}"),
                format!("{lines}│     {end}{path}{lines}[model: m]{end}"),
                format!("{lines}└─ {end}{assistant}{error}"),
            ]
        );
    }

    #[test]
    fn labels_are_one_line_and_long_commands_shortened() {
        let plain = |_: Style, text: &str| text.to_owned();
        let label = |item| label(&item, &plain);
        assert_eq!(
            label(TreeItem::Reply("two\nlines".into())),
            "assistant: two lines"
        );
        assert_eq!(
            label(TreeItem::Tool("read".into(), "src/a.rs:3-4".into())),
            "[read: src/a.rs:3-4]"
        );
        assert_eq!(
            label(TreeItem::Tool(
                "bash".into(),
                format!("echo {}\nls", "x".repeat(60))
            )),
            format!("[bash: echo {}...]", "x".repeat(45))
        );
        assert_eq!(
            label(TreeItem::Compaction(Some(52_600))),
            "[compaction: 53k tokens]"
        );
        assert_eq!(label(TreeItem::Compaction(None)), "[compaction]");
        assert_eq!(
            label(TreeItem::BranchSummary("Went\nthere".into())),
            "[branch summary]: Went there"
        );
        assert_eq!(label(TreeItem::Command("ls".into())), "[bash]: ls");
        assert_eq!(label(TreeItem::Stopped(None)), "assistant: (aborted)");
        assert_eq!(
            label(TreeItem::Stopped(Some(format!(
                "provider:\n{}",
                "x".repeat(90)
            )))),
            format!("assistant: provider: {}", "x".repeat(70))
        );
        assert_eq!(
            label(TreeItem::Prompt("a\u{202e}b".into())),
            "user: a\\u{202e}b"
        );
    }
}
