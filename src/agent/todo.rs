//! `todo_write`, as Claude Code's `TodoWrite` and Codex's `update_plan`: the agent keeps a
//! checklist of its work, each call sending the whole list, which the user sees as it changes
//! and with `/todos`. The list is the latest call's, read from the conversation; a compaction
//! keeps it in its summary.

use super::{FAILED, status};
use serde_json::{Value, json};

pub const NAME: &str = "todo_write";

/// Listed under the available tools.
pub(super) const SNIPPET: &str = "Keep a checklist of multi-step work, which the user sees each \
time it changes: send the whole list each time, one item in_progress while you work on it, \
marked completed as soon as it is done.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    const ALL: [Self; 3] = [Self::Pending, Self::InProgress, Self::Completed];

    pub fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: Status,
}

pub(super) fn definition() -> Value {
    let statuses: Vec<&str> = Status::ALL.iter().map(|status| status.name()).collect();
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Create or update the checklist of your current work; the user sees it \
each time it changes. Use it for a task of three or more distinct steps, or when the user gives \
several tasks at once; skip it for a single simple step or a question that needs no work. Send \
the whole list each time, every item with its status. Mark an item in_progress before you start \
it, keep one in_progress at a time, and mark it completed as soon as it is done, not several at \
the end; add the steps you discover as you go, and remove those no longer needed. An empty list \
clears it.",
        "parameters":{"type":"object","properties":{
            "todos":{"type":"array","description":"The whole list, in the order to do it",
                "items":{"type":"object","properties":{
                    "content":{"type":"string","description":"The step, in the imperative, e.g. \"Run the tests\""},
                    "status":{"type":"string","enum":statuses}},
                  "required":["content","status"]}}},
          "required":["todos"]}})
}

/// The list a call's arguments give, or what is wrong with them as Pi's validation says it.
fn parse(arguments: &Value) -> Result<Vec<Todo>, Vec<String>> {
    let Some(items) = arguments.get("todos") else {
        return Err(vec!["  - todos: must have required property".into()]);
    };
    let Some(items) = items.as_array() else {
        return Err(vec!["  - todos: must be array".into()]);
    };
    let mut errors = Vec::new();
    let mut todos = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let content = match item.get("content").map(Value::as_str) {
            None => Err("must have required property"),
            Some(None) => Err("must be string"),
            Some(Some(text)) if text.trim().is_empty() => Err("must not be empty"),
            Some(Some(text)) => Ok(one_line(text)),
        };
        let status = match item.get("status").map(Value::as_str) {
            None => Err("must have required property"),
            Some(None) => Err("must be string"),
            Some(Some(name)) => {
                Status::named(name).ok_or("must be one of pending, in_progress, completed")
            }
        };
        match (content, status) {
            (Ok(content), Ok(status)) => todos.push(Todo { content, status }),
            (content, status) => {
                for (key, result) in [("content", content.err()), ("status", status.err())] {
                    if let Some(error) = result {
                        errors.push(format!("  - todos/{index}/{key}: {error}"));
                    }
                }
            }
        }
    }
    match errors.is_empty() {
        true => Ok(todos),
        false => Err(errors),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The call line's text: how far the list is.
pub(super) fn summary(arguments: &Value) -> String {
    match parse(arguments) {
        Ok(todos) if todos.is_empty() => "cleared".into(),
        Ok(todos) => progress(&todos),
        Err(_) => String::new(),
    }
}

fn progress(todos: &[Todo]) -> String {
    let done = (todos.iter())
        .filter(|todo| todo.status == Status::Completed)
        .count();
    format!("{done} of {} completed", todos.len())
}

/// The list as the model and the user read it: one item a line, its status in brackets first.
pub fn listed(todos: &[Todo]) -> String {
    let lines: Vec<String> = (todos.iter())
        .map(|todo| format!("[{}] {}", todo.status.name(), todo.content))
        .collect();
    lines.join("\n")
}

/// An item of `listed`'s text, `None` for another line.
pub fn item(line: &str) -> Option<Todo> {
    let rest = line.strip_prefix('[')?;
    let (status, content) = rest.split_once("] ")?;
    Some(Todo {
        content: content.to_owned(),
        status: Status::named(status)?,
    })
}

/// What a call gives the model, and whether it failed.
pub(super) fn run(arguments: &Value) -> (String, bool) {
    match parse(arguments) {
        Ok(todos) if todos.is_empty() => ("Todo list cleared.".into(), false),
        Ok(todos) => {
            let text = format!(
                "Todo list updated, {}:\n{}",
                progress(&todos),
                listed(&todos)
            );
            (text, false)
        }
        Err(errors) => {
            let errors: Vec<&str> = errors.iter().map(String::as_str).collect();
            (status::validation_failed(NAME, &errors, arguments), true)
        }
    }
}

/// How a compaction's summary keeps the list.
const OPEN: &str = "\n\n<todo-list>\n";
const CLOSE: &str = "\n</todo-list>";

/// The list appended to a compaction's summary, nothing for none.
pub(super) fn kept(todos: Option<&[Todo]>) -> String {
    match todos {
        Some(todos) if !todos.is_empty() => format!("{OPEN}{}{CLOSE}", listed(todos)),
        _ => String::new(),
    }
}

/// A summary without the list `kept` appended to it.
pub(super) fn without_kept(summary: &str) -> &str {
    match summary.rfind(OPEN) {
        Some(at) if summary.ends_with(CLOSE) => &summary[..at],
        _ => summary,
    }
}

/// The list the conversation `items` last gave: the latest call's that did not fail, else what
/// the compaction at their start kept; `None` when there is none or it was cleared.
pub(super) fn latest(items: &[Value]) -> Option<Vec<Todo>> {
    let failed = |id: &Value| {
        let output = items
            .iter()
            .find(|item| item["type"] == "function_call_output" && &item["call_id"] == id);
        output.is_none_or(|output| output[FAILED] == true)
    };
    let called = items.iter().rev().find_map(|item| {
        let call = item["type"] == "function_call" && item["name"] == NAME;
        let arguments = call.then(|| serde_json::from_str(item["arguments"].as_str()?).ok())??;
        (!failed(&item["call_id"])).then(|| parse(&arguments).ok())?
    });
    let todos = called.or_else(|| {
        let summary = super::compaction::previous_summary(items)?;
        let at = summary.rfind(OPEN)?;
        let list = summary[at + OPEN.len()..].strip_suffix(CLOSE)?;
        Some(list.lines().filter_map(item).collect())
    })?;
    (!todos.is_empty()).then_some(todos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, todos: Value) -> Value {
        json!({"type":"function_call","call_id":id,"name":NAME,
            "arguments":json!({"todos":todos}).to_string()})
    }

    fn output(id: &str, failed: bool) -> Value {
        let mut output = json!({"type":"function_call_output","call_id":id,"output":"ok"});
        if failed {
            output[FAILED] = json!(true);
        }
        output
    }

    #[test]
    fn a_list_is_checked_and_listed_for_the_model() {
        let todos = json!([{"content":"Read the parser","status":"completed"},
            {"content":"Add\n the tool","status":"in_progress"},
            {"content":"Update the docs","status":"pending"}]);
        let (text, failed) = run(&json!({"todos":todos}));
        assert!(!failed);
        assert_eq!(
            text,
            "Todo list updated, 1 of 3 completed:\n[completed] Read the parser\n\
[in_progress] Add the tool\n[pending] Update the docs"
        );
        assert_eq!(summary(&json!({"todos":todos})), "1 of 3 completed");
        assert_eq!(
            run(&json!({"todos":[]})),
            ("Todo list cleared.".into(), false)
        );
        assert_eq!(summary(&json!({"todos":[]})), "cleared");

        let (text, failed) = run(&json!({"todos":[{"content":" ","status":"done"},{}]}));
        assert!(failed);
        for error in [
            "todos/0/content: must not be empty",
            "todos/0/status: must be one of pending, in_progress, completed",
            "todos/1/content: must have required property",
            "todos/1/status: must have required property",
        ] {
            assert!(text.contains(error), "{error} in {text}");
        }
        assert!(run(&json!({"todos":"x"})).1 && run(&json!({})).1);
    }

    #[test]
    fn the_list_is_the_latest_successful_calls_or_what_a_compaction_kept() {
        let first = json!([{"content":"One","status":"in_progress"}]);
        let second = json!([{"content":"One","status":"completed"},
            {"content":"Two","status":"in_progress"}]);
        let one = vec![Todo {
            content: "One".into(),
            status: Status::InProgress,
        }];
        assert_eq!(latest(&[]), None);
        let mut items = vec![call("a", first.clone()), output("a", false)];
        assert_eq!(latest(&items), Some(one.clone()));
        // A failed call, and one not answered yet, leave the list as it was.
        items.extend([
            call("b", second.clone()),
            output("b", true),
            call("c", second),
        ]);
        assert_eq!(latest(&items), Some(one.clone()));
        items.push(output("c", false));
        let two = latest(&items).unwrap();
        assert_eq!(two.len(), 2);
        items.extend([call("d", json!([])), output("d", false)]);
        assert_eq!(latest(&items), None);

        let summary = format!("Earlier work.{}", kept(Some(&two)));
        assert_eq!(without_kept(&summary), "Earlier work.");
        assert_eq!(without_kept("Earlier work."), "Earlier work.");
        let compacted = vec![super::super::compaction::summary_message(&summary)];
        assert_eq!(latest(&compacted), Some(two));
        let mut later = compacted.clone();
        later.extend([call("e", first), output("e", false)]);
        assert_eq!(latest(&later), Some(one));
        assert_eq!(kept(Some(&[])), "");
        assert_eq!(kept(None), "");
    }
}
