//! Where the model changed on a branch, so the next request can say so where it happened: each
//! `model_change` record after a reply, but those the agent's own `switch_scramjet_model` made.

use serde_json::Value;
use std::collections::HashSet;

/// A change of the model before the conversation's item `at` (its length: after the last item).
#[derive(Clone, Debug, PartialEq)]
pub struct ModelChange {
    pub at: usize,
    pub from: String,
    pub to: String,
}

/// The changes on a branch, followed entry by entry as `Entries::context` reads them and as
/// `Session::append` writes them, counting the items the conversation holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ModelChanges {
    items: usize,
    replied: bool,
    /// The agent's switch calls not yet answered: a change while one runs is the agent's.
    switching: HashSet<String>,
    changes: Vec<ModelChange>,
}

impl ModelChanges {
    pub(super) fn list(&self) -> &[ModelChange] {
        &self.changes
    }

    /// Follows the entry `value`, `model` the one in effect before it.
    pub(super) fn see(&mut self, value: &Value, model: Option<&str>) {
        match value["type"].as_str() {
            Some("compaction") => {
                let cut = (value["first_kept"].as_u64())
                    .and_then(|cut| usize::try_from(cut).ok())
                    .unwrap_or(0)
                    .min(self.items);
                self.changes.retain(|change| change.at >= cut);
                for change in &mut self.changes {
                    change.at = change.at - cut + 1;
                }
                self.items = self.items - cut + 1;
            }
            Some("model_change") => self.changed(model, value["model"].as_str()),
            Some("thinking_level_change" | "command_state" | "autopilot") => {}
            kind => {
                let call = value["call_id"].as_str().unwrap_or_default();
                if kind == Some("function_call") && value["name"] == crate::agent::switch::NAME {
                    self.switching.insert(call.to_owned());
                } else if kind == Some("function_call_output") {
                    self.switching.remove(call);
                }
                self.replied |= value[crate::agent::PRODUCER].is_string();
                self.items += 1;
            }
        }
    }

    fn changed(&mut self, from: Option<&str>, to: Option<&str>) {
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        if from == to || !self.replied || !self.switching.is_empty() {
            return;
        }
        match self.changes.last_mut() {
            // Changes before the next item make one, or none when they come back.
            Some(last) if last.at == self.items => {
                if last.from == to {
                    self.changes.pop();
                } else {
                    last.to = to.to_owned();
                }
            }
            _ => self.changes.push(ModelChange {
                at: self.items,
                from: from.to_owned(),
                to: to.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn follow(entries: &[Value]) -> ModelChanges {
        let mut changes = ModelChanges::default();
        let mut model: Option<String> = None;
        for entry in entries {
            changes.see(entry, model.as_deref());
            if let Some(next) = (entry["model"].as_str()).or(entry["_scramjet_model"].as_str()) {
                model = Some(next.to_owned());
            }
        }
        changes
    }

    fn at(at: usize, from: &str, to: &str) -> ModelChange {
        ModelChange {
            at,
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn a_change_after_a_reply_is_kept_where_it_happened() {
        let changes = follow(&[
            json!({"type":"model_change","model":"a"}),
            json!({"role":"user","content":"one"}),
            json!({"type":"model_change","model":"b"}),
            json!({"type":"message","role":"assistant","_scramjet_model":"b"}),
            json!({"type":"model_change","model":"c"}),
            json!({"role":"user","content":"two"}),
            json!({"type":"message","role":"assistant","_scramjet_model":"c"}),
            json!({"type":"model_change","model":"a"}),
        ]);
        // Before any reply there is nothing to tell apart.
        assert_eq!(changes.list(), [at(2, "b", "c"), at(4, "c", "a")]);
    }

    #[test]
    fn a_burst_of_changes_makes_one_and_one_that_comes_back_none() {
        let reply = json!({"type":"message","role":"assistant","_scramjet_model":"a"});
        let change = |model: &str| json!({"type":"model_change","model":model});
        let burst = follow(&[reply.clone(), change("b"), change("c")]);
        assert_eq!(burst.list(), [at(1, "a", "c")]);
        let back = follow(&[reply, change("b"), change("a")]);
        assert_eq!(back.list(), []);
    }

    #[test]
    fn the_agents_own_switch_is_not_a_change_to_mark() {
        let changes = follow(&[
            json!({"type":"function_call","call_id":"s","name":"switch_scramjet_model",
                "_scramjet_model":"a"}),
            json!({"type":"model_change","model":"b"}),
            json!({"type":"function_call_output","call_id":"s"}),
            json!({"type":"model_change","model":"c"}),
        ]);
        assert_eq!(changes.list(), [at(2, "b", "c")]);
    }

    #[test]
    fn a_compaction_drops_the_changes_it_summarized_and_moves_the_rest() {
        let reply =
            |model: &str| json!({"type":"message","role":"assistant","_scramjet_model":model});
        let change = |model: &str| json!({"type":"model_change","model":model});
        let changes = follow(&[
            reply("a"),
            change("b"),
            reply("b"),
            change("c"),
            reply("c"),
            change("d"),
            json!({"type":"compaction","first_kept":2,"summary":"s"}),
        ]);
        assert_eq!(changes.list(), [at(1, "b", "c"), at(2, "c", "d")]);
    }
}
