//! Scramjet's `switch_scramjet_model` tool (`model-switch-tool.ts`): the agent changes the
//! session's model for its next requests, as `/model` does, and its call row is the record.

use super::{Agent, ModelOption, status};
use serde_json::{Value, json};

pub const NAME: &str = "switch_scramjet_model";

/// Listed under the available tools.
pub(super) const SNIPPET: &str = "You have access to `switch_scramjet_model` to change the \
active model mid-session. Provide the target model's `provider` and `model` ID; the switch takes \
effect on the next model request and is recorded as a tool row. An unknown target returns an \
error and leaves the current model unchanged (no silent fallback).";

pub(super) fn definition() -> Value {
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Change the active harness model for this session. Provide the target \
model's provider and ID. The switch takes effect on the next model request. An unknown target \
returns an error and leaves the current model unchanged.",
        "parameters":{"type":"object","properties":{
            "provider":{"type":"string",
                "description":"Provider of the target model, e.g. 'anthropic' or 'openai'."},
            "model":{"type":"string",
                "description":"Model ID to switch to, e.g. 'claude-opus-4-8'."}},
          "required":["provider","model"]}})
}

/// The call line's text.
pub(super) fn summary(arguments: &Value) -> String {
    let text = |key: &str| arguments[key].as_str().unwrap_or_default();
    format!("{}/{}", text("provider"), text("model"))
}

/// What a call switched to, or what the model receives instead and whether that is a failure.
pub(super) enum Switched {
    To(String),
    Not(String, bool),
}

impl Agent {
    /// Runs a call as Scramjet's tool does: the model a provider offers with that id and
    /// provider becomes the current one, recorded in the session; an unknown one changes
    /// nothing, and the model is told which there are.
    pub(super) fn switch_model(&mut self, arguments: &Value) -> Switched {
        let (provider, model) = match given(arguments) {
            Ok(given) => given,
            Err(errors) => {
                let errors: Vec<&str> = errors.iter().map(String::as_str).collect();
                return Switched::Not(status::validation_failed(NAME, &errors, arguments), true);
            }
        };
        let Some(models) = self.listed_models() else {
            return Switched::Not(
                format!(
                    "Cannot switch to \"{provider}/{model}\": no provider offers any model. The \
model was not switched."
                ),
                false,
            );
        };
        let found = models.iter().find(|option| {
            option.id == model && (option.provider.as_deref()).is_none_or(|known| known == provider)
        });
        let Some(found) = found else {
            let catalog = catalog(&models, &self.model_scope);
            return Switched::Not(
                format!(
                    "Unknown model \"{provider}/{model}\". The model was not switched.\n{catalog}"
                ),
                false,
            );
        };
        let name = found.name.as_deref().unwrap_or(&found.id);
        if found.reference() == self.config.model {
            let text = format!(
                "Already on {name} ({}). No switch needed.",
                found.reference()
            );
            return Switched::Not(text, false);
        }
        if let Err(error) = self.set_model(found.reference()) {
            let text = format!("Failed to switch to {name}: {error}. The model was not changed.");
            return Switched::Not(text, false);
        }
        self.load_model_info();
        Switched::To(match &found.provider {
            Some(provider) => format!(
                "Switched to {name} (ID: {}, provider: {provider}).",
                found.id
            ),
            None => format!("Switched to {name} (ID: {}).", found.id),
        })
    }
}

/// The provider and model asked for, or what is wrong with them as Pi's validation says it.
fn given(arguments: &Value) -> Result<(&str, &str), Vec<String>> {
    let field = |key: &str| match arguments.get(key) {
        None => Err(format!("  - {key}: must have required property")),
        Some(Value::String(text)) => Ok(text.as_str()),
        Some(_) => Err(format!("  - {key}: must be string")),
    };
    match (field("provider"), field("model")) {
        (Ok(provider), Ok(model)) => Ok((provider, model)),
        (provider, model) => Err([provider.err(), model.err()]
            .into_iter()
            .flatten()
            .collect()),
    }
}

/// The models a switch can go to, as Scramjet lists them after an unknown one: those of the
/// user's scope when there is one, else all the providers', sorted.
fn catalog(models: &[ModelOption], scope: &[String]) -> String {
    let mut lines: Vec<String> = (models.iter())
        .filter(|model| scope.is_empty() || scope.contains(&model.reference()))
        .map(|model| {
            let name = model.name.as_deref().unwrap_or(&model.id);
            format!("- {} ({name})", model.reference())
        })
        .collect();
    if lines.is_empty() {
        return "No models are available.".into();
    }
    lines.sort();
    format!("Available models (provider/id):\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_all_listed_models_without_a_scope_and_ids_without_a_card() {
        let model = |id: &str, provider: Option<&str>, name: Option<&str>| ModelOption {
            id: id.into(),
            name: name.map(str::to_owned),
            provider: provider.map(str::to_owned),
            reasoning: true,
            levels: Vec::new(),
            window: None,
            output: None,
            images: true,
            rate: None,
        };
        let models = [
            model("gpt-x", Some("openai"), Some("GPT X")),
            model("bare", None, None),
            model("claude-y", Some("anthropic"), Some("Claude Y")),
        ];
        assert_eq!(
            catalog(&models, &[]),
            "Available models (provider/id):\n- anthropic/claude-y (Claude Y)\n- bare (bare)\n- \
openai/gpt-x (GPT X)"
        );
        assert_eq!(catalog(&[], &[]), "No models are available.");
        let arguments = json!({"provider":"openai","model":"gpt-x"});
        assert_eq!(summary(&arguments), "openai/gpt-x");
    }
}
