//! `/plan`, as Claude Code's and Codex's: plan mode, in which the agent proposes a plan without
//! carrying it out (`Agent::set_planning`). After each plan turn that ends by itself, the full
//! screen asks whether to implement the plan, as Codex does.
use super::drive::Start;
use super::footer;
use super::interactive::Interactive;
use super::style;
use super::text::clip;

/// What `/plan` takes to end plan mode.
const OFF: [&str; 4] = ["off", "exit", "stop", "clear"];

/// What the prompt that implements the plan says, as Codex's.
const IMPLEMENT: &str = "Implement the plan.";

impl Interactive {
    /// `/plan` starts plan mode, `/plan PROMPT` starts it and sends the prompt, `/plan off` ends
    /// it.
    pub(super) fn plan(&mut self, argument: &str) {
        let planning = self.session.agent.planning();
        let off = OFF.contains(&argument.to_lowercase().as_str());
        let said = match (planning, off, argument) {
            (false, true, _) => "Plan mode is off",
            (true, true, _) => "Plan mode off: the agent can change files again",
            (true, false, "" | "on") => "Plan mode is on; /plan off ends it",
            (false, false, _) => {
                "Plan mode on: the agent proposes a plan without changing files; /plan off ends it"
            }
            (true, false, _) => "",
        };
        self.session.agent.set_planning(!off);
        self.footer_changed();
        if !said.is_empty() {
            self.say_styled(style::MUTED, said);
        }
        if !off && !matches!(argument, "" | "on") {
            if self.in_turn {
                let said = "(the prompt is not sent while a turn runs: send it once it ends)";
                return self.say_styled(style::MUTED, said);
            }
            self.session.chosen = Some((argument.to_owned(), Start::Typed));
        }
    }

    /// After a turn in plan mode that ended by itself, whether to implement the plan: if so,
    /// plan mode ends and the prompt that implements it is returned.
    pub(super) fn offer_plan(&mut self) -> Option<(String, Start)> {
        if !self.session.agent.planning() {
            return None;
        }
        if self.screen.is_none() {
            let said = "Plan mode: /plan off, then a prompt, implements the plan";
            self.say_styled(style::MUTED, said);
            return None;
        }
        let rows = [
            "Yes, implement the plan (plan mode ends)",
            "No, keep planning",
        ];
        let row = |at: usize, width| clip(rows[at], width);
        let chosen = self.select("Implement this plan?", rows.len(), rows.len(), 0, None, row);
        if chosen != Some(0) {
            return None;
        }
        self.session.agent.set_planning(false);
        self.footer_changed();
        Some((IMPLEMENT.to_owned(), Start::Typed))
    }

    pub(super) fn footer_changed(&self) {
        if let Some(screen) = &self.screen {
            screen.footer(footer::Shown::of(&self.session.agent));
        }
    }
}
