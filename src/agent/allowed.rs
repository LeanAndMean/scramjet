//! Claude Code's `--allowedTools` and `--permission-prompts none`: which calls `--ask` lets run
//! without asking, and whether the others are refused rather than asked.
use super::tools::Call;
use super::{Agent, mcp};

/// What the command line allows, in Scramjet's tool names (`Bash` given as `bash`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Allowed {
    /// Whole tools (`bash`), MCP servers' tools (`mcp__tavily__search`, `mcp__tavily`,
    /// `mcp__tavily__*`), and rules for one tool's calls (`Bash(git *)`), which allow nothing.
    pub tools: Vec<String>,
    /// `--permission-prompts none`: a call that would ask is refused.
    pub refuse_asked: bool,
}

/// What the model is told of a call refused for `refuse_asked`.
pub(super) const REFUSED: &str = "This tool call needs the user's approval, and nobody answers \
approvals in this run (--permission-prompts none), so it did not run. Only the tools \
--allowedTools names run without approval.";

impl Allowed {
    /// The rules for one tool's calls, which Scramjet does not keep: their calls ask as without
    /// them.
    pub fn rules(&self) -> impl Iterator<Item = &str> {
        (self.tools.iter())
            .filter(|name| name.contains('('))
            .map(String::as_str)
    }

    /// Whether `tools` names an MCP server's tool.
    pub fn names_mcp(&self) -> bool {
        self.tools.iter().any(|name| name.starts_with("mcp__"))
    }

    pub(super) fn allows(&self, call: &Call) -> bool {
        if call.name != mcp::CALL {
            return self.tools.iter().any(|name| name == call.name);
        }
        let server = call.arguments["server"].as_str().unwrap_or_default();
        let tool = call.arguments["tool"].as_str().unwrap_or_default();
        let whole = format!("mcp__{server}");
        (self.tools.iter()).any(|name| match name.strip_prefix(&whole) {
            Some("" | "__*") => true,
            Some(rest) => rest.strip_prefix("__") == Some(tool),
            None => false,
        })
    }
}

impl Agent {
    pub fn set_allowed(&mut self, allowed: Allowed) {
        self.allowed = allowed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_tools_and_mcp_servers_tools_are_allowed_but_not_rules() {
        let allowed = Allowed {
            tools: ["bash", "Write(src/**)", "mcp__tavily__search", "mcp__docs"]
                .map(String::from)
                .to_vec(),
            refuse_asked: false,
        };
        let call = |name, arguments| Call { name, arguments };
        let mcp = |server, tool| call(mcp::CALL, json!({"server": server, "tool": tool}));
        assert!(allowed.allows(&call("bash", json!({"command": "rm -rf x"}))));
        assert!(!allowed.allows(&call("write", json!({"path": "src/a"}))));
        assert!(allowed.allows(&mcp("tavily", "search")));
        assert!(!allowed.allows(&mcp("tavily", "extract")));
        assert!(!allowed.allows(&mcp("tavily_x", "search")));
        assert!(allowed.allows(&mcp("docs", "anything")));
        assert!(!allowed.allows(&mcp("docsite", "anything")));
        let star = Allowed {
            tools: vec!["mcp__tavily__*".into()],
            refuse_asked: false,
        };
        assert!(star.allows(&mcp("tavily", "extract")));
        assert_eq!(allowed.rules().collect::<Vec<_>>(), ["Write(src/**)"]);
        assert!(allowed.names_mcp() && !Allowed::default().names_mcp());
    }
}
