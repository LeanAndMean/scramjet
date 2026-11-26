//! `/goal`, as Claude Code's: a condition the agent works toward, checked after each turn that
//! ends by itself (`Agent::check_goal`); until a check finds it met, its reason goes back to the
//! agent as the next prompt. Escape stops the work and keeps the goal for the next prompt.
use super::drive::Start;
use super::interactive::Interactive;
use super::style;
use scramjet::agent::AgentError;

/// What `/goal` takes to clear the goal, as Claude Code's.
const CLEAR: [&str; 6] = ["clear", "stop", "off", "reset", "none", "cancel"];

/// The session's goal.
pub(super) struct Goal {
    condition: String,
    /// How many checks found it not met.
    checks: usize,
    /// The reason the last check gave.
    reason: Option<String>,
}

impl Interactive {
    /// `/goal CONDITION` sets the goal and starts working toward it; typed during a turn, it is
    /// first checked when the turn ends. `/goal` says the goal, `/goal clear` clears it.
    pub(super) fn goal(&mut self, argument: &str) {
        if argument.is_empty() {
            let said = match &self.goal {
                None => "No goal set. Usage: /goal <condition>, /goal clear".to_owned(),
                Some(goal) => match (&goal.reason, goal.checks) {
                    (None, _) => format!("Goal: {} (not checked yet)", goal.condition),
                    (Some(reason), checks) => format!(
                        "Goal: {} (not met at {checks} {}; last: {reason})",
                        goal.condition,
                        if checks == 1 { "check" } else { "checks" }
                    ),
                },
            };
            return self.say_styled(style::MUTED, &said);
        }
        if CLEAR.contains(&argument.to_lowercase().as_str()) {
            let said = match self.goal.take() {
                Some(goal) => format!("Goal cleared: {}", goal.condition),
                None => "No goal set".to_owned(),
            };
            return self.say_styled(style::MUTED, &said);
        }
        self.goal = Some(Goal {
            condition: argument.to_owned(),
            checks: 0,
            reason: None,
        });
        if self.in_turn {
            let said = format!("Goal set: {argument} (checked when this turn ends)");
            return self.say_styled(style::MUTED, &said);
        }
        self.session.agent.attach_goal(true);
        self.session.chosen = Some((format!("/goal {argument}"), Start::Typed));
    }

    /// After a turn that ended by itself, whether the goal is met: if not, the prompt that goes
    /// on, with the check's reason; if so, or if it cannot be, the goal ends. A check that fails
    /// or is cancelled keeps the goal and stops.
    pub(super) fn check_goal(&mut self) -> Option<(String, Start)> {
        let condition = self.goal.as_ref()?.condition.clone();
        let result = self.during_turn(|agent, ui, cancel| agent.check_goal(&condition, ui, cancel));
        let check = match result {
            Ok(check) => check,
            Err(AgentError::Cancelled) => {
                self.say_styled(style::MUTED, "(goal check cancelled; the goal stays)");
                return None;
            }
            Err(error) => {
                self.say(&format!("Error: {error}"));
                self.say_styled(style::MUTED, "The goal stays; /goal clear clears it");
                return None;
            }
        };
        let verdict = &check.verdict;
        let (said, next) = match (verdict.met, verdict.impossible) {
            (true, _) => (Some(format!("Goal met: {}", verdict.reason)), None),
            (false, true) => (
                Some(format!("Goal ended, it cannot be met: {}", verdict.reason)),
                None,
            ),
            (false, false) => (None, Some(format!("Goal not met yet: {}", verdict.reason))),
        };
        match &next {
            Some(_) => {
                let goal = self.goal.as_mut()?;
                goal.checks += 1;
                goal.reason = Some(verdict.reason.clone());
            }
            None => self.goal = None,
        }
        if let Err(error) = self.session.agent.keep_goal_check(check) {
            self.say(&format!("Error: {error}"));
        }
        if let Some(said) = said {
            self.say(&said);
        }
        let next = next?;
        self.session.agent.attach_goal(false);
        Some((next, Start::Typed))
    }
}
