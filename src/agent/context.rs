//! What the context holds, part by part, as `/context` shows it: each part's size estimated from
//! its text, as compaction estimates, and scaled to the context the last response reported.
use super::compaction::estimate;
use super::{Agent, Part, Section, mcp, shortened};
use serde_json::Value;

/// The context's parts, by category, and the window they fill.
#[derive(Debug, PartialEq)]
pub struct Breakdown {
    pub model: String,
    pub window: Option<u64>,
    /// The context's tokens: as the last response reported them, with an estimate of what came
    /// after it, or else the estimate alone.
    pub used: u64,
    /// Whether `used` rests on a reported count.
    pub reported: bool,
    /// What automatic compaction keeps free of the window, when it is on and the window known.
    pub reserve: Option<u64>,
    /// In the order shown, none empty.
    pub categories: Vec<Category>,
}

#[derive(Debug, PartialEq)]
pub struct Category {
    pub name: &'static str,
    pub tokens: u64,
    /// The items under it, each with its tokens, in the order they are sent.
    pub items: Vec<(String, u64)>,
}

pub const SYSTEM_PROMPT: &str = "System prompt";
const IDENTITY: &str = "Identity and environment";
const TOOLS: &str = "Tools";
const MCP_TOOLS: &str = "MCP tools";
const CONTEXT_FILES: &str = "Context files";
const SKILLS: &str = "Skills";
const AGENTS: &str = "Subagents";
const COMMANDS: &str = "Commands";
const MESSAGES: &str = "Messages";
const ORDER: [&str; 9] = [
    SYSTEM_PROMPT,
    IDENTITY,
    TOOLS,
    MCP_TOOLS,
    CONTEXT_FILES,
    SKILLS,
    AGENTS,
    COMMANDS,
    MESSAGES,
];

fn tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// The system prompt as `/system-prompt` shows it: the instructions in their parts, then the
/// tool definitions.
#[derive(Debug, PartialEq)]
pub struct SystemPrompt {
    pub parts: Vec<Part>,
    pub tools: Value,
    /// Whether the session's last request sent them; else the next request sends them.
    pub sent: bool,
}

impl Agent {
    /// What the session's last request sent, or before one what the next sends.
    pub fn system_prompt(&self) -> SystemPrompt {
        match &self.sent {
            Some((parts, tools)) => SystemPrompt {
                parts: parts.clone(),
                tools: tools.clone(),
                sent: true,
            },
            None => SystemPrompt {
                parts: self.instruction_parts(),
                tools: self.tool_definitions(),
                sent: false,
            },
        }
    }

    /// What the next request's context holds, by part.
    pub fn context_breakdown(&self) -> Breakdown {
        let mut items: Vec<(&'static str, String, u64)> = Vec::new();
        for part in self.instruction_parts() {
            let category = match part.section {
                Section::Opening | Section::Directives | Section::Sandbox => SYSTEM_PROMPT,
                Section::Identity | Section::Environment => IDENTITY,
                Section::Mcp => MCP_TOOLS,
                Section::ContextFile => CONTEXT_FILES,
                Section::Skills => SKILLS,
                Section::Agents => AGENTS,
                Section::Commands => COMMANDS,
            };
            let listed = match part.section {
                Section::Skills => skills(&part.text),
                Section::Agents | Section::Commands => by_set(&part.text),
                Section::ContextFile => vec![(shortened(&part.name), tokens(&part.text))],
                Section::Mcp => vec![("Servers listed".to_owned(), tokens(&part.text))],
                _ => vec![(part.name, tokens(&part.text))],
            };
            items.extend(listed.into_iter().map(|(name, n)| (category, name, n)));
        }
        if let Value::Array(definitions) = self.tool_definitions() {
            for definition in &definitions {
                let name = definition["name"].as_str().unwrap_or("");
                let size = tokens(&serde_json::to_string(definition).unwrap_or_default());
                match mcp::direct_name(name) {
                    Some((server, _)) => items.push((MCP_TOOLS, server.to_owned(), size)),
                    None => items.push((TOOLS, name.to_owned(), size)),
                }
            }
        }
        for item in &self.conversation {
            let kind = match (item["type"].as_str(), item["role"].as_str()) {
                (Some("function_call"), _) => "Tool calls",
                (Some("function_call_output"), _) => "Tool results",
                (Some("reasoning"), _) => "Thinking",
                (_, Some("assistant")) => "Replies",
                _ => "Prompts and summaries",
            };
            items.push((
                MESSAGES,
                kind.to_owned(),
                estimate(std::slice::from_ref(item)),
            ));
        }
        let estimated: u64 = items.iter().map(|(.., n)| n).sum();
        let reported = self.context_size();
        let used = reported.unwrap_or(estimated);
        let scale = |n: u64| match estimated {
            0 => n,
            _ => (n as u128 * used as u128 / estimated as u128) as u64,
        };
        let categories = (ORDER.into_iter())
            .filter_map(|name| {
                let mut listed: Vec<(String, u64)> = Vec::new();
                for (_, item, n) in items.iter().filter(|(category, ..)| *category == name) {
                    match listed.iter_mut().find(|(seen, _)| seen == item) {
                        Some((_, total)) => *total += scale(*n),
                        None => listed.push((item.clone(), scale(*n))),
                    }
                }
                let tokens = listed.iter().map(|(_, n)| n).sum();
                (!listed.is_empty()).then_some(Category {
                    name,
                    tokens,
                    items: listed,
                })
            })
            .collect();
        let window = self.info.window;
        Breakdown {
            model: self.info.id.clone(),
            window,
            used,
            reported: reported.is_some(),
            reserve: (self.config.compacting.automatic && window.is_some())
                .then(|| self.reserve_and_kept().0),
            categories,
        }
    }
}

/// The skills' listing, by skill, its preamble apart.
fn skills(text: &str) -> Vec<(String, u64)> {
    let mut listed = Vec::new();
    let mut rest = text.to_owned();
    while let Some(start) = rest.find("  <skill>\n")
        && let Some(length) = rest[start..].find("</skill>\n")
    {
        let end = start + length + "</skill>\n".len();
        let entry: String = rest.drain(start..end).collect();
        let name = (entry.split_once("<name>"))
            .and_then(|(_, after)| after.split_once("</name>"))
            .map_or("skill", |(name, _)| name);
        listed.push((name.to_owned(), tokens(&entry)));
    }
    listed.insert(0, ("Preamble".to_owned(), tokens(&rest)));
    listed
}

/// A listing of commands or agents, by the set each comes from, its heading apart.
fn by_set(text: &str) -> Vec<(String, u64)> {
    let mut listed: Vec<(String, u64)> = vec![("Heading".to_owned(), 0)];
    for line in text.split_inclusive('\n') {
        let name = (line.strip_prefix("- "))
            .map(|entry| entry.trim_start_matches('/'))
            .map(|entry| match entry.split_once(':') {
                Some((set, after)) if !set.contains(' ') && !after.starts_with(' ') => set,
                _ => "Others",
            });
        let name = name.unwrap_or("Heading");
        match listed.iter_mut().find(|(seen, _)| seen == name) {
            Some((_, total)) => *total += tokens(line),
            None => listed.push((name.to_owned(), tokens(line))),
        }
    }
    listed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_is_counted_by_set_and_a_skills_listing_by_skill() {
        let commands = "# Available commands\n\nThe following:\n\n- /rs:loop [focus]: Build\n\
- /rs:steer: Steer\n- /mach12:pr-create: Create\n- /own: Mine";
        let listed = by_set(commands);
        let names: Vec<&str> = listed.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Heading", "rs", "mach12", "Others"]);
        let total: u64 = listed.iter().map(|(_, n)| n).sum();
        assert!(total.abs_diff(tokens(commands)) <= 6);
        let agents = by_set("- scramjet:command-architect: Use when");
        assert_eq!(agents[1].0, "scramjet");

        let text = "The following skills.\n\n<available_skills>\n  <skill>\n    <name>one</name>\n  \
</skill>\n  <skill>\n    <name>two</name>\n  </skill>\n</available_skills>\n";
        let names: Vec<String> = skills(text).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["Preamble", "one", "two"]);
    }
}
