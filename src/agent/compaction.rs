//! Compaction as Scramjet does it: its checkpoint prompts, a text view of Responses conversation
//! items, and the agent replacing older history with a summary, on `/compact` or when the context
//! fills.
use super::{Agent, AgentError, AgentUi, PRODUCER, Purpose, message_text};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::AtomicBool;

pub(super) const SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub(super) const SUMMARY_SUFFIX: &str = "\n</summary>";
/// How a branch summary is sent: Scramjet's `BRANCH_SUMMARY_PREFIX`/`SUFFIX`.
const BRANCH_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
const BRANCH_SUFFIX: &str = "</summary>";
/// Begins every branch summary the model writes, as Scramjet's `BRANCH_SUMMARY_PREAMBLE`.
const BRANCH_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";
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

const BRANCH_PROMPT: &str = r#"Create a structured summary of this conversation branch for context when returning later.

Use this EXACT format:

## Goal
[What was the user trying to accomplish in this branch?]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Work that was started but not finished]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next to continue this work]

Keep each section concise. Preserve exact file paths, function names, and error messages."#;

/// Scramjet's `reserveTokens`: automatic compaction runs once the context passes what is left
/// of the model's window after this much is kept for the next response.
pub(super) const RESERVE_TOKENS: u64 = 16_384;

/// Whether a context of `tokens` should be compacted for a model whose window is `window`, as
/// Scramjet's `shouldCompact`: a window no larger than the reserve never compacts.
pub(super) fn should_compact(tokens: u64, window: u64) -> bool {
    window > RESERVE_TOKENS && tokens > window - RESERVE_TOKENS
}

/// The paths a summarized conversation gave to read, write and edit calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct FileLists {
    /// Read and never written or edited.
    pub read: Vec<String>,
    pub modified: Vec<String>,
}

impl FileLists {
    /// The lists a compaction or branch summary entry saved.
    pub fn of(entry: &Value) -> Self {
        let paths = |key: &str| {
            entry[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect()
        };
        Self {
            read: paths("read_files"),
            modified: paths("modified_files"),
        }
    }

    /// Scramjet's `computeFileLists`: `self` (an earlier compaction's lists) and the paths of
    /// the calls in `items` as given, each list sorted.
    pub fn with_calls(&self, items: &[Value]) -> Self {
        let mut read: BTreeSet<String> = self.read.iter().cloned().collect();
        let mut modified: BTreeSet<String> = self.modified.iter().cloned().collect();
        for item in items.iter().filter(|item| item["type"] == "function_call") {
            let Ok(arguments) =
                serde_json::from_str::<Value>(item["arguments"].as_str().unwrap_or_default())
            else {
                continue;
            };
            let Some(path) = arguments["path"].as_str().filter(|path| !path.is_empty()) else {
                continue;
            };
            match item["name"].as_str() {
                Some("read") => read.insert(path.to_owned()),
                Some("write" | "edit") => modified.insert(path.to_owned()),
                _ => false,
            };
        }
        read.retain(|path| !modified.contains(path));
        Self {
            read: read.into_iter().collect(),
            modified: modified.into_iter().collect(),
        }
    }

    /// Scramjet's `formatFileOperations`, appended to a summary.
    pub fn format(&self) -> String {
        let mut sections = Vec::new();
        if !self.read.is_empty() {
            sections.push(format!(
                "<read-files>\n{}\n</read-files>",
                self.read.join("\n")
            ));
        }
        if !self.modified.is_empty() {
            sections.push(format!(
                "<modified-files>\n{}\n</modified-files>",
                self.modified.join("\n")
            ));
        }
        if sections.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", sections.join("\n\n"))
        }
    }
}

pub(super) fn summary_message(summary: &str) -> Value {
    json!({"role":"user", "content":format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")})
}

/// Whether a user message is a compaction or branch summary, which Scramjet does not count as a
/// user message.
pub(super) fn is_summary(item: &Value) -> bool {
    item["content"]
        .as_str()
        .is_some_and(|text| text.starts_with(SUMMARY_PREFIX) || text.starts_with(BRANCH_PREFIX))
}

pub(super) fn branch_summary_message(summary: &str) -> Value {
    json!({"role":"user", "content":format!("{BRANCH_PREFIX}{summary}{BRANCH_SUFFIX}")})
}

/// The summary in a branch summary's message.
pub(super) fn branch_summary(item: &Value) -> Option<&str> {
    item["content"]
        .as_str()?
        .strip_prefix(BRANCH_PREFIX)?
        .strip_suffix(BRANCH_SUFFIX)
}

/// A branch left by a move in the session tree, as Scramjet's `prepareBranchEntries` collects
/// it for its summary.
pub(super) struct Branch {
    /// Its conversation items without tool results (the calls tell enough), with compaction and
    /// branch summaries as they are sent.
    items: Vec<Value>,
    /// The file lists its branch summaries saved.
    files: FileLists,
}

impl Branch {
    /// The branch of the session entries `entries`.
    pub fn of(entries: &[Value]) -> Self {
        let mut files = FileLists::default();
        let mut items = Vec::new();
        for entry in entries {
            match entry["type"].as_str() {
                Some(
                    "function_call_output"
                    | "model_change"
                    | "thinking_level_change"
                    | "command_state",
                ) => {}
                Some("compaction") => items.push(summary_message(
                    entry["summary"].as_str().unwrap_or_default(),
                )),
                Some("branch_summary") => {
                    let saved = FileLists::of(entry);
                    files.read.extend(saved.read);
                    files.modified.extend(saved.modified);
                    items.push(branch_summary_message(
                        entry["summary"].as_str().unwrap_or_default(),
                    ));
                }
                _ => items.push(entry.clone()),
            }
        }
        Self { items, files }
    }

    /// Whether it has anything for a summary to tell.
    pub fn is_empty(&self) -> bool {
        serialize(&self.items).is_empty()
    }

    /// The files it read and modified, its branch summaries' lists included.
    pub fn files(&self) -> FileLists {
        self.files.with_calls(&self.items)
    }

    /// The prompt for its summary, with its latest items that fit in `budget` tokens, as Scramjet
    /// keeps a summary that does not fit if the rest leaves a tenth of the budget; `None` when
    /// not even the last item fits.
    pub fn prompt(&self, budget: u64, focus: &str) -> Option<String> {
        let items = &self.items;
        let mut start = items.len();
        let mut tokens = 0;
        while start > 0 {
            let item = &items[start - 1];
            let size = estimate(std::slice::from_ref(item));
            if tokens + size > budget {
                let summary = previous_summary(std::slice::from_ref(item)).is_some()
                    || branch_summary(item).is_some();
                if summary && tokens * 10 < budget * 9 {
                    start -= 1;
                }
                break;
            }
            tokens += size;
            start -= 1;
        }
        if start == items.len() {
            return None;
        }
        let mut prompt = format!(
            "<conversation>\n{}\n</conversation>\n\n{BRANCH_PROMPT}",
            serialize(&items[start..])
        );
        if !focus.is_empty() {
            prompt.push_str(&format!("\n\nAdditional focus: {focus}"));
        }
        Some(prompt)
    }
}

/// The branch summary saved for a branch, from the model's `text`, as Scramjet writes it.
pub(super) fn branch_summary_text(text: &str, files: &FileLists) -> String {
    format!("{BRANCH_PREAMBLE}{text}{}", files.format())
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

/// Compaction keeps about this much of the latest conversation, as Scramjet does.
const KEEP_RECENT_TOKENS: u64 = 20_000;

#[derive(Clone)]
pub struct Compaction {
    /// The last response's context size and an estimate of the items after it, or without a
    /// response of the current model an estimate of the whole conversation (chars/4).
    pub tokens_before: u64,
    pub kept_items: usize,
    pub summary: String,
}

/// Why compaction ran without `/compact`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AutoCompaction {
    /// The last response's context passed the model's window minus `RESERVE_TOKENS`.
    Threshold,
    /// A request failed because its input exceeded the model's window.
    Overflow,
}

impl AutoCompaction {
    /// A short line saying why compaction ran, for the user.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Threshold => "Automatic: the context was nearing the model's window.",
            Self::Overflow => "Automatic: a request exceeded the model's context window.",
        }
    }
}

impl Agent {
    /// Replaces the conversation before its last ~20k tokens with a summary the model writes, as
    /// Scramjet's compaction does, followed by the files read and modified in all compacted
    /// history, and records that in the session. On failure or cancellation
    /// nothing changes. `None`: nothing is old enough to summarize.
    pub fn compact(
        &mut self,
        focus: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Option<Compaction>, AgentError> {
        let previous = previous_summary(&self.conversation).map(str::to_owned);
        let start = usize::from(previous.is_some());
        let cut = cut_point(&self.conversation, start, KEEP_RECENT_TOKENS);
        if cut == start {
            return Ok(None);
        }
        // As in Scramjet, a turn cut in two is summarized apart from the history before it.
        let turn = turn_start(&self.conversation, start, cut);
        let history = &self.conversation[start..turn.unwrap_or(cut)];
        let previous_files = self.session.compacted_files();
        let files = previous_files.with_calls(&self.conversation[start..cut]);
        let mut summary = match previous {
            Some(previous) if history.is_empty() => previous
                .strip_suffix(&previous_files.format())
                .unwrap_or(&previous)
                .to_owned(),
            _ if history.is_empty() => "No prior history.".into(),
            previous => {
                let prompt = prompt(history, previous.as_deref(), focus);
                self.summarize(prompt, Purpose::Compaction, ui, cancel)?
            }
        };
        if let Some(turn) = turn {
            let prompt = turn_prefix_prompt(&self.conversation[turn..cut]);
            let prefix = self.summarize(prompt, Purpose::Compaction, ui, cancel)?;
            summary = split_turn_summary(&summary, &prefix);
        }
        summary.push_str(&files.format());
        let tokens_before = self
            .context_size()
            .unwrap_or_else(|| estimate(&self.conversation));
        self.session.append(
            &json!({"type":"compaction", "summary":summary, "first_kept":cut,
                "read_files":files.read, "modified_files":files.modified,
                "tokens_before":tokens_before}),
        )?;
        let kept = self.conversation.split_off(cut);
        self.conversation = vec![summary_message(&summary)];
        self.conversation.extend(kept);
        // Unknown until the next response reports it.
        self.usage.context_tokens = None;
        Ok(Some(Compaction {
            tokens_before,
            kept_items: self.conversation.len() - 1,
            summary,
        }))
    }

    /// One request without tools for the text a summarization `prompt` asks for.
    pub(super) fn summarize(
        &mut self,
        prompt: String,
        purpose: Purpose,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<String, AgentError> {
        let mut request = json!({
            "model": self.config.model,
            "instructions": SYSTEM_PROMPT,
            "input": [{"role":"user", "content":prompt}],
            "stream": true,
            "store": false,
            "prompt_cache_key": self.session.id(),
            "reasoning": {"summary": "auto"},
        });
        if let Some(effort) = self.effort() {
            request["reasoning"]["effort"] = json!(effort);
        }
        if let Some(tokens) = self.config.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        let reply = self.respond(&request, purpose, ui, cancel)?;
        let summary = reply
            .items
            .iter()
            .filter(|item| item["type"] == "message")
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n");
        if summary.trim().is_empty() {
            return Err(AgentError::Provider("the model returned no summary".into()));
        }
        Ok(summary)
    }

    /// Compacts as `/compact` without a focus does and shows the result as `/compact` does,
    /// saying why it ran. Returns whether there was anything to compact.
    pub(super) fn auto_compact(
        &mut self,
        reason: AutoCompaction,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<bool, AgentError> {
        match self.compact("", ui, cancel) {
            Ok(Some(compaction)) => {
                ui.compacted(&compaction, Some(reason));
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
            Err(error) => Err(AgentError::CompactionFailed(Box::new(error))),
        }
    }

    /// As Scramjet's threshold check: compacts after a turn once the context passed the model's
    /// window minus `RESERVE_TOKENS`. Without a known window it does nothing.
    pub(super) fn compact_when_full(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let (Some(tokens), Some(window)) = (self.usage.context_tokens, self.info.window) else {
            return Ok(());
        };
        if should_compact(tokens, window) {
            self.auto_compact(AutoCompaction::Threshold, ui, cancel)?;
        }
        Ok(())
    }

    /// As Scramjet counts the context: the last response's, if the current model gave it, and an
    /// estimate of the items added after it, such as tool output from a cancelled turn.
    fn context_size(&self) -> Option<u64> {
        let tokens = self.usage.context_tokens?;
        let last = self
            .conversation
            .iter()
            .rposition(|item| item.get(PRODUCER).is_some())?;
        (self.conversation[last][PRODUCER] == self.config.model.as_str())
            .then(|| tokens + estimate(&self.conversation[last + 1..]))
    }

    /// As Scramjet's check before a new prompt, which catches a turn that ended without a reply.
    pub(super) fn compact_before_prompt(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let (Some(tokens), Some(window)) = (self.context_size(), self.info.window) else {
            return Ok(());
        };
        if !should_compact(tokens, window) {
            return Ok(());
        }
        match self.auto_compact(AutoCompaction::Threshold, ui, cancel) {
            Err(AgentError::CompactionFailed(error)) => {
                ui.compaction_failed(&error);
                Ok(())
            }
            result => result.map(drop),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{AgentConfig, Session, SessionChoice};
    use crate::provider_fixture as fixture;

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
    fn file_lists_merge_earlier_ones_and_list_modified_files_once() {
        let call = |name: &str, arguments: &str| json!({"type":"function_call","name":name,"arguments":arguments});
        let earlier = FileLists {
            read: vec!["z.rs".into(), "w.rs".into()],
            modified: vec!["m.rs".into()],
        };
        let lists = earlier.with_calls(&[
            call("read", r#"{"path":"b.rs","offset":3}"#),
            call("read", r#"{"path":"a.rs"}"#),
            call("read", r#"{"path":"a.rs"}"#),
            call("edit", r#"{"path":"w.rs","edits":[]}"#),
            call("write", r#"{"path":"~/n.rs","content":""}"#),
            call("bash", r#"{"path":"x.rs","command":"ls"}"#),
            call("read", r#"{"path":""}"#),
            call("read", "not json"),
            json!({"type":"function_call_output","name":"read","output":r#"{"path":"o.rs"}"#}),
        ]);
        assert_eq!(lists.read, ["a.rs", "b.rs", "z.rs"]);
        assert_eq!(lists.modified, ["m.rs", "w.rs", "~/n.rs"]);
        assert_eq!(
            lists.format(),
            "\n\n<read-files>\na.rs\nb.rs\nz.rs\n</read-files>\n\n<modified-files>\nm.rs\nw.rs\n~/n.rs\n</modified-files>"
        );
        assert_eq!(FileLists::default().format(), "");
        let modified_only = FileLists::default().with_calls(&[call("edit", r#"{"path":"e.rs"}"#)]);
        assert_eq!(
            modified_only.format(),
            "\n\n<modified-files>\ne.rs\n</modified-files>"
        );
    }

    #[test]
    fn a_branch_keeps_its_latest_items_within_the_budget_and_squeezes_in_summaries() {
        let reply =
            |text: &str| json!({"type":"message","role":"assistant","content":[{"text":text}]});
        let user = json!({"role":"user","content":"a".repeat(400)});
        let compaction = json!({"type":"compaction","summary":"s".repeat(400),"first_kept":0});
        let entries = [
            json!({"type":"model_change","model":"m"}),
            user.clone(),
            json!({"type":"function_call_output","call_id":"c","output":"hidden"}),
            reply(&"b".repeat(40)),
        ];
        let branch = Branch::of(&entries);
        let conversation = |prompt: Option<String>| {
            prompt.map(|prompt| prompt.split("\n</conversation>").next().unwrap().to_owned())
        };
        assert_eq!(
            conversation(branch.prompt(50, "")),
            Some(format!("<conversation>\n[Assistant]: {}", "b".repeat(40)))
        );
        assert!(
            conversation(branch.prompt(110, ""))
                .unwrap()
                .contains("[User]: aaa")
        );
        assert_eq!(branch.prompt(5, ""), None);
        let summarized = Branch::of(&[compaction, reply(&"b".repeat(40))]);
        assert!(
            conversation(summarized.prompt(50, ""))
                .unwrap()
                .contains("<summary>\nsss")
        );
        assert!(
            !conversation(summarized.prompt(11, ""))
                .unwrap()
                .contains("sss")
        );
        assert!(Branch::of(&entries[..1]).is_empty());
        assert!(!Branch::of(&entries[1..2]).is_empty());
    }

    #[test]
    fn compaction_runs_past_the_window_less_the_reserve() {
        assert!(!should_compact(20_000, 36_384));
        assert!(should_compact(20_001, 36_384));
        assert!(should_compact(u64::MAX, u64::MAX));
        assert!(!should_compact(1_000_000, RESERVE_TOKENS));
        assert!(!should_compact(0, 0));
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

    #[test]
    fn compaction_summarizes_older_items_and_resumes_compacted() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events(
                "first summary",
                fixture::known_usage(),
            )),
            stream(&fixture::text_events(
                "updated summary",
                fixture::known_usage(),
            )),
            stream(&fixture::text_events("continued", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "compact");
        let add_call = |agent: &mut Agent, name: &str, path: &str| {
            let arguments = json!({"path":path}).to_string();
            let id = format!("{name}-{path}");
            let call =
                json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments});
            agent.add(call).unwrap();
            let output = json!({"type":"function_call_output","call_id":id,"output":"ok"});
            agent.add(output).unwrap();
        };
        let old = json!({"role":"user", "content":"remember the goal"});
        let recent = json!({"role":"user", "content":"recent work ".repeat(7000)});
        agent.add(old).unwrap();
        add_call(&mut agent, "read", "a.rs");
        add_call(&mut agent, "edit", "b.rs");
        add_call(&mut agent, "bash", "ignored.rs");
        agent.add(recent.clone()).unwrap();
        // No response of the current model gave this count, so the conversation is estimated.
        agent.usage.context_tokens = Some(30_000);
        let estimate = estimate(agent.conversation());
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let first = agent
            .compact("file paths", &mut ui, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!((first.kept_items, first.tokens_before), (1, estimate));
        assert_eq!(
            first.summary,
            "first summary\n\n<read-files>\na.rs\n</read-files>\n\n<modified-files>\nb.rs\n</modified-files>"
        );
        assert_eq!(agent.usage.context_tokens, None);
        assert_eq!(agent.conversation()[1], recent);
        assert_eq!(
            agent.conversation()[0]["content"],
            format!("{}{}{}", SUMMARY_PREFIX, first.summary, SUMMARY_SUFFIX)
        );
        let (session, resumed, _) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, agent.conversation());
        assert_eq!(session.context_tokens(), None);
        agent.session = session;
        add_call(&mut agent, "read", "c.rs");
        add_call(&mut agent, "read", "b.rs");
        let newest = json!({"role":"user", "content":"newer work ".repeat(8000)});
        agent.add(newest.clone()).unwrap();
        let second = agent.compact("", &mut ui, &cancel).unwrap().unwrap();
        assert_eq!(second.kept_items, 1);
        assert!(second.tokens_before > 20_000);
        assert_eq!(
            second.summary,
            "updated summary\n\n<read-files>\na.rs\nc.rs\n</read-files>\n\n<modified-files>\nb.rs\n</modified-files>"
        );
        let (session, resumed, prompts) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, agent.conversation());
        assert_eq!(prompts.len(), 3);
        agent.session = session;
        agent.conversation = resumed;
        agent.turn("continue", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        for request in &requests[..2] {
            assert_eq!(request["instructions"], SYSTEM_PROMPT);
            assert!(request.get("tools").is_none());
            assert!(request.get("tool_choice").is_none());
            assert_eq!(request["input"].as_array().unwrap().len(), 1);
        }
        let first_prompt = requests[0]["input"][0]["content"].as_str().unwrap();
        assert!(first_prompt.contains("[User]: remember the goal"));
        assert!(!first_prompt.contains("recent work"));
        assert!(first_prompt.ends_with("Additional focus: file paths"));
        let second_prompt = requests[1]["input"][0]["content"].as_str().unwrap();
        assert!(second_prompt.contains(&format!(
            "<previous-summary>\n{}\n</previous-summary>",
            first.summary
        )));
        assert!(second_prompt.contains("[User]: recent work"));
        assert!(!second_prompt.contains("newer work"));
        assert_eq!(requests[2]["input"][1], newest);
        assert_eq!(
            requests[2]["input"][0]["content"],
            format!("{}{}{}", SUMMARY_PREFIX, second.summary, SUMMARY_SUFFIX)
        );
        assert!(requests[2]["input"][0].get(PRODUCER).is_none());
        assert_eq!(ui.replies, ["continued"]);
        assert!(ui.calls.is_empty());
    }

    #[test]
    fn a_turn_cut_in_two_is_summarized_apart_from_the_history_before_it() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("history", fixture::known_usage())),
            stream(&fixture::text_events("prefix", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "compact-split");
        let call = |id: &str, name: &str| json!({"type":"function_call","call_id":id,"name":name,"arguments":json!({"path":format!("{id}.rs")}).to_string()});
        let output = |id: &str, text: String| json!({"type":"function_call_output","call_id":id,"output":text});
        for item in [
            json!({"role":"user","content":"old goal"}),
            json!({"type":"message","role":"assistant","content":[{"text":"noted"}]}),
            json!({"role":"user","content":"long task"}),
            call("a", "read"),
            output("a", "x".repeat(90_000)),
            call("b", "edit"),
            output("b", "y".repeat(1000)),
            json!({"type":"message","role":"assistant","content":[{"text":"done"}]}),
        ] {
            agent.add(item).unwrap();
        }
        let mut ui = Recorder::default();
        let compacted = agent
            .compact("focus", &mut ui, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(compacted.kept_items, 3);
        assert_eq!(
            compacted.summary,
            "history\n\n---\n\n**Turn Context (split turn):**\n\nprefix\n\n<read-files>\na.rs\n</read-files>"
        );
        assert_eq!(agent.conversation()[1]["call_id"], "b");
        let requests = server.join().unwrap();
        let prompt = |n: usize| {
            requests[n]["input"][0]["content"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(prompt(0).contains("[User]: old goal\n\n[Assistant]: noted\n</conversation>"));
        assert!(prompt(0).ends_with("Additional focus: focus"));
        assert!(prompt(1).starts_with(
            "<conversation>\n[User]: long task\n\n[Assistant tool calls]: read(path=\"a.rs\")"
        ));
        assert!(prompt(1).contains("[... 88000 more characters truncated]\n</conversation>"));
        assert!(prompt(1).ends_with("Focus on what's needed to understand the kept suffix."));
    }

    #[test]
    fn a_reused_summary_keeps_one_file_list() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "prefix",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "compact-reuse");
        let previous = "earlier\n\n<read-files>\na.rs\n</read-files>";
        agent
            .session
            .append(&json!({"type":"compaction","summary":previous,"first_kept":0,"read_files":["a.rs"]}))
            .unwrap();
        agent.conversation = vec![summary_message(previous)];
        let call = |id: &str| json!({"type":"function_call","call_id":id,"name":"write","arguments":json!({"path":"a.rs"}).to_string()});
        let output = |id: &str, text: String| json!({"type":"function_call_output","call_id":id,"output":text});
        for item in [
            json!({"role":"user","content":"long task"}),
            call("a"),
            output("a", "x".repeat(90_000)),
            call("b"),
            output("b", "ok".into()),
        ] {
            agent.add(item).unwrap();
        }
        let compacted = agent
            .compact("", &mut Recorder::default(), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(
            compacted.summary,
            "earlier\n\n---\n\n**Turn Context (split turn):**\n\nprefix\n\n<modified-files>\na.rs\n</modified-files>"
        );
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[test]
    fn failed_or_textless_compactions_leave_history_unchanged() {
        let mut failed = fixture::text_events("partial", fixture::known_usage());
        let terminal = failed.last_mut().unwrap();
        terminal["type"] = json!("response.failed");
        terminal["response"]["status"] = json!("failed");
        terminal["response"]["error"] = json!({"code":"invalid_prompt","message":"Invalid prompt"});
        let (endpoint, server) = serve(vec![
            stream(&failed),
            stream(&fixture::text_events(" ", fixture::known_usage())),
            stream(&fixture::call_events("{}", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "compact-failure");
        agent.add(json!({"role":"user","content":"old"})).unwrap();
        agent
            .add(json!({"role":"user","content":"recent ".repeat(12000)}))
            .unwrap();
        let before = agent.conversation().to_vec();
        let log = std::fs::read(agent.session_path()).unwrap();
        let mut ui = Recorder::default();
        for _ in 0..3 {
            assert!(agent.compact("", &mut ui, &AtomicBool::new(false)).is_err());
            assert_eq!(agent.conversation(), before);
            assert_eq!(std::fs::read(agent.session_path()).unwrap(), log);
        }
        assert!(matches!(
            agent.compact("", &mut ui, &AtomicBool::new(true)),
            Err(AgentError::Cancelled)
        ));
        assert_eq!(agent.conversation(), before);
        assert_eq!(server.join().unwrap().len(), 3);
        assert!(ui.calls.is_empty());
        assert!(ui.replies.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn short_history_compaction_is_a_no_op_without_a_request() {
        let (mut agent, _scratch) = agent(([127, 0, 0, 1], 9).into(), "compact-short");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        assert!(agent.compact("", &mut ui, &cancel).unwrap().is_none());
        agent.add(json!({"role":"user","content":"hi"})).unwrap();
        let log = std::fs::read(agent.session_path()).unwrap();
        assert!(agent.compact("", &mut ui, &cancel).unwrap().is_none());
        assert_eq!(std::fs::read(agent.session_path()).unwrap(), log);
        assert_eq!(agent.usage.requests, 0);
    }

    /// Usage of a response that filled `total` tokens of context.
    fn full_usage(total: u64) -> Value {
        json!({"input_tokens": total - 10, "output_tokens": 10, "total_tokens": total})
    }

    #[test]
    fn a_resumed_session_knows_its_context_size_until_a_compaction() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("first answer", full_usage(29_000))),
            stream(&fixture::text_events("summary", fixture::known_usage())),
            stream(&fixture::text_events("continued", fixture::known_usage())),
        ]);
        let (mut first, scratch) = agent(endpoint, "resume-context");
        let cancel = AtomicBool::new(false);
        first
            .turn("initial", &mut Recorder::default(), &cancel)
            .unwrap();
        first
            .add(json!({"role":"user", "content":"recent work ".repeat(7000)}))
            .unwrap();
        let path = first.session_path().to_owned();
        drop(first);

        let config = AgentConfig {
            endpoint,
            model: "m".into(),
            root: scratch.0.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
        };
        let mut resumed = Agent::start(config, SessionChoice::Open(path.clone())).unwrap();
        assert_eq!(resumed.usage.context_tokens, Some(29_000));
        resumed.set_context_window(Some(30_000));
        let mut ui = Recorder::default();
        resumed.turn("continue", &mut ui, &cancel).unwrap();
        assert_eq!(ui.compactions.len(), 1);
        assert_eq!(ui.compactions[0].2, Some(AutoCompaction::Threshold));
        assert_eq!(ui.replies, ["continued"]);
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1]["instructions"], SYSTEM_PROMPT);
        assert!(!requests[2].to_string().contains("\"_scramjet_"));
        let (session, _, _) = Session::open(&path).unwrap();
        assert_eq!(session.context_tokens(), Some(15));

        // Saved before the compaction, the first answer's count is not trusted after it.
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let compaction = lines
            .iter()
            .position(|line| line.contains("\"type\":\"compaction\""))
            .unwrap();
        assert!(
            lines[..compaction]
                .concat()
                .contains("\"_scramjet_context_tokens\":29000")
        );
        let cut = scratch.0.join("cut.jsonl");
        std::fs::write(&cut, lines[..=compaction].join("\n")).unwrap();
        assert_eq!(Session::open(&cut).unwrap().0.context_tokens(), None);
    }

    /// A conversation with a small old prompt and ~21k tokens of recent work to keep.
    fn seed_long_conversation(agent: &mut Agent) {
        agent
            .add(json!({"role":"user", "content":"remember the goal"}))
            .unwrap();
        agent
            .add(json!({"role":"user", "content":"recent work ".repeat(7000)}))
            .unwrap();
    }

    #[test]
    fn a_full_context_is_compacted_after_the_turn() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("done", full_usage(10_000))),
            stream(&fixture::text_events("summary", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-threshold");
        seed_long_conversation(&mut agent);
        agent.set_context_window(Some(20_000));
        let mut ui = Recorder::default();
        agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["done"]);
        assert_eq!(
            ui.compactions,
            [(10_000, 3, Some(AutoCompaction::Threshold))]
        );
        assert_eq!(ui.waits, [Purpose::Reply, Purpose::Compaction]);
        // The turn ends after compacting: no further request is sent.
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["instructions"], SYSTEM_PROMPT);
        assert!(
            requests[1]["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains("[User]: remember the goal")
        );
        assert_eq!(agent.usage.context_tokens, None);
        assert!(ui.calls.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn a_failed_automatic_compaction_is_reported_as_such_after_the_reply() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("done", full_usage(10_000))),
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 4\r\n\r\nnope".to_vec(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-threshold-failed");
        seed_long_conversation(&mut agent);
        agent.set_context_window(Some(20_000));
        let mut ui = Recorder::default();
        let error = agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "automatic compaction failed: provider: HTTP 400 Bad Request: nope"
        );
        assert_eq!(server.join().unwrap().len(), 2);
        assert_eq!(ui.replies, ["done"]);
        assert!(ui.compactions.is_empty());
        assert_eq!(agent.conversation().len(), 4);
    }

    #[test]
    fn a_turn_left_without_a_reply_is_compacted_before_the_next_prompt() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events(
                "counted by another model",
                full_usage(100),
            )),
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 4\r\n\r\nnope".to_vec(),
            stream(&fixture::text_events("sent anyway", full_usage(100))),
            stream(&fixture::text_events("summary", full_usage(100))),
            stream(&fixture::text_events("compacted first", full_usage(100))),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-before-prompt");
        agent.set_context_window(Some(40_000));
        // A turn cancelled after a tool call: its output is not in the last response's count.
        let cancelled = |producer: &str| {
            vec![
                json!({"role":"user","content":"old goal"}),
                json!({"type":"message","role":"assistant","content":[{"text":"noted"}],"_scramjet_model":producer}),
                json!({"role":"user","content":"big task ".repeat(9000)}),
                json!({"type":"function_call","call_id":"a","name":"read","arguments":"{}","_scramjet_model":producer}),
                json!({"type":"function_call_output","call_id":"a","output":"y".repeat(20_000)}),
            ]
        };
        let mut ui = Recorder::default();
        for (producer, prompt) in [("other", "one"), ("m", "two"), ("m", "three")] {
            agent.conversation = cancelled(producer);
            agent.usage.context_tokens = Some(20_000);
            agent
                .turn(prompt, &mut ui, &AtomicBool::new(false))
                .unwrap();
        }
        assert_eq!(
            ui.replies,
            ["counted by another model", "sent anyway", "compacted first"]
        );
        assert_eq!(
            ui.failed_compactions,
            ["provider: HTTP 400 Bad Request: nope"]
        );
        assert_eq!(
            ui.compactions,
            [(25_000, 3, Some(AutoCompaction::Threshold))]
        );
        assert_eq!(
            ui.waits,
            [
                Purpose::Reply,
                Purpose::Compaction,
                Purpose::Reply,
                Purpose::Compaction,
                Purpose::Reply
            ]
        );
        let requests = server.join().unwrap();
        assert_eq!(requests[0]["input"].as_array().unwrap().len(), 6);
        assert_eq!(requests[2]["input"].as_array().unwrap().len(), 6);
        assert_eq!(requests[2]["input"][5]["content"], "two");
        assert!(
            requests[3]["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains("[User]: old goal\n\n[Assistant]: noted\n</conversation>")
        );
        let input = requests[4]["input"].as_array().unwrap();
        assert_eq!(input.len(), 5);
        assert_eq!(input[0], summary_message("summary"));
        assert_eq!(input[4]["content"], "three");
    }

    #[test]
    fn the_threshold_needs_a_known_window_and_a_full_enough_context() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "done",
            full_usage(1_000_000),
        ))]);
        let (mut unknown, _scratch) = agent(endpoint, "auto-threshold-unknown");
        let mut ui = Recorder::default();
        unknown
            .turn("hi", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());

        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "done",
            fixture::known_usage(),
        ))]);
        let (mut low, _scratch) = agent(endpoint, "auto-threshold-low");
        low.set_context_window(Some(20_000));
        low.turn("hi", &mut ui, &AtomicBool::new(false)).unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());
    }

    /// A stream that ends with the provider saying the input exceeded the context window.
    fn overflow_events() -> Vec<Value> {
        let mut events = fixture::text_events("partial", fixture::known_usage());
        events.truncate(4);
        events.push(json!({"type":"error","code":"context_length_exceeded",
            "message":"Your input exceeds the context window of this model."}));
        events
    }

    #[test]
    fn an_overflowing_request_is_compacted_and_sent_again() {
        let (endpoint, server) = serve(vec![
            stream(&overflow_events()),
            stream(&fixture::text_events("summary", fixture::known_usage())),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-overflow");
        seed_long_conversation(&mut agent);
        let mut ui = Recorder::default();
        agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["done"]);
        assert!(matches!(
            ui.compactions[..],
            [(_, 2, Some(AutoCompaction::Overflow))]
        ));
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1]["instructions"], SYSTEM_PROMPT);
        let retried = requests[2]["input"].as_array().unwrap();
        assert_eq!(
            retried[0]["content"],
            format!("{}summary{}", SUMMARY_PREFIX, SUMMARY_SUFFIX)
        );
        assert_eq!(retried[2], json!({"role":"user","content":"continue"}));
        assert!(ui.calls.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn a_second_overflow_fails_the_turn_with_a_clear_message() {
        let (endpoint, server) = serve(vec![
            stream(&overflow_events()),
            stream(&fixture::text_events("summary", fixture::known_usage())),
            stream(&overflow_events()),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-overflow-twice");
        seed_long_conversation(&mut agent);
        let mut ui = Recorder::default();
        let error = agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "context overflow: provider error: Your input exceeds the context window of this \
model. (again after compacting once; reduce the context or switch to a larger-context model)"
        );
        assert_eq!(server.join().unwrap().len(), 3);
        assert_eq!(ui.compactions.len(), 1);
        assert!(ui.replies.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn an_overflow_that_cannot_compact_fails_the_turn() {
        let (endpoint, server) = serve(vec![stream(&overflow_events())]);
        let (mut agent, _scratch) = agent(endpoint, "auto-overflow-nothing");
        let mut ui = Recorder::default();
        let error = agent
            .turn("hi", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "context overflow: provider error: Your input exceeds the context window of this \
model. (nothing is old enough to compact)"
        );
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());
        assert!(ui.replies.is_empty());
        assert!(ui.retries.is_empty());
    }
}
