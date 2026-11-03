//! The rows of `/tree`, laid out as Scramjet's tree selector draws a session: a chain of entries
//! stays in one column, each branch point indents its children under `├─`/`└─` connectors with
//! `│` gutters, the branch with the current entry comes first, and `• ` marks that entry's path.

use super::text;
use scramjet_rs::agent::{SessionTree, TreeItem};

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

/// The entries of `tree` in the order shown, as indices into `tree.entries` with each row's text,
/// and the row of the current entry.
pub(super) fn rows(tree: &SessionTree) -> (Vec<(usize, String)>, Option<usize>) {
    let entries = &tree.entries;
    let mut children: std::collections::HashMap<Option<usize>, Vec<usize>> = Default::default();
    for (index, entry) in entries.iter().enumerate() {
        children.entry(entry.parent).or_default().push(index);
    }
    // Stable, so siblings otherwise keep their file order.
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
            .get(&Some(entries[row.entry].line))
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
    let current = tree.current.and_then(|line| {
        laid_out
            .iter()
            .position(|row| entries[row.entry].line == line)
    });
    let rows = laid_out
        .iter()
        .map(|row| {
            let entry = &entries[row.entry];
            let marker = if entry.active { "• " } else { "" };
            let text = format!(
                "{}{marker}{}",
                prefix(row, multiple_roots),
                label(&entry.item)
            );
            (row.entry, text)
        })
        .collect();
    (rows, current)
}

/// Several roots hang from an unseen root, whose level is not drawn.
fn display_indent(indent: usize, multiple_roots: bool) -> usize {
    if multiple_roots {
        indent.saturating_sub(1)
    } else {
        indent
    }
}

/// Three columns for each level of indent: gutters, and the row's own connector in the last one.
fn prefix(row: &Row, multiple_roots: bool) -> String {
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
                    1 => '─',
                    _ => ' ',
                },
                None => ' ',
            },
        );
    }
    prefix
}

/// An entry's text as Scramjet's tree shows it, on one line.
fn label(item: &TreeItem) -> String {
    let label = match item {
        TreeItem::Prompt(text) => format!("user: {text}"),
        TreeItem::Reply(text) => format!("assistant: {text}"),
        TreeItem::Reasoning => "assistant: (no content)".into(),
        TreeItem::Command(command) => format!("[bash]: {command}"),
        TreeItem::Tool(name, summary) if name == "bash" => {
            let command = text::one_line(summary);
            match command.char_indices().nth(50) {
                Some((end, _)) => format!("[bash: {}...]", &command[..end]),
                None => format!("[bash: {command}]"),
            }
        }
        TreeItem::Tool(name, summary) => format!("[{name}: {summary}]"),
        TreeItem::Compaction(Some(tokens)) => {
            format!("[compaction: {}k tokens]", (tokens + 500) / 1000)
        }
        TreeItem::Compaction(None) => "[compaction]".into(),
        TreeItem::BranchSummary(summary) => format!("[branch summary]: {summary}"),
        TreeItem::Model(model) => format!("[model: {model}]"),
        TreeItem::Effort(level) => format!("[thinking: {level}]"),
    };
    text::one_line(&text::visible(&label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet_rs::agent::TreeEntry;

    fn entry(line: usize, parent: Option<usize>, text: &str, active: bool) -> TreeEntry {
        TreeEntry {
            line,
            parent,
            item: TreeItem::Prompt(text.into()),
            active,
        }
    }

    fn shown(tree: &SessionTree) -> Vec<String> {
        rows(tree).0.into_iter().map(|(_, text)| text).collect()
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
        assert_eq!(
            shown(&tree),
            [
                "• user: a",
                "• user: b",
                "├─ • user: c",
                "│     • user: d",
                "│     ├─ • user: e",
                "│     └─ user: f",
                "└─ user: g",
                "      user: h",
            ]
        );
        assert_eq!(rows(&tree).1, Some(4));
        assert_eq!(rows(&tree).0[4].0, 7);
    }

    #[test]
    fn a_chain_stays_in_one_column_and_several_roots_have_no_connector() {
        let chain = SessionTree {
            entries: vec![entry(1, None, "a", true), entry(2, Some(1), "b", true)],
            current: Some(2),
        };
        assert_eq!(shown(&chain), ["• user: a", "• user: b"]);
        let roots = SessionTree {
            entries: vec![
                entry(1, None, "old", false),
                entry(2, Some(1), "reply", false),
                entry(4, None, "new", true),
            ],
            current: Some(4),
        };
        assert_eq!(
            shown(&roots),
            ["• user: new", "user: old", "   user: reply"]
        );
    }

    #[test]
    fn labels_are_one_line_and_long_commands_shortened() {
        let label = |item| label(&item);
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
        assert_eq!(
            label(TreeItem::Prompt("a\u{202e}b".into())),
            "user: a\\u{202e}b"
        );
    }
}
