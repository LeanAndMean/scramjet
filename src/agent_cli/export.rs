//! `/export` and `--export`: a session's current branch as a web page of its own, as Scramjet
//! exports one, drawn from the replay of its items that the terminal shows (`show_items`). The
//! page runs no script and loads nothing: markdown is rendered here, what the model, a tool or
//! the user wrote is escaped, its links kept only to the web and to mail, and the images of
//! prompts and results are in the page.

use super::children::{call, icon, usage_line};
use super::slash::grouped;
use super::terminal::{command_body, framed, lines as lines_count};
use super::text::visible;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use scramjet_rs::agent::{
    AgentError, AgentUi, Answer, AutoCompaction, Branch, ChildItem, ChildRun, ChildState, Children,
    Choice, CompactRead, Compaction, DELEGATE, DELEGATE_SCOPE_WARNING, Delegation, Ending,
    PUBLICATION_TOOLS, Purpose, Stats, StreamProgress, ToolDetail, UserCommand, read_branch,
    show_items, terminal_line,
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Lines of a bash command's output shown until the rest is opened, as Scramjet's export shows.
const BASH_LINES: usize = 5;
/// Lines of other results, of written files and of commands the user ran.
const RESULT_LINES: usize = 10;

/// What the page shows of the agent besides its session: the instructions and the tools of its
/// requests, which a session file does not keep.
pub(super) struct Setup<'a> {
    pub(super) instructions: &'a str,
    pub(super) tools: &'a Value,
}

/// `--export FILE [OUTPUT]`, as Scramjet's: the page of the session saved in `file`, where a
/// resume would continue it, written to `output`, by default `scramjet-rs-session-<its id>.html`
/// in the current directory. Returns where it went.
pub(super) fn file(file: &Path, output: Option<PathBuf>) -> Result<PathBuf, String> {
    let branch = read_branch(file).map_err(|error| match error.kind() {
        std::io::ErrorKind::InvalidData => error.to_string(),
        _ => format!("{}: {error}", file.display()),
    })?;
    let id = file.file_stem().unwrap_or_default().to_string_lossy();
    let output = output.unwrap_or_else(|| format!("scramjet-rs-session-{id}.html").into());
    let page = page(&branch, &id, None);
    std::fs::write(&output, page).map_err(|error| format!("{}: {error}", output.display()))?;
    Ok(output)
}

/// The page showing `branch` of the session `id`.
pub(super) fn page(branch: &Branch, id: &str, setup: Option<Setup>) -> String {
    let title = branch
        .name
        .clone()
        .unwrap_or_else(|| format!("Session {id}"));
    let mut html = Html::default();
    // `show_items` shows conversation items; the entries only a session file has are shown here.
    // The page shows every call whole, so no read is compact.
    let mut start = 0;
    for (index, entry) in branch.entries.iter().enumerate() {
        let kind = entry["type"].as_str();
        if !matches!(kind, Some("compaction" | "branch_summary" | "model_change")) {
            continue;
        }
        show_items(&branch.entries[start..index], None, &mut html);
        start = index + 1;
        let summary = entry["summary"].as_str().unwrap_or_default();
        match kind {
            Some("compaction") => html.compaction_summary(summary),
            Some("branch_summary") => html.branch_summary(summary),
            _ => html.model(entry["model"].as_str().unwrap_or_default()),
        }
    }
    show_items(&branch.entries[start..], None, &mut html);
    html.block();
    let mut info = vec![("Session", escape(id))];
    if let Some(created) = branch.header["created"].as_u64() {
        let created = UNIX_EPOCH + Duration::from_millis(created);
        info.push(("Created", escape(&httpdate::fmt_http_date(created))));
    }
    if let Some(root) = branch.header["root"].as_str() {
        info.push(("Project", escape(root)));
    }
    let models = branch.models();
    if !models.is_empty() {
        info.push(("Models", escape(&models.join(", "))));
    }
    let stats = Stats::of(&branch.entries);
    let messages = format!("{} user, {} assistant", stats.user, stats.assistant);
    info.push(("Messages", messages));
    info.push(("Tool calls", stats.tool_calls.to_string()));
    let used = stats.tokens;
    let mut tokens = format!(
        "{} input, {} output",
        grouped(used.input),
        grouped(used.output)
    );
    if used.cache_read > 0 {
        tokens += &format!(", {} cache read", grouped(used.cache_read));
    }
    if used.cache_write > 0 {
        tokens += &format!(", {} cache write", grouped(used.cache_write));
    }
    info.push(("Tokens", tokens));
    if let Some(spent) = super::terminal::spent(used.cost, used.unpriced) {
        info.push(("Cost", spent));
    }
    let exported = httpdate::fmt_http_date(SystemTime::now());
    info.push(("Exported", escape(&exported)));
    let info: String = (info.iter())
        .map(|(label, value)| format!("<dt>{label}</dt><dd>{value}</dd>"))
        .collect();
    let setup = setup.map(setup_html).unwrap_or_default();
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\">\n\
<title>{title}</title>\n<style>{CSS}</style>\n</head>\n<body>\n<main>\n<header>\n<h1>{title}</h1>\n\
<dl>{info}</dl>\n{setup}</header>\n{}</main>\n</body>\n</html>\n",
        html.out,
        title = escape(&visible(&title)),
    )
}

/// The instructions and the tools, each closed until opened.
fn setup_html(setup: Setup) -> String {
    let tools: Vec<&Value> = setup.tools.as_array().into_iter().flatten().collect();
    let rows: String = (tools.iter())
        .map(|tool| {
            let text = |key: &str| escape(&visible(tool[key].as_str().unwrap_or_default()));
            format!("<dt>{}</dt><dd>{}</dd>", text("name"), text("description"))
        })
        .collect();
    format!(
        "<details class=\"setup\"><summary>Instructions</summary><div class=\"text\">{}</div>\
</details>\n<details class=\"setup\"><summary>Tools ({})</summary><dl class=\"tools\">{rows}</dl>\
</details>\n",
        escape(&visible(setup.instructions)),
        tools.len(),
    )
}

/// The conversation as HTML, written as `show_items` replays it.
#[derive(Default)]
struct Html {
    out: String,
    /// Reasoning shown since the last block, which goes before the next.
    thinking: String,
    /// A tool's block is open: its call was shown and its result is to follow.
    tool: bool,
    /// The images of the prompt or result shown next.
    images: String,
}

impl Html {
    fn end_thinking(&mut self) {
        let thinking = std::mem::take(&mut self.thinking);
        if !thinking.trim().is_empty() {
            self.out += &format!(
                "<details class=\"thinking\"><summary>Thinking</summary>\
<div class=\"markdown\">{}</div></details>\n",
                markdown(thinking.trim())
            );
        }
    }

    fn end_tool(&mut self) {
        if std::mem::take(&mut self.tool) {
            self.out += "</div>\n";
        }
    }

    /// Ends what is open, before a block of its own or at the end.
    fn block(&mut self) {
        self.end_thinking();
        self.end_tool();
    }

    fn model(&mut self, model: &str) {
        self.block();
        let model = escape(&visible(model));
        self.out += &format!("<div class=\"note\">Model: {model}</div>\n");
    }

    fn summary(&mut self, heading: &str, summary: &str) {
        self.block();
        self.out += &format!(
            "<details class=\"summary\"><summary>{heading}</summary>\
<div class=\"markdown\">{}</div></details>\n",
            markdown(summary)
        );
    }
}

impl AgentUi for Html {
    fn waiting(&mut self, _purpose: Purpose) {}
    fn progress(&mut self, update: &StreamProgress) {
        if let StreamProgress::Reasoning(text) = update {
            self.end_tool();
            self.thinking += text;
        }
    }
    fn retrying(&mut self, _error: &str, _attempt: u32, _retries: u32, _delay: Duration) {}
    fn assistant(&mut self, text: &str) {
        self.block();
        if !text.trim().is_empty() {
            let text = markdown(text.trim_end());
            self.out += &format!("<div class=\"reply markdown\">{text}</div>\n");
        }
    }
    fn tool_call(&mut self, name: &str, summary: &str, _: Option<&CompactRead>) {
        self.block();
        self.out += &format!(
            "<div class=\"tool\"><div class=\"call\"><b>{}</b> {}</div>\n",
            escape(&visible(name)),
            escape(&visible(summary.trim_end_matches('\n'))),
        );
        self.tool = true;
    }
    fn tool_running(&mut self, _: std::time::Instant) {}
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        self.end_thinking();
        // A call whose arguments did not parse has no call line.
        if !self.tool {
            let name = escape(&visible(name));
            self.out += &format!("<div class=\"tool\"><div class=\"call\"><b>{name}</b></div>\n");
        }
        let body = match (name, detail) {
            (_, Some(ToolDetail::Diff(diff))) => diff_lines(diff),
            // As the terminal shows a write: what it wrote, trailing blank lines left out.
            (_, Some(ToolDetail::Written { content, .. })) => {
                lines(content.trim_end_matches(['\n', '\r']), RESULT_LINES)
            }
            (_, Some(ToolDetail::Answer(answer))) => {
                format!("<div class=\"text\">{}</div>", escape(&visible(answer)))
            }
            (_, Some(ToolDetail::Subagents(children))) => children_html(children),
            (DELEGATE, None) if !failed => {
                let body = command_body(output);
                let mut shown = String::new();
                if body.starts_with(DELEGATE_SCOPE_WARNING) {
                    shown += "<div class=\"warning\">WARNING: caller and delegate allowed-tools \
scopes do not overlap; tool calls outside them are not blocked.</div>";
                }
                shown + &opened("Instructions", body)
            }
            (name, Some(ToolDetail::Publication(payload))) => {
                let what = match name {
                    "create_issue" => "The issue",
                    "create_pr" => "The pull request",
                    _ => "The comment",
                };
                opened(what, payload) + &lines(output, usize::MAX)
            }
            (name, None) if PUBLICATION_TOOLS.contains(&name) => lines(output, usize::MAX),
            // A failed command's ending, after a blank line, shows below what was cut.
            ("bash", None) => match output.rsplit_once("\n\n") {
                Some((body, ending)) if failed => {
                    let ending = escape(&visible(ending));
                    lines(body, BASH_LINES) + &format!("<div class=\"text\">{ending}</div>")
                }
                _ => lines(output, BASH_LINES),
            },
            _ => lines(output, RESULT_LINES),
        };
        let class = if failed { "result failed" } else { "result" };
        let images = std::mem::take(&mut self.images);
        self.out += &format!("<div class=\"{class}\">{body}{images}</div>\n</div>\n");
        self.tool = false;
    }
    fn command_started(&mut self, _command: &str, _include: bool) {}
    fn command_result(&mut self, ran: &UserCommand) {
        self.block();
        let (class, title) = match ran.include {
            true => ("command", "Run with !: its output went to the model"),
            false => (
                "command excluded",
                "Run with !!: its output stayed out of the context",
            ),
        };
        let ending = match &ran.ending {
            Ending::Done => String::new(),
            Ending::Exited(code) => format!("<div class=\"failed\">(exit {code})</div>"),
            Ending::Cancelled => "<div class=\"cancelled\">(cancelled)</div>".to_owned(),
            Ending::Failed(why) => format!("<div class=\"failed\">({})</div>", escape(why)),
        };
        self.out += &format!(
            "<div class=\"{class}\" title=\"{title}\"><div class=\"call\">$ {}</div>{}{ending}</div>\n",
            escape(&visible(&ran.command)),
            lines(&ran.output, RESULT_LINES),
        );
    }
    fn stopped(&mut self, error: Option<&str>) {
        self.block();
        self.out += &match error {
            Some(error) => format!(
                "<div class=\"error\">Error: {}</div>\n",
                escape(&visible(error))
            ),
            None => "<div class=\"note\">(cancelled)</div>\n".to_owned(),
        };
    }
    fn compacted(&mut self, compaction: &Compaction, _automatic: Option<AutoCompaction>) {
        self.compaction_summary(&compaction.summary);
    }
    fn compaction_failed(&mut self, _error: &AgentError) {}
    fn images(&mut self, urls: &[&str]) {
        let shown: String = (urls.iter())
            .filter(|url| shown_image(url))
            .map(|url| format!("<img src=\"{url}\" alt=\"\">"))
            .collect();
        if !shown.is_empty() {
            self.images = format!("<div class=\"images\">{shown}</div>");
        }
    }
    /// Its images first, as Scramjet's page shows a prompt.
    fn user_prompt(&mut self, prompt: &str) {
        self.block();
        let images = std::mem::take(&mut self.images);
        let prompt = escape(&visible(prompt));
        self.out +=
            &format!("<div class=\"prompt\">{images}<div class=\"text\">{prompt}</div></div>\n");
    }
    fn command_prompt(&mut self, typed: &str, sent: &str) {
        self.block();
        let images = std::mem::take(&mut self.images);
        // As the screen labels them: `[skill] NAME` or `[command] NAME`.
        let (heading, body) = match framed(sent) {
            Some(framed) => (
                format!(
                    "[{}] {}, sent as {}",
                    framed.label,
                    framed.name,
                    lines_count(framed.body)
                ),
                framed.body,
            ),
            None => {
                let body = command_body(sent);
                (format!("Sent as {}", lines_count(body)), body)
            }
        };
        self.out += &format!(
            "<div class=\"prompt\">{images}<div class=\"text\">{}</div>{}</div>\n",
            escape(&visible(typed)),
            opened(&heading, body),
        );
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.summary(
            "[compaction] The earlier conversation is summarized",
            summary,
        );
    }
    fn branch_summary(&mut self, summary: &str) {
        self.summary(
            "[branch] The branch this conversation came back from is summarized",
            summary,
        );
    }
    fn approve(&mut self, _question: &str, _cancel: &AtomicBool) -> Answer {
        Answer::Deny
    }
    /// Within the block of the call that asked it.
    fn question(&mut self, message: &str) {
        self.end_thinking();
        let message = markdown(message);
        self.out += &format!("<div class=\"question markdown\">{message}</div>\n");
    }
    fn notice(&mut self, text: &str) {
        self.block();
        let text = escape(&visible(text));
        self.out += &format!("<div class=\"notice\">{text}</div>\n");
    }
    fn next_step(&mut self, record: &str) {
        self.block();
        let record = escape(&visible(record));
        self.out += &format!("<div class=\"note text\">{record}</div>\n");
    }
    fn choose(&mut self, _: &str, _: &[Choice], _: Option<usize>) -> Option<usize> {
        None
    }
}

/// `markdown` under `heading`, closed until opened.
fn opened(heading: &str, markdown_text: &str) -> String {
    format!(
        "<details class=\"sent\"><summary>{}</summary><div class=\"markdown\">{}</div></details>",
        escape(heading),
        markdown(markdown_text)
    )
}

/// The lines of an output as a terminal shows them, the first `first` of them until the rest is
/// opened.
fn lines(text: &str, first: usize) -> String {
    let lines: Vec<String> = (text.lines())
        .map(|line| escape(&visible(&terminal_line(line))))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    if lines.len() <= first {
        return format!("<div class=\"lines\">{}</div>", lines.join("\n"));
    }
    let more = lines.len() - first;
    format!(
        "<details class=\"more\"><summary><span class=\"lines\">{}</span>\
<span class=\"hint\">… ({more} more lines)</span></summary><div class=\"lines\">{}</div></details>",
        lines[..first].join("\n"),
        lines[first..].join("\n"),
    )
}

/// An edit's changed lines, each colored by its mark.
fn diff_lines(diff: &str) -> String {
    let rows: Vec<String> = (diff.lines())
        .map(|line| {
            let class = match line.chars().next() {
                Some('+') => "added",
                Some('-') => "removed",
                _ => "context",
            };
            let line = escape(&visible(&terminal_line(line)));
            format!("<span class=\"{class}\">{line}</span>")
        })
        .collect();
    format!("<div class=\"lines diff\">{}</div>", rows.join("\n"))
}

/// A `subagent` call's children: how each went, closed until opened, with its task and what it
/// did.
fn children_html(children: &Children) -> String {
    let mut out = String::from("<div class=\"children\">");
    if children.mode != Delegation::Single || children.runs.len() > 1 {
        let done = (children.runs.iter())
            .filter(|run| run.state == ChildState::Done)
            .count();
        let (mode, unit) = match children.mode {
            Delegation::Chain => ("chain", "steps"),
            _ => ("parallel", "tasks"),
        };
        let total = children.runs.len();
        out += &format!("<div class=\"note\">{mode} {done}/{total} {unit} done</div>");
    }
    for run in &children.runs {
        out += &child_html(run);
    }
    out + "</div>"
}

fn child_html(run: &ChildRun) -> String {
    let text = |text: &str| escape(&visible(text));
    let mut head = format!(
        "{} <b>{}</b> ({})",
        icon(&run.state).0,
        text(&run.agent),
        text(&run.source)
    );
    if !run.model.is_empty() {
        head += &format!(" {}", text(&run.model));
    }
    if let Some(effort) = &run.effort {
        head += &format!(" [Effort:{}]", text(effort));
    }
    let usage = usage_line(&run.usage, None);
    if !usage.is_empty() {
        head += &format!(" · {}", text(&usage));
    }
    let mut body = String::new();
    for warning in &run.warnings {
        body += &format!("<div class=\"warning\">{}</div>", text(warning));
    }
    if let ChildState::Failed { message, .. } = &run.state {
        body += &format!("<div class=\"error\">Error: {}</div>", text(message));
    }
    body += &format!(
        "<div class=\"label\">Task</div><div class=\"text\">{}</div>",
        text(&run.task)
    );
    if !run.items.is_empty() {
        body += "<div class=\"label\">Output</div>";
    }
    for item in &run.items {
        body += &match item {
            ChildItem::Call { name, summary } => {
                format!(
                    "<div class=\"lines\">→ {}</div>",
                    text(&call(name, summary))
                )
            }
            ChildItem::Text(said) => format!("<div class=\"markdown\">{}</div>", markdown(said)),
        };
    }
    format!("<details class=\"child\"><summary>{head}</summary>{body}</details>")
}

/// `text` as HTML, written as markdown: raw HTML in it is shown as text, images as links, and
/// a link only where it goes to the web or to mail, or within the page.
fn markdown(text: &str) -> String {
    let text = visible(text);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // Whether each open link or image has its tags shown; a link within a link has none.
    let mut kept: Vec<bool> = Vec::new();
    let events = Parser::new_ext(&text, options).filter_map(|event| match event {
        Event::Html(html) | Event::InlineHtml(html) => Some(Event::Text(html)),
        Event::Start(
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }
            | Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            },
        ) => {
            let keep = linked(&dest_url) && !kept.contains(&true);
            kept.push(keep);
            keep.then_some(Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }))
        }
        Event::End(TagEnd::Link | TagEnd::Image) => {
            (kept.pop() == Some(true)).then_some(Event::End(TagEnd::Link))
        }
        event => Some(event),
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

/// Whether the page shows an image from `url`: one in it, of a type `read` sends, which the
/// browser decodes as an image and nothing else.
fn shown_image(url: &str) -> bool {
    let types = ["png", "jpeg", "gif", "webp"];
    let data = (url.strip_prefix("data:image/")).and_then(|rest| {
        let (kind, data) = rest.split_once(";base64,")?;
        types.contains(&kind).then_some(data)
    });
    data.is_some_and(|data| {
        (data.bytes()).all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
    })
}

/// Whether a link to `url` is kept: to the web, to mail, or without a scheme, which stays within
/// the page or next to it.
fn linked(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let schemes = ["http://", "https://", "mailto:"];
    !url.chars().any(|c| c.is_control() || c.is_whitespace())
        && (schemes.iter().any(|scheme| lower.starts_with(scheme)) || !url.contains(':'))
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped += "&amp;",
            '<' => escaped += "&lt;",
            '>' => escaped += "&gt;",
            '"' => escaped += "&quot;",
            '\'' => escaped += "&#39;",
            c => escaped.push(c),
        }
    }
    escaped
}

const CSS: &str = "
:root { color-scheme: dark; --bg: #1b1c22; --card: #23242c; --text: #d7d7dc; --dim: #8b8c96;
  --accent: #6cb6ff; --border: #34353f; --code: #2b2c35; --green: #7fd18b; --red: #f47c7c;
  --yellow: #e5c07b; --prompt: #2d3140; }
@media (prefers-color-scheme: light) {
  :root { color-scheme: light; --bg: #f4f4f6; --card: #ffffff; --text: #24252b; --dim: #6c6d78;
    --accent: #0b62c4; --border: #d9d9e0; --code: #ececf1; --green: #1f7a33; --red: #b42323;
    --yellow: #8a6100; --prompt: #e6ebf5; }
}
body { margin: 0; background: var(--bg); color: var(--text);
  font: 15px/1.5 system-ui, -apple-system, 'Segoe UI', sans-serif; }
main { max-width: 960px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { font-size: 1.4em; margin: 0 0 12px; overflow-wrap: anywhere; }
header { background: var(--card); border: 1px solid var(--border); border-radius: 8px;
  padding: 16px; margin-bottom: 24px; }
header > dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 16px; margin: 0; }
dt { color: var(--dim); }
dd { margin: 0; overflow-wrap: anywhere; }
dl.tools { display: block; }
dl.tools dt { color: var(--text); font-weight: bold; margin-top: 8px; }
dl.tools dd { color: var(--dim); white-space: pre-wrap; }
code, .lines, .call, pre { font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
  font-size: 13px; }
.lines, .text { white-space: pre-wrap; overflow-wrap: anywhere; display: block; tab-size: 4; }
pre { background: var(--code); padding: 8px 10px; border-radius: 6px; overflow-x: auto; }
:not(pre) > code { background: var(--code); padding: 1px 4px; border-radius: 4px; }
a { color: var(--accent); }
table { border-collapse: collapse; }
th, td { border: 1px solid var(--border); padding: 4px 8px; }
blockquote { margin: 0; padding-left: 12px; border-left: 3px solid var(--border); color: var(--dim); }
.markdown > :first-child { margin-top: 0; }
.markdown > :last-child { margin-bottom: 0; }
.prompt { background: var(--prompt); border-radius: 8px; padding: 10px 14px; margin: 20px 0 12px; }
.reply { margin: 12px 0; }
.images img { display: block; max-width: 100%; max-height: 400px; border-radius: 4px;
  margin: 6px 0; }
.tool, .command { background: var(--card); border: 1px solid var(--border); border-radius: 6px;
  padding: 8px 12px; margin: 10px 0; }
.command { border-color: var(--green); }
.command.excluded { border-color: var(--border); border-style: dashed; }
.call { overflow-wrap: anywhere; white-space: pre-wrap; }
.command > .call { color: var(--green); font-weight: bold; }
.result, .command > .lines, .command > details { color: var(--dim); margin-top: 6px; }
.result.failed, .failed, .error { color: var(--red); }
.cancelled, .warning, .notice { color: var(--yellow); }
.note, .label, .hint { color: var(--dim); }
.note { margin: 10px 0; font-size: 0.9em; }
.label { margin-top: 8px; font-size: 0.9em; }
.question { margin-top: 6px; }
.diff .added { color: var(--green); }
.diff .removed { color: var(--red); }
.diff .context { color: var(--dim); }
details > summary { cursor: pointer; }
details.more > summary { display: block; list-style: none; }
details.more > summary::-webkit-details-marker { display: none; }
details.more[open] > summary .hint { display: none; }
details.more:not([open]) > summary .hint::after { content: ' (click to show)'; }
details.thinking, details.summary, details.sent, details.child, details.setup { margin: 8px 0; }
details.thinking > summary, details.sent > summary, details.setup > summary { color: var(--dim); }
details.thinking > div { color: var(--dim); font-style: italic; margin: 6px 0 0 14px; }
details.summary { background: var(--card); border: 1px solid var(--border); border-radius: 6px;
  padding: 8px 12px; }
details.summary > div, details.sent > div, details.child > div { margin: 6px 0 0 14px; }
details.setup > div { margin-top: 6px; max-height: 480px; overflow: auto; }
";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn branch(entries: Vec<Value>) -> Branch {
        Branch {
            header: json!({"type":"session","version":1,"root":"/work/app","created":0}),
            entries,
            name: None,
        }
    }

    fn call_item(id: &str, name: &str, arguments: Value) -> Value {
        json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments.to_string()})
    }

    fn output_item(id: &str, output: &str) -> Value {
        json!({"type":"function_call_output","call_id":id,"output":output})
    }

    #[test]
    fn a_page_shows_the_branch_as_the_terminal_replays_it() {
        let reply = json!({"type":"message","role":"assistant","_scramjet_model":"gpt-6-luna",
            "_scramjet_usage":{"input":1200,"output":30,"cache_write":100,"cost":0.0123},
            "content":[{"type":"output_text","text":"Found **two** files:\n\n- `a.rs`\n- b.rs"}]});
        let bash_output = (1..=8).map(|n| format!("line {n}")).collect::<Vec<_>>();
        let page = page(
            &branch(vec![
                json!({"type":"model_change","model":"gpt-6-luna"}),
                json!({"role":"user","content":"list <files>"}),
                json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Look first."}]}),
                call_item("c1", "bash", json!({"command":"ls"})),
                output_item("c1", &bash_output.join("\n")),
                call_item("c2", "edit", json!({"path":"a.rs","edits":[]})),
                json!({"type":"function_call_output","call_id":"c2","output":"Edited",
                    "_scramjet_diff":"-1 old\n+1 new"}),
                reply,
                json!({"type":"compaction","summary":"Listed the files.","first_kept":0}),
                json!({"role":"user","content":"Ran `false`\n(no output)\n\nCommand exited with code 1",
                    "_scramjet_typed":"!false","_scramjet_failed":true}),
                json!({"type":"turn_stopped"}),
            ]),
            "1700000000000-1",
            None,
        );
        let at = |text: &str| {
            page.find(text)
                .unwrap_or_else(|| panic!("{text:?} not in the page:\n{page}"))
        };
        assert!(
            at("<title>Session 1700000000000-1</title>") < at("<dt>Project</dt><dd>/work/app</dd>")
        );
        at("<dt>Models</dt><dd>gpt-6-luna</dd>");
        at("<dt>Messages</dt><dd>1 user, 3 assistant</dd><dt>Tool calls</dt><dd>2</dd>");
        at(
            "<dt>Tokens</dt><dd>1,200 input, 30 output, 100 cache write</dd><dt>Cost</dt><dd>$0.0123</dd>",
        );
        let order = [
            at("<div class=\"note\">Model: gpt-6-luna</div>"),
            at("<div class=\"text\">list &lt;files&gt;</div>"),
            at("<summary>Thinking</summary><div class=\"markdown\"><p>Look first.</p>"),
            at("<div class=\"call\"><b>bash</b> ls</div>"),
            at(
                "<summary><span class=\"lines\">line 1\nline 2\nline 3\nline 4\nline 5</span>\
<span class=\"hint\">… (3 more lines)</span></summary><div class=\"lines\">line 6\nline 7\nline 8</div>",
            ),
            at("<span class=\"removed\">-1 old</span>\n<span class=\"added\">+1 new</span>"),
            at("<p>Found <strong>two</strong> files:</p>"),
            at("<li><code>a.rs</code></li>"),
            at("<summary>[compaction] The earlier conversation is summarized</summary>"),
            at("<div class=\"call\">$ false</div><div class=\"failed\">(exit 1)</div>"),
            at("<div class=\"note\">(cancelled)</div>"),
        ];
        assert!(order.is_sorted(), "{order:?}\n{page}");
        // Every block that opened closed.
        assert_eq!(page.matches("<div").count(), page.matches("</div>").count());
    }

    #[test]
    fn a_command_or_skill_shows_under_its_label_without_its_tags() {
        let skill = "<skill name=\"pdf\" location=\"/s/pdf/SKILL.md\">\nReferences are relative \
            to /s/pdf.\n\nFill forms.\n</skill>\n\nform.pdf";
        let command =
            "<scramjet-command name=\"rs:loop\">\n# Build\n\nOne step.\n</scramjet-command>";
        let page = page(
            &branch(vec![
                json!({"role":"user","content":skill,"_scramjet_typed":"/skill:pdf form.pdf"}),
                json!({"role":"user","content":command,"_scramjet_typed":"/rs:loop"}),
                json!({"role":"user","content":"a.txt:\nhello","_scramjet_typed":"@a.txt"}),
            ]),
            "1700000000000-1",
            None,
        );
        let at = |text: &str| {
            page.find(text)
                .unwrap_or_else(|| panic!("{text:?} not in the page:\n{page}"))
        };
        let order = [
            at("<div class=\"text\">/skill:pdf form.pdf</div>"),
            at("<summary>[skill] pdf, sent as 3 lines</summary>"),
            at("<p>Fill forms.</p>"),
            at("<summary>[command] rs:loop, sent as 3 lines</summary>"),
            at("<p>One step.</p>"),
            at("<summary>Sent as 2 lines</summary>"),
        ];
        assert!(order.is_sorted(), "{order:?}\n{page}");
        assert!(!page.contains("skill name=") && !page.contains("scramjet-command"));
    }

    #[test]
    fn what_the_model_or_a_tool_wrote_is_shown_never_run_or_loaded() {
        let shown = markdown(
            "<script>alert(1)</script>\n\nSee [it](javascript:alert(1)), [docs](https://x.dev/a?b=1&c=2), \
[mail](mailto:a@b.c), [here](#top) and ![logo](https://x.dev/logo.png) <img src=x onerror=alert(1)>",
        );
        assert!(!shown.contains("<script"), "{shown}");
        assert!(!shown.contains("<img"), "{shown}");
        assert!(!shown.contains("javascript:"), "{shown}");
        assert!(
            shown.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{shown}"
        );
        assert!(shown.contains("See it, "), "{shown}");
        assert!(
            shown.contains("<a href=\"https://x.dev/a?b=1&amp;c=2\">docs</a>"),
            "{shown}"
        );
        assert!(
            shown.contains("<a href=\"mailto:a@b.c\">mail</a>"),
            "{shown}"
        );
        assert!(shown.contains("<a href=\"#top\">here</a>"), "{shown}");
        assert!(
            shown.contains("<a href=\"https://x.dev/logo.png\">logo</a>"),
            "{shown}"
        );
        assert!(
            shown.contains("&lt;img src=x onerror=alert(1)&gt;"),
            "{shown}"
        );
        // An image within a link is its text.
        let nested = markdown("[![badge](https://x.dev/b.svg)](https://x.dev)");
        assert_eq!(nested.matches("<a ").count(), 1, "{nested}");
        let page = page(
            &branch(vec![
                json!({"role":"user","content":"</div><script>x()</script>"}),
                call_item("c1", "bash", json!({"command":"echo '<b>'"})),
                output_item("c1", "\x1b[31m<b>red</b>\x1b[0m\u{202e}txt"),
            ]),
            "id",
            Some(Setup {
                instructions: "Be <brief>.",
                tools: &json!([{"name":"read","description":"Read <files>"}]),
            }),
        );
        assert!(!page.contains("<script>"), "{page}");
        assert!(
            page.contains("&lt;/div&gt;&lt;script&gt;x()&lt;/script&gt;"),
            "{page}"
        );
        assert!(
            page.contains("<b>bash</b> echo &#39;&lt;b&gt;&#39;"),
            "{page}"
        );
        assert!(
            page.contains("&lt;b&gt;red&lt;/b&gt;\\u{202e}txt"),
            "{page}"
        );
        assert!(
            page.contains("<div class=\"text\">Be &lt;brief&gt;.</div>"),
            "{page}"
        );
        assert!(
            page.contains("<dt>read</dt><dd>Read &lt;files&gt;</dd>"),
            "{page}"
        );
        assert!(page.contains("default-src 'none'"), "{page}");
    }

    #[test]
    fn the_images_of_prompts_and_results_are_shown_in_their_blocks() {
        let image = |url: &str| json!({"type":"input_image","detail":"auto","image_url":url});
        let png = "data:image/png;base64,iVBORw0K+/8=";
        let page = page(
            &branch(vec![
                json!({"role":"user","_scramjet_typed":"@shot.png what is it?","content":[
                    {"type":"input_text","text":"<file name=\"shot.png\"></file>\nwhat is it?"},
                    image(png)]}),
                call_item("c1", "read", json!({"path":"b.jpg"})),
                json!({"type":"function_call_output","call_id":"c1","output":[
                    {"type":"input_text","text":"Read image file [image/jpeg]"},
                    image("data:image/jpeg;base64,/9j/4A=="),
                    image("data:image/svg+xml;base64,PHN2Zz4="),
                    image("data:image/png;base64,AA\" onerror=\"x()"),
                    image("https://x.dev/track.png")]}),
                json!({"role":"user","content":"thanks"}),
            ]),
            "id",
            None,
        );
        let at = |text: &str| {
            page.find(text)
                .unwrap_or_else(|| panic!("{text:?} not in the page:\n{page}"))
        };
        let order = [
            at(&format!(
                "<div class=\"prompt\"><div class=\"images\"><img src=\"{png}\" alt=\"\"></div>\
<div class=\"text\">@shot.png what is it?</div>"
            )),
            at("<b>read</b> b.jpg"),
            at("Read image file [image/jpeg]</div><div class=\"images\">\
<img src=\"data:image/jpeg;base64,/9j/4A==\" alt=\"\"></div></div>"),
            at("<div class=\"prompt\"><div class=\"text\">thanks</div>"),
        ];
        assert!(order.is_sorted(), "{order:?}\n{page}");
        assert_eq!(page.matches("<img ").count(), 2, "{page}");
        assert!(!page.contains("svg") && !page.contains("onerror"), "{page}");
        assert!(!page.contains("x.dev"), "{page}");
        assert!(page.contains("img-src data:;"), "{page}");
        assert_eq!(page.matches("<div").count(), page.matches("</div>").count());
    }

    #[test]
    fn a_question_its_answer_and_children_show_in_their_calls_block() {
        let children = json!({"mode":"single","runs":[{"agent":"scout","source":"user",
            "model":"gpt-6-luna","effort":"low","task":"Find <it>","state":"done",
            "items":[{"call":{"name":"bash","summary":"rg it"}},{"text":"Found **it**."}],
            "usage":{"turns":2,"input_tokens":1500,"cached_input_tokens":0,"output_tokens":40},
            "warnings":[]}]});
        let page = page(
            &branch(vec![
                call_item(
                    "q1",
                    "get_scramjet_user_input",
                    json!({"type":"confirm","message":"Go **on**?"}),
                ),
                output_item("q1", r#"{"confirmed":true}"#),
                call_item(
                    "s1",
                    "subagent",
                    json!({"agent":"scout","task":"Find <it>"}),
                ),
                json!({"type":"function_call_output","call_id":"s1","output":"Found it.",
                    "_scramjet_subagents":children}),
            ]),
            "id",
            None,
        );
        let at = |text: &str| {
            page.find(text)
                .unwrap_or_else(|| panic!("{text:?} not in the page:\n{page}"))
        };
        let order = [
            at("<b>get_scramjet_user_input</b>"),
            at("<div class=\"question markdown\"><p>Go <strong>on</strong>?</p>"),
            at("<div class=\"result\"><div class=\"text\">Answer: Yes</div>"),
            at("<b>subagent</b>"),
            at(
                "<summary>✓ <b>scout</b> (user) gpt-6-luna [Effort:low] · 2 turns ↑1.5k ↓40</summary>",
            ),
            at("<div class=\"text\">Find &lt;it&gt;</div>"),
            at("<div class=\"lines\">→ $ rg it</div>"),
            at("<p>Found <strong>it</strong>.</p>"),
        ];
        assert!(order.is_sorted(), "{order:?}\n{page}");
        assert_eq!(page.matches("<div").count(), page.matches("</div>").count());
    }
}
