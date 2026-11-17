//! `/system-prompt`: the instructions a request sends, part by part, then its tool definitions.
use super::report::{ReportView, Tab};
use super::style;
use super::terminal::tokens;
use super::text::visible;
use scramjet::agent::context::SystemPrompt;
use serde_json::Value;

/// At 4 characters a token, as `/context` estimates.
fn estimate(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

fn heading(name: &str, text: &str) -> String {
    let heading = format!("── {name} · {} tokens ──", tokens(estimate(text)));
    format!("{}\n", style::mark(style::INFO_HEADING, &heading))
}

/// The instructions, each part under a heading naming it, then each tool's definition.
pub(super) fn text(prompt: &SystemPrompt) -> String {
    let tools: &[Value] = prompt.tools.as_array().map_or(&[], Vec::as_slice);
    let mut text = match prompt.sent {
        true => "What the session's last request sent: its instructions, then its tools.\n",
        false => {
            "No request has been sent yet: what the next request sends, its instructions, \
then its tools.\n"
        }
    }
    .to_owned();
    for part in &prompt.parts {
        text += "\n";
        text += &heading(&part.name, &part.text);
        text += &visible(part.text.trim_matches('\n'));
        text += "\n";
    }
    for tool in tools {
        let definition = serde_json::to_string_pretty(tool).unwrap_or_default();
        let name = tool["name"].as_str().unwrap_or("?");
        text += "\n";
        text += &heading(&format!("Tool {name}"), &definition);
        text += &visible(&definition);
        text += "\n";
    }
    if tools.is_empty() {
        text += "\nNo tools.\n";
    }
    text
}

/// The full screen's view of it.
pub(super) fn view(prompt: &SystemPrompt) -> ReportView {
    let tab = Tab {
        name: "System prompt".to_owned(),
        summary: text(prompt),
        details: String::new(),
    };
    ReportView::new("System prompt", "", vec![tab], 0, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet::agent::{Part, Section};
    use serde_json::json;

    #[test]
    fn each_part_and_each_tool_shows_under_a_heading_naming_it() {
        let prompt = SystemPrompt {
            parts: vec![
                Part {
                    section: Section::Opening,
                    name: "Opening".into(),
                    text: "You are a coding agent.\n".into(),
                },
                Part {
                    section: Section::ContextFile,
                    name: "/work/CLAUDE.md".into(),
                    text: "\n\nRun the tests.\u{1b}[31m\n\n".into(),
                },
            ],
            tools: json!([{"type": "function", "name": "read", "parameters": {}}]),
            sent: true,
        };
        let shown = style::themed(&text(&prompt), false);
        assert_eq!(
            shown,
            "What the session's last request sent: its instructions, then its tools.

── Opening · 6 tokens ──
You are a coding agent.

── /work/CLAUDE.md · 6 tokens ──
Run the tests.\\u{1b}[31m

── Tool read · 16 tokens ──
{
  \"name\": \"read\",
  \"parameters\": {},
  \"type\": \"function\"
}
"
        );
        let unsent = SystemPrompt {
            parts: Vec::new(),
            tools: json!([]),
            sent: false,
        };
        let shown = text(&unsent);
        assert!(shown.starts_with("No request has been sent yet"), "{shown}");
        assert!(shown.ends_with("\nNo tools.\n"), "{shown}");
    }
}
