//! How the agent's terminal output looks, decided in one place: each kind of text the renderers
//! show has its escape sequence here, named for what it shows.

pub const RESET: &str = "\x1b[0m";

/// The status line and the running command's output lines above it.
pub const STATUS: &str = "\x1b[2m";
/// A subagent's glyph while it runs, once it is done and when it failed, and the name of the
/// agent or of several children's mode beside it.
pub const SUBAGENT_RUNNING: &str = "\x1b[33m";
pub const SUBAGENT_DONE: &str = "\x1b[32m";
pub const SUBAGENT_FAILED: &str = "\x1b[31m";
pub const SUBAGENT_NAME: &str = "\x1b[1m";
/// Secondary notes: `took 1.2s`, `(cancelled)`, why a compaction ran, editor hints.
pub const MUTED: &str = "\x1b[2m";
/// A tool's result preview.
pub const TOOL_OUTPUT: &str = "\x1b[2m";
/// A failed tool's result preview.
pub const TOOL_FAILED: &str = "\x1b[31m";
/// Streamed reasoning summaries.
pub const REASONING: &str = "\x1b[2m";
/// The line shown in place of hidden reasoning.
pub const THINKING_HIDDEN: &str = "\x1b[2;3m";
/// A compaction summary.
pub const SUMMARY: &str = "\x1b[2m";
/// A section heading in an information block such as `/session`'s.
pub const INFO_HEADING: &str = "\x1b[1m";
/// A provider failure that is retried.
pub const WARNING: &str = "\x1b[33m";
/// What Scramjet notifies the user of about a command, such as a blocked report.
pub const NOTICE: &str = "\x1b[33m";
/// The record of a next step chosen or offered after a command completed.
pub const NEXT_STEP: &str = "\x1b[2m";
/// A command the user ran, as Scramjet's bash-mode color shows it: the rules around its block
/// when run with `!`, then with `!!` (kept out of the conversation), and its `$ command` line.
pub const COMMAND_RULE: &str = "\x1b[32m";
pub const COMMAND_EXCLUDED_RULE: &str = "\x1b[2m";
pub const COMMAND_LINE: &str = "\x1b[1;32m";
/// How a command the user ran ended, if not with exit code 0: failed, or cancelled.
pub const COMMAND_FAILED: &str = "\x1b[31m";
pub const COMMAND_CANCELLED: &str = "\x1b[33m";

pub const DIFF_REMOVED: &str = "\x1b[31m";
pub const DIFF_ADDED: &str = "\x1b[32m";
pub const DIFF_CONTEXT: &str = "\x1b[2m";
/// The words a one-line change replaced, within a removed or added line, and where they end
/// (which must leave the line's own style on).
pub const DIFF_CHANGED: &str = "\x1b[7m";
pub const DIFF_CHANGED_END: &str = "\x1b[27m";

/// Markdown `**strong**` text.
pub const STRONG: &str = "\x1b[1m";
/// Markdown code spans.
pub const CODE: &str = "\x1b[36m";
/// The lines of a fenced code block, and its fence lines.
pub const CODE_BLOCK: &str = "\x1b[32m";
pub const CODE_FENCE: &str = "\x1b[2m";
/// Markdown headings: level 1, then the others.
pub const TITLE: &str = "\x1b[1;4;33m";
pub const HEADING: &str = "\x1b[1;33m";

/// A selector's title, its selected row and the mark before that row.
pub const SELECTOR_TITLE: &str = "\x1b[1m";
pub const SELECTED: &str = "\x1b[1m";
pub const SELECTOR_MARK: &str = "\x1b[36m";
/// A heading among a selector's rows, over the rows it groups.
pub const SELECTOR_HEADING: &str = "\x1b[2m";
/// The tree lines before the rows of forked sessions in `/resume` and of `/tree`'s entries.
pub const SELECTOR_TREE: &str = "\x1b[2m";
/// In `/tree`'s rows, as Scramjet's tree colors them: the mark on the current entry's path,
/// who wrote a message, the error a turn stopped with, compactions, a branch summary's label,
/// and the entries that are not messages (tool calls, commands the user ran, model and
/// thinking changes) or replies without text.
pub const TREE_PATH: &str = "\x1b[36m";
pub const TREE_USER: &str = "\x1b[36m";
pub const TREE_ASSISTANT: &str = "\x1b[32m";
pub const TREE_ERROR: &str = "\x1b[31m";
pub const TREE_COMPACTION: &str = "\x1b[36m";
pub const TREE_SUMMARY: &str = "\x1b[33m";
pub const TREE_OTHER: &str = "\x1b[2m";
/// What a selector's key asks to confirm with the row it would remove, and why it cannot, as
/// Scramjet's error color shows them; what it did, in Scramjet's accent color.
pub const SELECTOR_WARNING: &str = "\x1b[31m";
pub const SELECTOR_DONE: &str = "\x1b[36m";
/// Text selected with the mouse in the full screen, as Scramjet's retained view shows it.
pub const SELECTION: &str = "\x1b[7m";
/// The cursor block after the query typed in a selector, as Scramjet's input draws it.
pub const SEARCH_CURSOR: &str = "\x1b[7m";
/// The selected row of the completion list under the prompt.
pub const COMPLETION_SELECTED: &str = "\x1b[36m";
/// The mark after the current session or model in a selector.
pub const CURRENT: &str = "\x1b[32m";
/// The rules above and below the list of a question the model asks, its title, and the rows of
/// the selected option, as Scramjet's border and accent colors show them.
pub const QUESTION_BORDER: &str = "\x1b[34m";
pub const QUESTION_TITLE: &str = "\x1b[1;36m";
pub const QUESTION_SELECTED: &str = "\x1b[36m";
