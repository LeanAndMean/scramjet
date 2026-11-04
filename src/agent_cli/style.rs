//! How the agent's terminal output looks, decided in one place: each kind of text the renderers
//! show has its escape sequence here, named for what it shows.

pub const RESET: &str = "\x1b[0m";

/// The status line and the running command's output lines above it.
pub const STATUS: &str = "\x1b[2m";
/// The status line's spinner, in Scramjet's accent color, and while a request waits to be retried,
/// in its warning color.
pub const SPINNER: &str = "\x1b[36m";
pub const SPINNER_RETRYING: &str = "\x1b[33m";
/// A subagent's glyph while it runs, once it is done and when it failed, and the name of the
/// agent or of several children's mode beside it.
pub const SUBAGENT_RUNNING: &str = "\x1b[33m";
pub const SUBAGENT_DONE: &str = "\x1b[32m";
pub const SUBAGENT_FAILED: &str = "\x1b[31m";
pub const SUBAGENT_NAME: &str = "\x1b[1m";
/// The footer under the prompt, and how full the context is there past 70%, then past 90%, as
/// Scramjet's warning and error colors show it.
pub const FOOTER: &str = "\x1b[2m";
pub const FOOTER_NEARLY_FULL: &str = "\x1b[33m";
pub const FOOTER_FULL: &str = "\x1b[31m";
/// Secondary notes: `Took 1.2s`, `(cancelled)`, why a compaction ran, editor hints.
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

/// Markdown `**strong**` text, `*emphasis*`, both (`***`), and `~~struck~~` text.
pub const STRONG: &str = "\x1b[1m";
pub const EMPHASIS: &str = "\x1b[3m";
pub const STRONG_EMPHASIS: &str = "\x1b[1;3m";
pub const STRUCK: &str = "\x1b[9m";
/// A markdown link's text, and its address after it when that differs.
pub const LINK: &str = "\x1b[4;34m";
pub const LINK_URL: &str = "\x1b[2m";
/// A markdown block quote's text, and the `│` border before it.
pub const QUOTE: &str = "\x1b[2;3m";
pub const QUOTE_BORDER: &str = "\x1b[34m";
/// A markdown horizontal rule, and a list item's bullet or number.
pub const HORIZONTAL_RULE: &str = "\x1b[2m";
pub const LIST_BULLET: &str = "\x1b[36m";
/// Markdown code spans.
pub const CODE: &str = "\x1b[36m";
/// The lines of a fenced code block, and its fence lines.
pub const CODE_BLOCK: &str = "\x1b[32m";
pub const CODE_FENCE: &str = "\x1b[2m";
/// Highlighted code, as near as the terminal's palette comes to Scramjet's colors: comments (and
/// annotations, diff headers), keywords, function names, variables (and parameters, keys,
/// attributes), strings, numbers (and literals such as `true`), and types (and built-ins). Diff
/// lines take `DIFF_ADDED` and `DIFF_REMOVED`; other code is plain.
pub const SYNTAX_COMMENT: &str = "\x1b[2m";
pub const SYNTAX_KEYWORD: &str = "\x1b[34m";
pub const SYNTAX_FUNCTION: &str = "\x1b[33m";
pub const SYNTAX_VARIABLE: &str = "\x1b[35m";
pub const SYNTAX_STRING: &str = "\x1b[32m";
pub const SYNTAX_NUMBER: &str = "\x1b[33m";
pub const SYNTAX_TYPE: &str = "\x1b[36m";
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
/// the entries that are not messages (tool calls, commands the user ran, model and thinking
/// changes) or replies without text, and the labels the user gave entries, with when.
pub const TREE_PATH: &str = "\x1b[36m";
pub const TREE_USER: &str = "\x1b[36m";
pub const TREE_ASSISTANT: &str = "\x1b[32m";
pub const TREE_ERROR: &str = "\x1b[31m";
pub const TREE_COMPACTION: &str = "\x1b[36m";
pub const TREE_SUMMARY: &str = "\x1b[33m";
pub const TREE_OTHER: &str = "\x1b[2m";
pub const TREE_LABEL: &str = "\x1b[33m";
pub const TREE_LABEL_TIME: &str = "\x1b[2m";
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
/// In `/resume`, the current session's prompt or name, and other sessions' names, in Scramjet's
/// accent and warning colors.
pub const SESSION_CURRENT: &str = "\x1b[36m";
pub const SESSION_NAMED: &str = "\x1b[33m";
/// A model `/scoped-models` has on, and one it has off.
pub const MODEL_ON: &str = "\x1b[32m";
pub const MODEL_OFF: &str = "\x1b[2m";
/// The rules above and below the list of a question the model asks, its title, and the rows of
/// the selected option, as Scramjet's border and accent colors show them.
pub const QUESTION_BORDER: &str = "\x1b[34m";
pub const QUESTION_TITLE: &str = "\x1b[1;36m";
pub const QUESTION_SELECTED: &str = "\x1b[36m";
