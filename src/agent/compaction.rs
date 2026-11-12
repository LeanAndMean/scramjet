//! Compaction as Scramjet does it: its checkpoint prompts, a text view of Responses conversation
//! items, and the agent replacing older history with a summary, on `/compact` or when the context
//! fills.
use super::client::Protocol;
use super::{
    Agent, AgentError, AgentUi, CALIBRATION, Compacting, PRODUCER, Purpose, USAGE, message_text,
    tools,
};
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

/// Whether a context of `tokens` should be compacted for a model whose window is `window`, less
/// the `reserve` kept for the next response, as Scramjet's `shouldCompact`, or that takes at most
/// `input` tokens, as its `maxInputTokens` check: a window no larger than the reserve never
/// compacts.
pub(super) fn should_compact(tokens: u64, window: u64, input: Option<u64>, reserve: u64) -> bool {
    window > reserve && (tokens > window - reserve || input.is_some_and(|input| tokens > input))
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
                    | "command_state"
                    | "autopilot",
                ) => {}
                Some("function_call") if entry["name"] == super::status::RECORD => {}
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

/// A Responses request's tokens at chars/4: its instructions, tools and input.
pub(super) fn request_estimate(request: &Value) -> u64 {
    let tools = request
        .get("tools")
        .map_or(0, |tools| tools.to_string().chars().count());
    let instructions = request["instructions"].as_str().unwrap_or_default();
    let input = request["input"].as_array().map_or(&[][..], Vec::as_slice);
    (instructions.chars().count() + tools).div_ceil(4) as u64 + estimate(input)
}

/// Cuts the text outputs of `input`'s tool results, largest first, until together they hold
/// at most `budget` characters, each cut one keeping its start and end around `note` of the
/// characters it lost. Returns how many were cut.
fn fit_outputs(input: &mut [Value], budget: u64, note: impl Fn(usize) -> String) -> usize {
    let sizes: Vec<(usize, usize)> = (input.iter().enumerate())
        .filter(|(_, item)| item["type"] == "function_call_output")
        .filter_map(|(i, item)| Some((i, item["output"].as_str()?.chars().count())))
        .collect();
    let budget = budget as usize;
    let total: usize = sizes.iter().map(|&(_, size)| size).sum();
    if total <= budget {
        return 0;
    }
    let room = note(usize::MAX).chars().count();
    // The largest size each output may keep, its note included, for all of them to fit.
    let kept = |cap: usize| -> usize { sizes.iter().map(|&(_, size)| size.min(cap)).sum() };
    let (mut low, mut high) = (0, sizes.iter().map(|&(_, size)| size).max().unwrap_or(0));
    while low < high {
        let mid = (low + high).div_ceil(2);
        if kept(mid) <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let cap = low.max(room * 2);
    let mut cut = 0;
    for (i, size) in sizes {
        if size <= cap {
            continue;
        }
        let chars: Vec<char> = input[i]["output"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .collect();
        let keep = cap - room;
        let (head, tail) = (keep.div_ceil(2), keep / 2);
        let text: String = (chars[..head].iter())
            .chain(note(size - keep).chars().collect::<Vec<_>>().iter())
            .chain(chars[size - tail..].iter())
            .collect();
        input[i]["output"] = Value::String(text);
        cut += 1;
    }
    cut
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
                Some("function_call_output") => tools::result_text(&item["output"]),
                Some("reasoning") => thinking(item),
                _ => text(item),
            };
            // Scramjet counts an image as 4800 characters.
            let images = tools::item_images(item) as u64 * 4800;
            (content.chars().count() as u64 + images).div_ceil(4)
        })
        .sum()
}

fn assistant(item: &Value) -> bool {
    item["type"] == "reasoning" || item["type"] == "function_call" || item["role"] == "assistant"
}

/// Each item's tokens in the context: chars/4, as Scramjet estimates them, but a reply as many
/// as its context measured, up to its output, as thinking counts in full where only its summary
/// is saved. The measure is how much the next request's input grew past the reply's input and
/// the items between; the last reply's is its output.
fn weights(items: &[Value], start: usize) -> Vec<u64> {
    let mut weights: Vec<u64> = (items.iter())
        .map(|item| estimate(std::slice::from_ref(item)))
        .collect();
    let replies: Vec<(usize, usize, u64, u64)> = (start..items.len())
        .filter_map(|end| {
            let usage = &items[end][USAGE];
            let input = ["input", "cache_read", "cache_write"]
                .iter()
                .map(|field| usage[field].as_u64())
                .sum::<Option<u64>>()?;
            let first = (start..end)
                .rev()
                .take_while(|&i| assistant(&items[i]))
                .last()
                .unwrap_or(end);
            Some((first, end, input, usage["output"].as_u64()?))
        })
        .collect();
    for (n, &(first, end, input, output)) in replies.iter().enumerate() {
        let measured = match replies.get(n + 1) {
            Some(&(next, _, next_input, _)) => {
                let between: u64 = weights[end + 1..next].iter().sum();
                next_input.saturating_sub(input + between)
            }
            None => output,
        };
        let estimated: u64 = weights[first..=end].iter().sum();
        weights[first] += measured.min(output).saturating_sub(estimated);
    }
    weights
}

/// Walk backwards to ~20k tokens, then choose the next safe assistant/user boundary, or the last
/// one when the newest reply with its tool output alone passes the budget (where Scramjet's
/// `findCutPoint` cuts nothing, and a local runtime then drops the prompt's start). Responses splits an assistant's text, reasoning and calls into separate items; outstanding
/// calls prevent a cut anywhere inside a call/output group (including parallel calls).
pub(super) fn cut_point(items: &[Value], start: usize, budget: u64) -> usize {
    let mut pending = HashSet::new();
    let mut cuts = Vec::new();
    for (i, item) in items.iter().enumerate().skip(start) {
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
    let weights = weights(items, start);
    let mut tokens = 0;
    for i in (start..items.len()).rev() {
        tokens += weights[i];
        if tokens >= budget {
            return (cuts.iter().copied().find(|&cut| cut >= i))
                .or(cuts.last().copied())
                .unwrap_or(start);
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
                let output = &tools::result_text(&item["output"]);
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

/// Scramjet's caps on the output of a compaction summary and of a turn-prefix summary: 0.8 and
/// 0.5 × its `reserveTokens`, at least one token.
fn summary_tokens(reserve: u64) -> u64 {
    (reserve.saturating_mul(4) / 5).max(1)
}

fn turn_prefix_tokens(reserve: u64) -> u64 {
    (reserve / 2).max(1)
}

/// A request a local runtime counted whole: its tokens by chars/4 and by the runtime, and
/// whether the conversation still extends it.
pub(super) struct Calibration {
    estimate: u64,
    input: u64,
    extends: bool,
}

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
    /// The last response's context passed the model's window minus the reserve.
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
    /// Replaces the conversation before its last `keep_recent` tokens with a summary the model writes, as
    /// Scramjet's compaction does, followed by the files read and modified in all compacted
    /// history, and records that in the session; `automatic` says why it runs without
    /// `/compact`. On failure or cancellation nothing changes. `None`: nothing is old enough to
    /// summarize.
    pub fn compact(
        &mut self,
        focus: &str,
        automatic: Option<AutoCompaction>,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Option<Compaction>, AgentError> {
        let purpose = Purpose::Compaction(automatic);
        let previous = previous_summary(&self.conversation).map(str::to_owned);
        let start = usize::from(previous.is_some());
        let (reserve, keep_recent) = self.reserve_and_kept();
        let cut = cut_point(&self.conversation, start, keep_recent);
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
                self.summarize(prompt, purpose, summary_tokens(reserve), ui, cancel)?
            }
        };
        if let Some(turn) = turn {
            let prompt = turn_prefix_prompt(&self.conversation[turn..cut]);
            let tokens = turn_prefix_tokens(reserve);
            let prefix = self.summarize(prompt, purpose, tokens, ui, cancel)?;
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
        // A delegated command's earlier result may be summarized away, so it may run again.
        self.delegated.clear();
        // Unknown until the next response reports it.
        self.usage.context_tokens = None;
        self.conversation_replaced();
        ui.usage(
            self.session_tokens(),
            self.context_tokens(),
            self.output_rate(),
        );
        Ok(Some(Compaction {
            tokens_before,
            kept_items: self.conversation.len() - 1,
            summary,
        }))
    }

    /// One request without tools for the text a summarization `prompt` asks for, of at most
    /// `tokens` output tokens (fewer when the model or `--max-output-tokens` allows fewer). As in
    /// Scramjet, a branch summary is asked for without reasoning, and a summary cut off at the
    /// output token limit is kept.
    pub(super) fn summarize(
        &mut self,
        prompt: String,
        purpose: Purpose,
        tokens: u64,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<String, AgentError> {
        let tokens = [self.info.output, self.config.max_output_tokens]
            .into_iter()
            .flatten()
            .fold(tokens, u64::min);
        let mut request = json!({
            "model": self.info.id,
            "instructions": SYSTEM_PROMPT,
            "input": [{"role":"user", "content":prompt}],
            "stream": true,
            "store": false,
            "prompt_cache_key": self.session.id(),
            "max_output_tokens": tokens,
        });
        let effort = match (purpose, self.info.protocol) {
            // A Messages request without effort would think; `none` sends no thinking.
            (Purpose::BranchSummary, Protocol::Messages | Protocol::Completions) => Some("none"),
            (Purpose::BranchSummary, Protocol::Responses | Protocol::Codex) => {
                Some(self.info.fit("none"))
            }
            _ => self.effort(),
        };
        if self.info.reasoning {
            request["reasoning"] = json!({"summary": "auto"});
            if let Some(effort) = effort {
                request["reasoning"]["effort"] = json!(effort);
            }
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
        match self.compact("", Some(reason), ui, cancel) {
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
    /// window minus the reserve. Without a known window, or with automatic compaction off, it
    /// does nothing.
    pub(super) fn compact_when_full(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let (Some(tokens), Some(window)) = (self.usage.context_tokens, self.info.window) else {
            return Ok(());
        };
        if self.full(tokens, window) {
            self.auto_compact(AutoCompaction::Threshold, ui, cancel)?;
        }
        Ok(())
    }

    /// Whether automatic compaction is on and `tokens` of context fill a `window` (`should_compact`).
    fn full(&self, tokens: u64, window: u64) -> bool {
        let (reserve, _) = self.reserve_and_kept();
        self.config.compacting.automatic && should_compact(tokens, window, self.info.input, reserve)
    }

    /// The settings' `reserveTokens` and `keepRecentTokens`, each at most a quarter of the window
    /// on a model a local runtime serves: their defaults suit Scramjet's 200k windows, and on
    /// Ollama's 32k or 40k one compaction would never run, or keep nearly everything.
    fn reserve_and_kept(&self) -> (u64, u64) {
        let Compacting {
            reserve,
            keep_recent,
            ..
        } = self.config.compacting;
        match (self.info.runtime, self.info.window) {
            (Some(_), Some(window)) => (reserve.min(window / 4), keep_recent.min(window / 4)),
            _ => (reserve, keep_recent),
        }
    }

    /// As Scramjet counts the context: the last response's, if the current model gave it, and an
    /// estimate of the items added after it, such as tool output from a cancelled turn.
    fn context_size(&self) -> Option<u64> {
        let tokens = self.usage.context_tokens?;
        let last = self
            .conversation
            .iter()
            .rposition(|item| item.get(PRODUCER).is_some())?;
        (self.conversation[last][PRODUCER].as_str())
            .is_some_and(|producer| self.produced(producer))
            .then(|| tokens + estimate(&self.conversation[last + 1..]))
    }

    /// As Scramjet's check before a new prompt, which catches a turn that ended without a reply.
    pub(super) fn compact_before_prompt(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        match self.context_size() {
            Some(tokens) => self.compact_if_full(tokens, ui, cancel),
            None => Ok(()),
        }
    }

    /// Compacts when `tokens` fill the window; a failed compaction is shown, not returned.
    fn compact_if_full(
        &mut self,
        tokens: u64,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let Some(window) = self.info.window else {
            return Ok(());
        };
        if !self.full(tokens, window) {
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

    /// Before each request of a turn on a model a local runtime serves, the threshold check:
    /// such a runtime cuts a prompt longer than its window instead of refusing it (Ollama 0.40.16
    /// answers 200, drops early messages or cuts the last to half its window, and reports the
    /// tokens it kept), so neither an overflow
    /// error nor the reported usage shows it, and the request is measured by estimate too.
    pub(super) fn compact_for_runtime(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        if self.info.runtime.is_none() {
            return Ok(());
        }
        self.compact_if_full(self.runtime_context(), ui, cancel)
    }

    /// On a model a local runtime serves, makes `request` fit the window less the reserve for
    /// the reply when compaction could not: its largest tool outputs are cut to their start and
    /// end in this request only, the session keeping them whole; when the rest of the request
    /// leaves less room, half of what is left below the window, and when it fills the window,
    /// nothing is cut. When the request still passes the window, or outputs
    /// were cut, says so once per turn (`said`).
    pub(super) fn fit_for_runtime(
        &self,
        request: &mut Value,
        said: &mut bool,
        ui: &mut dyn AgentUi,
    ) {
        let (Some(runtime), Some(window)) = (self.info.runtime, self.info.window) else {
            return;
        };
        let (reserve, _) = self.reserve_and_kept();
        let target = window.saturating_sub(reserve);
        let mut tokens = self.calibrated(request_estimate(request));
        let mut cut = 0;
        if tokens > target
            && let Some(input) = request["input"].as_array_mut()
        {
            let outputs: u64 = (input.iter())
                .filter(|item| item["type"] == "function_call_output")
                .map(|item| estimate(std::slice::from_ref(item)))
                .sum();
            let rest = tokens.saturating_sub(outputs);
            // When the rest leaves less than the reserve, the outputs take half what is left.
            let room = target
                .saturating_sub(rest)
                .max(window.saturating_sub(rest) / 2);
            if room > 0 {
                let note = |chars: usize| {
                    format!(
                        "\n[… {chars} characters cut here: the whole output does not fit {}'s \
                         {window}-token window. Ask for less at a time …]\n",
                        self.info.id
                    )
                };
                // At two characters a token: chars/4 undercounts output such as `seq`'s, whose digits
                // qwen3 takes one token each (live, such a request filled 12,224 of 12,288).
                cut = fit_outputs(input, room * 2, note);
                tokens = self.calibrated(request_estimate(request));
            }
        }
        if *said {
            return;
        }
        if tokens > window {
            *said = true;
            let outcome = match runtime {
                super::local::Runtime::Ollama => {
                    "Ollama drops the conversation's start without saying so"
                }
                _ => "the runtime may refuse the request",
            };
            ui.notice(&format!(
                "The next request, about {tokens} tokens with the system prompt and tools, is \
                 past {}'s {window}-token window, and compaction cannot cut it further: \
                 {outcome}. Load the model with a larger window, or ask for less output",
                self.info.id
            ));
        } else if cut > 0 {
            *said = true;
            let outputs = if cut == 1 {
                "a tool output"
            } else {
                "tool outputs"
            };
            ui.notice(&format!(
                "Cut {outputs} in the middle for the next request to fit {}'s {window}-token \
                 window; the session keeps them whole",
                self.info.id
            ));
        }
    }

    /// The larger of the context as reported and the next request by estimate: its instructions,
    /// tools and conversation at chars/4, `calibrated`.
    fn runtime_context(&self) -> u64 {
        let tools = serde_json::to_string(&self.tool_definitions()).unwrap_or_default();
        let base = (self.instructions().chars().count() + tools.chars().count()) as u64;
        let estimated = self.calibrated(base.div_ceil(4) + estimate(&self.conversation));
        self.context_size().unwrap_or(0).max(estimated)
    }

    /// Learns from a reply of `model` how far chars/4 was from the runtime's count of the
    /// request's `input`, when that count is of the whole request: llama.cpp, vLLM and LM Studio
    /// refuse a prompt past their window, while Ollama 0.40.16 drops early messages until it fits
    /// (counts up to the window) or, when the last alone does not, cuts it to half the window and
    /// two tokens, and reports what it kept. So Ollama's count is taken only for a request that
    /// fit the window by the calibrated estimate, and never at half the window. Whether it was
    /// taken.
    pub(super) fn calibrate(&mut self, model: &str, estimate: u64, input: u64) -> bool {
        let (Some(runtime), Some(window)) = (self.info.runtime, self.info.window) else {
            return false;
        };
        let whole = match runtime {
            super::local::Runtime::LlamaCpp
            | super::local::Runtime::Vllm
            | super::local::Runtime::LmStudio => true,
            super::local::Runtime::Ollama => {
                self.calibrated(estimate) < window && input.abs_diff(window / 2 + 2) > 64
            }
            super::local::Runtime::Other => false,
        };
        if whole && model == self.config.model && estimate > 0 {
            let calibration = Calibration {
                estimate,
                input,
                extends: true,
            };
            self.calibrations.insert(model.to_owned(), calibration);
            return true;
        }
        false
    }

    /// After compaction, a move in the tree, a session switch or a start, the next request no
    /// longer extends the measured ones; a model measured only in an earlier process takes its
    /// latest count saved in the conversation (`CALIBRATION`).
    pub(super) fn conversation_replaced(&mut self) {
        for calibration in self.calibrations.values_mut() {
            calibration.extends = false;
        }
        for item in self.conversation.iter().rev() {
            let (Some(model), Some(estimate), Some(input)) = (
                item[PRODUCER].as_str(),
                item[CALIBRATION]["estimate"].as_u64(),
                item[CALIBRATION]["input"].as_u64(),
            ) else {
                continue;
            };
            if estimate > 0 && !self.calibrations.contains_key(model) {
                let calibration = Calibration {
                    estimate,
                    input,
                    extends: false,
                };
                self.calibrations.insert(model.to_owned(), calibration);
            }
        }
    }

    /// A chars/4 estimate of a request to the current model, corrected by its last measured
    /// request (`calibrate`): while the conversation extends that request, less what chars/4
    /// overcounted in it, the items added since counting at chars/4; otherwise scaled by its
    /// count over its estimate.
    fn calibrated(&self, estimate: u64) -> u64 {
        match self.calibrations.get(&self.config.model) {
            Some(last) if last.extends => (estimate + last.input).saturating_sub(last.estimate),
            Some(last) => {
                (u128::from(estimate) * u128::from(last.input) / u128::from(last.estimate)) as u64
            }
            None => estimate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{AgentConfig, Compacting, Session, SessionChoice, Tokens};
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
        // An image's text is serialized; the image counts as Scramjet counts one.
        let image = json!({"type":"function_call_output", "output":[
            {"type":"input_text","text":"Read image file [image/png]"},
            {"type":"input_image","image_url":format!("data:image/png;base64,{}", "A".repeat(100_000))},
        ]});
        assert_eq!(
            serialize(std::slice::from_ref(&image)),
            "[Tool result]: Read image file [image/png]"
        );
        assert_eq!(estimate(&[image]), (27 + 4800_u64).div_ceil(4));
        let attached = json!({"role":"user","content":[
            {"type":"input_text","text":"<file name=\"/p/a.png\"></file>\n"},
            {"type":"input_image","image_url":format!("data:image/png;base64,{}", "A".repeat(100_000))},
        ]});
        assert_eq!(
            serialize(std::slice::from_ref(&attached)),
            "[User]: <file name=\"/p/a.png\"></file>\n"
        );
        assert_eq!(estimate(&[attached]), (30 + 4800_u64).div_ceil(4));
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
        // The reply with its output alone passes the budget: the cut comes just before it.
        assert_eq!(cut_point(&reply, 0, 10), 1);
        assert_eq!(cut_point(&reply, 0, estimate(&reply[2..])), 1);
        let users = vec![
            json!({"role":"user","content":"old"}),
            json!({"role":"user","content":"x".repeat(80_000)}),
        ];
        assert_eq!(cut_point(&users, 0, 20_000), 1);
    }

    #[test]
    fn replies_count_as_many_tokens_as_the_context_grew_by_them_up_to_their_output() {
        let usage = |input: u64, output: u64| json!({"input": 4, "cache_read": input - 4, "cache_write": 0, "output": output});
        let call = |id: &str, usage: Value| {
            json!({"type":"function_call","call_id":id,"name":"read","arguments":"{}",
                "_scramjet_usage":usage})
        };
        let thinking = json!({"type":"reasoning","summary":[{"text":"a short summary"}]});
        let output = |id: &str| json!({"type":"function_call_output","call_id":id,"output":"ok"});
        let items = |second: u64, third: u64| {
            vec![
                json!({"role":"user","content":"start"}),
                thinking.clone(),
                call("a", usage(20_000, 30_000)),
                output("a"),
                thinking.clone(),
                call("b", usage(second, 30_000)),
                output("b"),
                json!({"type":"message","role":"assistant","content":[{"text":"done"}],
                    "_scramjet_usage":usage(third, 10)}),
            ]
        };
        // Claude's thinking stays in the context in full: each reply took 30k of it, and the
        // budget is reached within the later one.
        let claude = items(50_001, 80_002);
        assert_eq!(cut_point(&claude, 0, 20_000), 4);
        assert_eq!(cut_point(&claude, 0, 40_000), 1);
        // Thinking the provider drops from the context, or input that shrank, as when kept
        // items' usage predates a compaction, counts as estimated.
        let dropped = items(20_100, 20_200);
        assert_eq!(cut_point(&dropped, 0, 20_000), 0);
        let shrunk = items(50_001, 5_000);
        assert_eq!(cut_point(&shrunk, 0, 20_000), 1);
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
        const R: u64 = 16_384;
        assert!(!should_compact(20_000, 36_384, None, R));
        assert!(should_compact(20_001, 36_384, None, R));
        assert!(should_compact(u64::MAX, u64::MAX, None, R));
        assert!(!should_compact(1_000_000, R, None, R));
        assert!(!should_compact(0, 0, None, R));
        // A model's input limit below its window is a threshold of its own.
        assert!(!should_compact(272_000, 400_000, Some(272_000), R));
        assert!(should_compact(272_001, 400_000, Some(272_000), R));
        assert!(!should_compact(272_001, R, Some(272_000), R));
        // The settings' reserve moves the threshold.
        assert!(!should_compact(30_000, 36_384, None, 6_384));
        assert!(should_compact(30_001, 36_384, None, 6_384));
        assert!(should_compact(2, 1, None, 0));
        assert_eq!(
            (summary_tokens(16_384), turn_prefix_tokens(16_384)),
            (13_107, 8192)
        );
        assert_eq!((summary_tokens(1), turn_prefix_tokens(1)), (1, 1));
        assert_eq!(summary_tokens(u64::MAX), u64::MAX / 5);
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
        let (mut agent, scratch) = agent(endpoint, "compact");
        let rates = scratch.0.join("output-rates.json");
        agent.rates = Some(rates.clone());
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
            .compact("file paths", None, &mut ui, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!((first.kept_items, first.tokens_before), (1, estimate));
        assert_eq!(
            first.summary,
            "first summary\n\n<read-files>\na.rs\n</read-files>\n\n<modified-files>\nb.rs\n</modified-files>"
        );
        assert_eq!(agent.usage.context_tokens, None);
        // The footer is told the context is not known now.
        assert_eq!(ui.usage.last(), Some(&(Tokens::default(), None)));
        // A summary's rate is not a reply's, as in Scramjet.
        assert!(!rates.exists());
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
        let second = agent.compact("", None, &mut ui, &cancel).unwrap().unwrap();
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
            .compact("focus", None, &mut ui, &AtomicBool::new(false))
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
        assert_eq!(requests[0]["max_output_tokens"], 13_107);
        assert_eq!(requests[1]["max_output_tokens"], 8192);
    }

    #[test]
    fn summaries_are_capped_as_scramjets_and_a_branch_summary_does_not_reason() {
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let claude = messages_stream(&[
            json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"s"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            json!({"type":"message_stop"}),
        ]);
        let (endpoint, server) = serve(vec![text("a"), text("b"), text("c"), claude]);
        let (mut agent, _scratch) = agent(endpoint, "summary-caps");
        agent.set_effort("high").unwrap();
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let mut summarize = |agent: &mut Agent, purpose, tokens| {
            agent
                .summarize("p".into(), purpose, tokens, &mut ui, &cancel)
                .unwrap()
        };
        summarize(
            &mut agent,
            Purpose::Compaction(None),
            summary_tokens(16_384),
        );
        agent.info.output = Some(4000);
        summarize(&mut agent, Purpose::BranchSummary, 2048);
        agent.config.max_output_tokens = Some(1000);
        summarize(
            &mut agent,
            Purpose::Compaction(None),
            turn_prefix_tokens(16_384),
        );
        agent.config.max_output_tokens = None;
        agent.set_model("claude-opus-5-5").unwrap();
        summarize(&mut agent, Purpose::BranchSummary, 2048);
        let bodies = server.join().unwrap();
        assert_eq!(bodies[0]["max_output_tokens"], 13_107);
        assert_eq!(
            bodies[0]["reasoning"],
            json!({"summary":"auto","effort":"high"})
        );
        assert_eq!(bodies[1]["max_output_tokens"], 2048);
        assert_eq!(
            bodies[1]["reasoning"],
            json!({"summary":"auto","effort":"none"})
        );
        assert_eq!(bodies[2]["max_output_tokens"], 1000);
        assert_eq!(bodies[3]["max_tokens"], 2048);
        assert!(bodies[3].get("thinking").is_none(), "{}", bodies[3]);
        assert!(bodies[3].get("output_config").is_none());
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
            .compact("", None, &mut Recorder::default(), &AtomicBool::new(false))
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
            assert!(
                agent
                    .compact("", None, &mut ui, &AtomicBool::new(false))
                    .is_err()
            );
            assert_eq!(agent.conversation(), before);
            assert_eq!(std::fs::read(agent.session_path()).unwrap(), log);
        }
        assert!(matches!(
            agent.compact("", None, &mut ui, &AtomicBool::new(true)),
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
        assert!(agent.compact("", None, &mut ui, &cancel).unwrap().is_none());
        agent.add(json!({"role":"user","content":"hi"})).unwrap();
        let log = std::fs::read(agent.session_path()).unwrap();
        assert!(agent.compact("", None, &mut ui, &cancel).unwrap().is_none());
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
            providers: crate::agent::tests::serving(endpoint),
            model: "m".into(),
            root: scratch.0.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: Default::default(),
            compacting: Default::default(),
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
            only_tools: None,
            command_sets: Vec::new(),
            skills: Vec::new(),
            skill_options: Default::default(),
            prompt: Default::default(),
            mcp_servers: Vec::new(),
            mcp_disabled: Vec::new(),
            sandbox: None,
            session_dir: None,
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
    fn with_automatic_compaction_off_only_compact_compacts() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("done", full_usage(10_000))),
            stream(&overflow_events()),
            stream(&fixture::text_events("summary", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-off");
        seed_long_conversation(&mut agent);
        agent.set_context_window(Some(20_000));
        agent.set_compacting(Compacting {
            automatic: false,
            ..Default::default()
        });
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("continue", &mut ui, &cancel).unwrap();
        let error = agent.turn("more", &mut ui, &cancel).unwrap_err();
        assert_eq!(
            error.to_string(),
            "context overflow: provider error: Your input exceeds the context window of this \
model. (automatic compaction is off: /compact, or turn it on in /settings)"
        );
        assert!(ui.compactions.is_empty());
        assert!(agent.compact("", None, &mut ui, &cancel).unwrap().is_some());
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2]["instructions"], SYSTEM_PROMPT);
    }

    #[test]
    fn the_settings_reserve_and_kept_tokens_say_when_and_how_far() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "done",
            full_usage(10_000),
        ))]);
        let (mut roomy, _scratch) = agent(endpoint, "auto-reserve-small");
        seed_long_conversation(&mut roomy);
        roomy.set_context_window(Some(20_000));
        roomy.set_compacting(Compacting {
            reserve: 5_000,
            ..Default::default()
        });
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        roomy.turn("continue", &mut ui, &cancel).unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());

        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("done", full_usage(10_000))),
            stream(&fixture::text_events("summary", fixture::known_usage())),
            stream(&fixture::text_events("prefix", fixture::known_usage())),
        ]);
        let (mut tight, _scratch) = agent(endpoint, "auto-reserve-large");
        seed_long_conversation(&mut tight);
        tight.set_context_window(Some(20_000));
        tight.set_compacting(Compacting {
            reserve: 12_000,
            keep_recent: 1,
            ..Default::default()
        });
        tight.turn("continue", &mut ui, &cancel).unwrap();
        assert_eq!(
            ui.compactions,
            [(10_000, 1, Some(AutoCompaction::Threshold))]
        );
        let requests = server.join().unwrap();
        assert_eq!(requests[1]["max_output_tokens"], 9_600);
        // Keeping one token cuts the last turn: its start is summarized apart.
        assert_eq!(requests[2]["max_output_tokens"], 6_000);
        assert!(
            requests[1]["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains("recent work recent work")
        );
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
        assert_eq!(
            ui.waits,
            [
                Purpose::Reply,
                Purpose::Compaction(Some(AutoCompaction::Threshold))
            ]
        );
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
        // The reply, then the error, which a resume shows.
        assert_eq!(agent.conversation().len(), 5);
        assert_eq!(
            agent.conversation()[4],
            json!({"type":"turn_stopped","error":error.to_string()})
        );
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
                Purpose::Compaction(Some(AutoCompaction::Threshold)),
                Purpose::Reply,
                Purpose::Compaction(Some(AutoCompaction::Threshold)),
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
    fn the_threshold_check_the_runtime_window_comes_before_each_request_of_a_turn() {
        let call = || {
            let events = fixture::call_events(r#"{"command":"true"}"#, full_usage(23_000));
            let text = serde_json::to_string(&events).unwrap();
            stream(
                &serde_json::from_str::<Vec<Value>>(&text.replace("\"inert\"", "\"bash\""))
                    .unwrap(),
            )
        };
        let run = |runtime: Option<crate::agent::local::Runtime>, name: &str| {
            let mut responses = Vec::new();
            if runtime.is_some() {
                responses.extend(runtime_says("m", 30_000));
            }
            responses.push(call());
            if runtime.is_some() {
                responses.push(stream(&fixture::text_events(
                    "summary",
                    fixture::known_usage(),
                )));
            }
            responses.push(stream(&fixture::text_events(
                "done",
                fixture::known_usage(),
            )));
            let (endpoint, server) = serve(responses);
            let (mut agent, _scratch) = agent(endpoint, name);
            agent.set_context_window(Some(30_000));
            agent.info.runtime = runtime;
            let model = agent.config.model.clone();
            (agent.runtime_windows).insert(model, super::super::turn::RuntimeWindow::Read(30_000));
            // Below the threshold with the system prompt and tools, which a runtime's check counts.
            for content in ["remember the goal".to_owned(), "recent work ".repeat(3000)] {
                agent
                    .add(json!({"role":"user", "content":content}))
                    .unwrap();
            }
            let mut ui = Recorder::default();
            agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
            let requests = server.join().unwrap();
            (
                ui,
                requests
                    .into_iter()
                    .filter(|body| !body.is_null())
                    .collect::<Vec<_>>(),
            )
        };
        // Ollama answers a prompt past its window, cut: compaction comes before the request.
        let (ui, requests) = run(
            Some(crate::agent::local::Runtime::Ollama),
            "runtime-threshold",
        );
        assert_eq!(
            ui.waits,
            [
                Purpose::Reply,
                Purpose::Compaction(Some(AutoCompaction::Threshold)),
                Purpose::Reply
            ]
        );
        assert_eq!(requests[2]["input"][0], summary_message("summary"));
        assert_eq!(
            requests[2]["input"].as_array().unwrap().last().unwrap()["type"],
            "function_call_output"
        );
        // A provider refuses one with an error, so the request goes as Scramjet sends it.
        let (ui, requests) = run(None, "provider-threshold");
        assert_eq!(ui.waits, [Purpose::Reply, Purpose::Reply]);
        assert_eq!(requests[1]["input"][0]["content"], "remember the goal");
    }

    #[test]
    fn a_runtime_model_counts_by_its_runtime_once_the_count_is_of_the_whole_request() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        // A 30,000-token window, so a threshold of 22,500; the first request is about 22,000
        // tokens at chars/4, and a 16,000-character output adds 4,000 more.
        let run = |runtime: crate::agent::local::Runtime, input: u64, name: &str| {
            let arguments = json!({"command": "head -c 16000 /dev/zero | tr '\\0' x"});
            let usage = json!({"input_tokens": input, "output_tokens": 10,
                "total_tokens": input + 10});
            let events = fixture::call_events(&arguments.to_string(), usage);
            let text = serde_json::to_string(&events).unwrap();
            let call = serde_json::from_str::<Vec<Value>>(&text.replace("\"inert\"", "\"bash\""));
            let [models, version] = runtime_says("m", 30_000);
            let (endpoint, server) = serve(vec![
                models,
                version,
                stream(&call.unwrap()),
                stream(&fixture::text_events("summary", fixture::known_usage())),
                stream(&fixture::text_events("done", fixture::known_usage())),
            ]);
            let (mut agent, _scratch) = agent(endpoint, name);
            agent.set_context_window(Some(30_000));
            agent.info.runtime = Some(runtime);
            let model = agent.config.model.clone();
            (agent.runtime_windows).insert(model, super::super::turn::RuntimeWindow::Read(30_000));
            let base = request_estimate(&agent.request());
            assert!(base < 20_000);
            let filler = "word ".repeat(((21_900 - base) * 4 / 50) as usize);
            for _ in 0..10 {
                agent.add(json!({"role":"user", "content":filler})).unwrap();
            }
            let mut ui = Recorder::default();
            agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
            drop(server);
            ui.waits
        };
        let compacted = [
            Purpose::Reply,
            Purpose::Compaction(Some(AutoCompaction::Threshold)),
            Purpose::Reply,
        ];
        // Ollama counted 17,000 of the 22,000 estimated, more than half its window, so the
        // request was whole: about 21,000 tokens with the output, under the threshold.
        let waits = run(crate::agent::local::Runtime::Ollama, 17_000, "calibrated");
        assert_eq!(waits, [Purpose::Reply, Purpose::Reply]);
        // A count of half its window may be of a prompt it cut, so chars/4 stands.
        let waits = run(crate::agent::local::Runtime::Ollama, 15_002, "cut-count");
        assert_eq!(waits, compacted);
        // A count clear below half, of a request under the window by chars/4, was not cut.
        let waits = run(crate::agent::local::Runtime::Ollama, 14_000, "under-half");
        assert_eq!(waits, [Purpose::Reply, Purpose::Reply]);
        // llama.cpp refuses a prompt past its window, so its count is always whole.
        let waits = run(crate::agent::local::Runtime::LlamaCpp, 12_000, "llama-cpp");
        assert_eq!(waits, [Purpose::Reply, Purpose::Reply]);
        let waits = run(crate::agent::local::Runtime::Other, 12_000, "other");
        assert_eq!(waits, compacted);
    }

    #[test]
    fn an_ollama_count_is_taken_only_for_a_request_it_cannot_have_cut() {
        let endpoint: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
        let (mut agent, _scratch) = agent(endpoint, "ollama-cut");
        agent.set_context_window(Some(30_000));
        agent.info.runtime = Some(crate::agent::local::Runtime::Ollama);
        let model = agent.config.model.clone();
        // Past the window by chars/4, so the prompt may have been cut: to half the window when
        // its last message alone does not fit, else by dropping early messages, which leaves
        // counts above half (3,423 to 3,719 of a 4,096 window, live).
        assert!(!agent.calibrate(&model, 31_000, 10_000));
        assert!(!agent.calibrate(&model, 40_000, 27_000));
        assert_eq!(agent.calibrated(20_000), 20_000);
        assert!(agent.calibrate(&model, 12_000, 10_000));
        assert_eq!(agent.calibrated(20_000), 18_000);
        assert!(agent.calibrate(&model, 29_000, 26_000));
        assert_eq!(agent.calibrated(30_000), 27_000);
        // A count of half the window and two may be of a cut prompt whatever the estimate.
        assert!(!agent.calibrate(&model, 16_000, 15_002));
        // Past the window once calibrated, though not by chars/4 (digits count a token each).
        assert!(agent.calibrate(&model, 10_000, 12_000));
        assert!(!agent.calibrate(&model, 29_000, 29_500));
    }

    #[test]
    fn a_runtime_count_saved_in_the_session_corrects_a_later_process() {
        let usage = json!({"input_tokens": 750, "output_tokens": 10, "total_tokens": 760});
        let [models, version] = runtime_says("m", 30_000);
        let (endpoint, server) = serve(vec![
            models,
            version,
            stream(&fixture::text_events("answer", usage)),
        ]);
        let (mut first, scratch) = agent(endpoint, "calibration-saved");
        first.set_context_window(Some(30_000));
        first.info.runtime = Some(crate::agent::local::Runtime::LlamaCpp);
        let model = first.config.model.clone();
        (first.runtime_windows).insert(
            model.clone(),
            super::super::turn::RuntimeWindow::Read(30_000),
        );
        first
            .turn("go", &mut Recorder::default(), &AtomicBool::new(false))
            .unwrap();
        drop(server);
        let saved = &first.conversation.last().unwrap()[CALIBRATION];
        assert_eq!(saved["input"], 750);
        let estimate = saved["estimate"].as_u64().unwrap();
        assert!(estimate > 0);
        let path = first.session_path().to_owned();
        drop(first);

        let config = crate::agent::tests::config(endpoint, &scratch.0);
        let resumed = Agent::start(config, SessionChoice::Open(path)).unwrap();
        assert_eq!(resumed.calibrated(estimate * 2), 1_500);
        // A provider's count is not taken, so nothing is saved.
        let (mut agent, _scratch) = agent(endpoint, "calibration-not-saved");
        assert!(!agent.calibrate(&model, 1_000, 750));
    }

    #[test]
    fn a_runtime_count_scales_estimates_after_the_conversation_is_replaced() {
        let endpoint: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
        let (mut agent, _scratch) = agent(endpoint, "calibration-kept");
        agent.set_context_window(Some(30_000));
        agent.info.runtime = Some(crate::agent::local::Runtime::LlamaCpp);
        let model = agent.config.model.clone();
        agent.calibrate(&model, 20_000, 15_000);
        // Items added since count at chars/4.
        assert_eq!(agent.calibrated(24_000), 19_000);
        // A compacted, resumed or new conversation keeps the runtime's measure as a ratio.
        agent.conversation_replaced();
        assert_eq!(agent.calibrated(8_000), 6_000);
        agent.calibrate(&model, 10_000, 9_000);
        assert_eq!(agent.calibrated(12_000), 11_000);
    }

    /// A turn on an Ollama model with a `window`: a 4,000-character output, then a 60,000 one,
    /// then a reply. Returns the agent, what it showed and the requests.
    fn large_output_turn(window: u64, name: &str) -> (Agent, ScratchDir, Recorder, Vec<Value>) {
        let call = |chars: u64, total: u64| {
            let arguments = json!({"command": format!("head -c {chars} /dev/zero | tr '\\0' x")});
            let events = fixture::call_events(&arguments.to_string(), full_usage(total));
            let text = serde_json::to_string(&events).unwrap();
            stream(
                &serde_json::from_str::<Vec<Value>>(&text.replace("\"inert\"", "\"bash\""))
                    .unwrap(),
            )
        };
        let [models, version] = runtime_says("m", window);
        let (endpoint, server) = serve(vec![
            models,
            version,
            call(4_000, 9_800),
            call(60_000, 10_800),
            stream(&fixture::text_events("prefix", fixture::known_usage())),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, scratch) = agent(endpoint, name);
        agent.set_context_window(Some(window));
        agent.info.runtime = Some(crate::agent::local::Runtime::Ollama);
        let model = agent.config.model.clone();
        (agent.runtime_windows).insert(model, super::super::turn::RuntimeWindow::Read(window));
        let mut ui = Recorder::default();
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        let requests = server.join().unwrap();
        let requests = requests
            .into_iter()
            .filter(|body| !body.is_null())
            .collect();
        (agent, scratch, ui, requests)
    }

    #[test]
    fn a_runtime_model_compacts_before_an_output_too_large_to_keep_and_cuts_it_to_fit() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let (agent, _scratch, ui, requests) = large_output_turn(16_000, "runtime-output-cut");
        // The first output fits; the second, cut at the tool's 50 KB, alone passes
        // `keepRecentTokens` (4,000 here) and the window: the turn's start and first call are
        // summarized, the second call is kept, and its output is cut to half the room the rest
        // of the request leaves below the window (about 9,800 tokens of 16,000) at two
        // characters a token, the session keeping it whole.
        assert_eq!(
            ui.waits,
            [
                Purpose::Reply,
                Purpose::Reply,
                Purpose::Compaction(Some(AutoCompaction::Threshold)),
                Purpose::Reply
            ]
        );
        let input = requests[3]["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert!(text(&input[0]).contains("prefix"));
        assert_eq!(input[1]["type"], "function_call");
        let sent = tools::result_text(&input[2]["output"]);
        assert!(sent.len() < 8_000 && sent.len() > 4_000, "{}", sent.len());
        assert!(sent.starts_with("[Output truncated") && sent.trim_end().ends_with("xxxx"));
        assert!(sent.contains("characters cut here: the whole output does not fit m's 16000"));
        assert!(request_estimate(&requests[3]) <= 12_000);
        let kept = (agent.conversation.iter())
            .rfind(|item| item["type"] == "function_call_output")
            .unwrap();
        assert!(tools::result_text(&kept["output"]).len() > 40_000);
        let notices: Vec<&String> = (ui.shown.iter())
            .filter(|s| s.starts_with("notice"))
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].contains("Cut a tool output in the middle")
                && notices[0].contains("m's 16000-token window"),
            "{notices:?}"
        );
    }

    #[test]
    fn a_runtime_model_whose_instructions_and_tools_alone_pass_the_window_cuts_nothing_and_says_so()
    {
        if crate::agent::tests::missing("bash") {
            return;
        }
        // The test agent's instructions and tools are about 9,800 tokens, past an 8,000-token
        // window: cutting outputs cannot make a request fit.
        let (_agent, _scratch, ui, requests) = large_output_turn(8_000, "runtime-output-kept");
        let outputs: Vec<String> = (requests.iter())
            .flat_map(|request| request["input"].as_array().unwrap())
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| tools::result_text(&item["output"]))
            .collect();
        assert!(outputs.iter().any(|output| output.len() > 40_000));
        assert!(
            !outputs
                .iter()
                .any(|output| output.contains("characters cut here"))
        );
        let notices: Vec<&String> = (ui.shown.iter())
            .filter(|s| s.starts_with("notice"))
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].contains("past m's 8000-token window")
                && notices[0].contains("Ollama drops the conversation's start"),
            "{notices:?}"
        );
    }

    #[test]
    fn outputs_are_cut_largest_first_to_fit_their_budget() {
        let output =
            |text: String| json!({"type":"function_call_output","call_id":"a","output":text});
        let mut input = vec![
            json!({"role":"user","content":"x".repeat(5_000)}),
            output("a".repeat(500)),
            output(format!("{}{}", "b".repeat(5_000), "c".repeat(5_000))),
            output("d".repeat(3_000)),
            json!({"type":"function_call_output","call_id":"i","output":[{"type":"input_image"}]}),
        ];
        let note = |chars: usize| format!("[{chars} cut]");
        assert_eq!(fit_outputs(&mut input, 20_000, note), 0);
        assert_eq!(fit_outputs(&mut input, 4_500, note), 2);
        let sizes: Vec<usize> = (input[1..4].iter())
            .map(|item| item["output"].as_str().unwrap().chars().count())
            .collect();
        assert_eq!(sizes[0], 500);
        assert!(sizes.iter().sum::<usize>() <= 4_500, "{sizes:?}");
        assert!(sizes[1] >= 1_900 && sizes[1] == sizes[2], "{sizes:?}");
        let cut = input[2]["output"].as_str().unwrap();
        assert!(cut.starts_with("bbb") && cut.ends_with("ccc"));
        assert!(cut.contains(" cut]"));
        assert_eq!(input[0]["content"].as_str().unwrap().len(), 5_000);
    }

    #[test]
    fn a_runtime_model_keeps_at_most_a_quarter_of_its_window_in_reserve_and_whole() {
        let (mut agent, _scratch) = agent(([127, 0, 0, 1], 9).into(), "runtime-quarter");
        agent.set_context_window(Some(40_960));
        assert_eq!(agent.reserve_and_kept(), (16_384, 20_000));
        assert!(agent.full(24_577, 40_960));
        agent.info.runtime = Some(crate::agent::local::Runtime::Ollama);
        assert_eq!(agent.reserve_and_kept(), (10_240, 10_240));
        assert!(!agent.full(30_720, 40_960));
        assert!(agent.full(30_721, 40_960));
        // A smaller setting stays, and so do both on a window four times theirs or more.
        agent.set_compacting(Compacting {
            reserve: 4_000,
            ..Default::default()
        });
        assert_eq!(agent.reserve_and_kept(), (4_000, 10_240));
        agent.set_compacting(Compacting::default());
        agent.set_context_window(Some(131_072));
        assert_eq!(agent.reserve_and_kept(), (16_384, 20_000));
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
