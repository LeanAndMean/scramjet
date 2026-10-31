//! Scramjet's checkpoint prompts and a text view of Responses conversation items.
use serde_json::{Value, json};
use std::collections::HashSet;

pub(super) const SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub(super) const SUMMARY_SUFFIX: &str = "\n</summary>";
pub(super) const SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

const PROMPT: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

const UPDATE_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

const TURN_PREFIX_PROMPT: &str = r#"This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix."#;

pub(super) fn summary_message(summary: &str) -> Value {
    json!({"role":"user", "content":format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")})
}

/// The summary of an earlier compaction, which is always the first item.
pub(super) fn previous_summary(items: &[Value]) -> Option<&str> {
    items.first()?["content"]
        .as_str()?
        .strip_prefix(SUMMARY_PREFIX)?
        .strip_suffix(SUMMARY_SUFFIX)
}

fn text(item: &Value) -> String {
    item["content"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let parts: Vec<_> = item["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str().or(part["refusal"].as_str()))
                .collect();
            parts.join(if item["role"] == "assistant" {
                "\n"
            } else {
                ""
            })
        })
}

fn thinking(item: &Value) -> String {
    item["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Content only, including full tool output (not the truncated summarization view).
pub(super) fn estimate(items: &[Value]) -> u64 {
    items
        .iter()
        .map(|item| {
            let content = match item["type"].as_str() {
                Some("function_call") => format!(
                    "{}{}",
                    item["name"].as_str().unwrap_or(""),
                    item["arguments"].as_str().unwrap_or("")
                ),
                Some("function_call_output") => item["output"].as_str().unwrap_or("").to_owned(),
                Some("reasoning") => thinking(item),
                _ => text(item),
            };
            (content.chars().count() as u64).div_ceil(4)
        })
        .sum()
}

/// Walk backwards to ~20k tokens, then choose the next safe assistant/user boundary.
/// Responses splits an assistant's text, reasoning and calls into separate items; outstanding
/// calls prevent a cut anywhere inside a call/output group (including parallel calls).
pub(super) fn cut_point(items: &[Value], start: usize, budget: u64) -> usize {
    let mut pending = HashSet::new();
    let mut cuts = Vec::new();
    for (i, item) in items.iter().enumerate().skip(start) {
        let assistant = |item: &Value| {
            item["type"] == "reasoning"
                || item["type"] == "function_call"
                || item["role"] == "assistant"
        };
        // One Responses assistant reply can contain reasoning, text and several calls. Keep
        // these together, including encrypted reasoning paired with a function call's fc_ id.
        let inside_reply = i > start && assistant(&items[i - 1]) && assistant(item);
        if pending.is_empty() && (item["role"] == "user" || assistant(item)) && !inside_reply {
            cuts.push(i);
        }
        if let Some(id) = item["call_id"].as_str() {
            if item["type"] == "function_call" {
                pending.insert(id);
            }
            if item["type"] == "function_call_output" {
                pending.remove(id);
            }
        }
    }
    let mut tokens = 0;
    for i in (start..items.len()).rev() {
        tokens += estimate(&items[i..i + 1]);
        if tokens >= budget {
            return cuts.iter().copied().find(|&cut| cut >= i).unwrap_or(start);
        }
    }
    start
}

pub(super) fn serialize(items: &[Value]) -> String {
    let mut parts: Vec<(&str, String)> = Vec::new();
    for item in items {
        let (label, content) = match item["type"].as_str() {
            Some("reasoning") => ("Assistant thinking", thinking(item)),
            Some("function_call") => {
                let args: Value = serde_json::from_str(item["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or(Value::Null);
                let args = args
                    .as_object()
                    .map(|args| {
                        args.iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                (
                    "Assistant tool calls",
                    format!("{}({args})", item["name"].as_str().unwrap_or("")),
                )
            }
            Some("function_call_output") => {
                let output = item["output"].as_str().unwrap_or("");
                let length = output.chars().count();
                let content = if length > 2000 {
                    format!(
                        "{}\n\n[... {} more characters truncated]",
                        output.chars().take(2000).collect::<String>(),
                        length - 2000
                    )
                } else {
                    output.to_owned()
                };
                ("Tool result", content)
            }
            _ if item["role"] == "user" => ("User", text(item)),
            _ if item["role"] == "assistant" => ("Assistant", text(item)),
            _ => continue,
        };
        if !content.is_empty() {
            if label == "Assistant tool calls"
                && let Some(("Assistant tool calls", calls)) = parts.last_mut()
            {
                calls.push_str(&format!("; {content}"));
            } else {
                parts.push((label, content));
            }
        }
    }
    parts
        .into_iter()
        .map(|(label, content)| format!("[{label}]: {content}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn prompt(items: &[Value], previous: Option<&str>, focus: &str) -> String {
    let mut prompt = format!("<conversation>\n{}\n</conversation>\n\n", serialize(items));
    if let Some(summary) = previous {
        prompt.push_str(&format!(
            "<previous-summary>\n{summary}\n</previous-summary>\n\n"
        ));
    }
    prompt.push_str(if previous.is_some() {
        UPDATE_PROMPT
    } else {
        PROMPT
    });
    if !focus.is_empty() {
        prompt.push_str(&format!("\n\nAdditional focus: {focus}"));
    }
    prompt
}

/// Where the turn that `cut` falls inside began, unless the cut is at a prompt or the turn began
/// before `start`.
pub(super) fn turn_start(items: &[Value], start: usize, cut: usize) -> Option<usize> {
    if items[cut]["role"] == "user" {
        return None;
    }
    (start..cut).rev().find(|&i| items[i]["role"] == "user")
}

pub(super) fn turn_prefix_prompt(items: &[Value]) -> String {
    format!(
        "<conversation>\n{}\n</conversation>\n\n{TURN_PREFIX_PROMPT}",
        serialize(items)
    )
}

/// Scramjet's summary of a split turn: the history before it, then the turn's beginning.
pub(super) fn split_turn_summary(history: &str, prefix: &str) -> String {
    format!("{history}\n\n---\n\n**Turn Context (split turn):**\n\n{prefix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_serialization_matches_scramjet_and_bounds_tool_results() {
        let items = vec![
            json!({"role":"user", "content":[{"type":"input_text","text":"fix it"}]}),
            json!({"type":"reasoning", "summary":[{"text":"a thought"}], "encrypted_content":"not text"}),
            json!({"type":"message", "role":"assistant", "content":[{"type":"output_text","text":"reading"}]}),
            json!({"type":"function_call", "name":"read", "arguments":"{\"path\":\"a.rs\",\"limit\":2}"}),
            json!({"type":"function_call_output", "output":"é".repeat(2003)}),
        ];
        assert_eq!(
            serialize(&items),
            format!(
                "[User]: fix it\n\n[Assistant thinking]: a thought\n\n[Assistant]: reading\n\n[Assistant tool calls]: read(limit=2, path=\"a.rs\")\n\n[Tool result]: {}\n\n[... 3 more characters truncated]",
                "é".repeat(2000)
            )
        );
        assert!(estimate(&items) > 500);
    }

    #[test]
    fn cuts_on_user_or_assistant_boundaries_not_inside_parallel_calls() {
        let items = vec![
            json!({"role":"user","content":"old"}),
            json!({"type":"reasoning","summary":[{"text":"think"}]}),
            json!({"type":"function_call","call_id":"a","name":"read","arguments":"{}"}),
            json!({"type":"function_call","call_id":"b","name":"read","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"a","output":"x".repeat(8000)}),
            json!({"type":"message","role":"assistant","content":[{"text":"still pending"}]}),
            json!({"type":"function_call_output","call_id":"b","output":"ok"}),
            json!({"type":"message","role":"assistant","content":[{"text":"done"}]}),
        ];
        assert_eq!(cut_point(&items, 0, 20_000), 0);
        assert_eq!(cut_point(&items, 0, 1000), 7);
        assert_eq!(cut_point(&items, 0, estimate(&items[1..])), 1);
        assert_eq!(cut_point(&items, 0, 1), 7);
        let reply = vec![
            json!({"role":"user","content":"old"}),
            json!({"type":"reasoning","encrypted_content":"opaque"}),
            json!({"type":"message","role":"assistant","content":[{"text":"reading"}]}),
            json!({"type":"function_call","id":"fc_a","call_id":"a","name":"read","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"a","output":"large output".repeat(1000)}),
        ];
        assert_eq!(cut_point(&reply, 0, 10), 0);
        assert_eq!(cut_point(&reply, 0, estimate(&reply[2..])), 0);
        let users = vec![
            json!({"role":"user","content":"old"}),
            json!({"role":"user","content":"x".repeat(80_000)}),
        ];
        assert_eq!(cut_point(&users, 0, 20_000), 1);
    }

    #[test]
    fn prompts_use_previous_summary_and_optional_focus() {
        let items = [json!({"role":"user","content":"new work"})];
        assert_eq!(
            prompt(&items, None, ""),
            format!("<conversation>\n[User]: new work\n</conversation>\n\n{PROMPT}")
        );
        assert_eq!(
            prompt(&items, Some("old summary"), "tests"),
            format!(
                "<conversation>\n[User]: new work\n</conversation>\n\n<previous-summary>\nold summary\n</previous-summary>\n\n{UPDATE_PROMPT}\n\nAdditional focus: tests"
            )
        );
        assert_eq!(
            previous_summary(&[summary_message("summary")]),
            Some("summary")
        );
    }
}
