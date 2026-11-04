//! The rows of `/tree`, laid out as Scramjet's tree selector draws a session: a chain of entries
//! stays in one column, each branch point indents its children under `├─`/`└─` connectors with
//! `│` gutters, the branch with the current entry comes first, and `• ` marks that entry's path.

use super::style::{RESET, SELECTOR_TREE, TREE_ASSISTANT, TREE_COMPACTION, TREE_ERROR};
use super::style::{TREE_OTHER, TREE_PATH, TREE_SUMMARY, TREE_USER};
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
/// styled when `color`, and the row of the current entry.
pub(super) fn rows(tree: &SessionTree, color: bool) -> (Vec<(usize, String)>, Option<usize>) {
    let paint = |style: &str, text: &str| match color && !text.is_empty() {
        true => format!("{style}{text}{RESET}"),
        false => text.to_owned(),
    };
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
            let text = paint(SELECTOR_TREE, &prefix(row, multiple_roots))
                + &paint(TREE_PATH, marker)
                + &label(&entry.item, &paint);
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

/// An entry's text as Scramjet's tree shows it, on one line, each part in its `paint`.
fn label(item: &TreeItem, paint: &dyn Fn(&str, &str) -> String) -> String {
    let clean = |text: &str| text::one_line(&text::visible(text));
    let assistant = |style: &str, text: &str| {
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
        TreeItem::Tool(name, summary) if name == "bash" => {
            let command = text::one_line(summary);
            let call = match command.char_indices().nth(50) {
                Some((end, _)) => format!("[bash: {}...]", &command[..end]),
                None => format!("[bash: {command}]"),
            };
            paint(TREE_OTHER, &clean(&call))
        }
        TreeItem::Tool(name, summary) => paint(TREE_OTHER, &clean(&format!("[{name}: {summary}]"))),
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
        rows(tree, false)
            .0
            .into_iter()
            .map(|(_, text)| text)
            .collect()
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
        assert_eq!(rows(&tree, false).1, Some(4));
        assert_eq!(rows(&tree, false).0[4].0, 7);
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
    fn rows_are_colored_as_scramjets_tree() {
        let item = |line, parent, item, active| TreeEntry {
            line,
            parent,
            item,
            active,
        };
        let tree = SessionTree {
            entries: vec![
                item(1, None, TreeItem::Prompt("a".into()), true),
                item(2, Some(1), TreeItem::Reply("b".into()), true),
                item(3, Some(1), TreeItem::Stopped(Some("boom".into())), false),
                item(4, Some(2), TreeItem::BranchSummary("went".into()), true),
                item(5, Some(4), TreeItem::Compaction(Some(2000)), true),
                item(6, Some(5), TreeItem::Model("m".into()), true),
            ],
            current: Some(6),
        };
        let shown: Vec<_> = rows(&tree, true).0.into_iter().map(|row| row.1).collect();
        let (lines, path, error) = ("\x1b[2m", "\x1b[36m• \x1b[0m", "\x1b[31mboom\x1b[0m");
        let (assistant, end) = ("\x1b[32massistant: \x1b[0m", "\x1b[0m");
        assert_eq!(
            shown,
            [
                format!("{path}\x1b[36muser: {end}a"),
                format!("{lines}├─ {end}{path}{assistant}b"),
                format!("{lines}│     {end}{path}\x1b[33m[branch summary]: {end}went"),
                format!("{lines}│     {end}{path}\x1b[36m[compaction: 2k tokens]{end}"),
                format!("{lines}│     {end}{path}{lines}[model: m]{end}"),
                format!("{lines}└─ {end}{assistant}{error}"),
            ]
        );
    }

    #[test]
    fn labels_are_one_line_and_long_commands_shortened() {
        let plain = |_: &str, text: &str| text.to_owned();
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
