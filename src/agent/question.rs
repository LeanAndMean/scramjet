//! Scramjet's `get_scramjet_user_input` tool: the model asks the user to confirm, to choose one of
//! its options, or to reply at the prompt (freetext), as Scramjet's `user-input.ts` does.

use serde_json::{Value, json};

pub const NAME: &str = "get_scramjet_user_input";

/// Listed under the available tools.
pub(super) const SNIPPET: &str = "You have access to `get_scramjet_user_input` for asking the \
user something mid-turn: confirm (yes/no) and select (one of your options) return their answer \
as the tool result; freetext ends the turn, and they reply in the editor. During a Scramjet \
command, ask through this tool rather than with a question in prose, which gets a status check \
instead of an answer. The question stays visible in the tool row; give the context it needs in \
its message or in prose just before.";

pub(super) fn definition() -> Value {
    let kind = |value: &str| json!({"const": value, "type": "string"});
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Request structured input from the user. Supports confirm (yes/no), select \
(pick from options), and freetext (open-ended input). Confirm and select block until the user \
responds and return their answer as the tool result; freetext terminates the turn so the user \
replies in the standard editor. The placeholder parameter is accepted for compatibility but unused \
by freetext.",
        "parameters":{"type":"object","properties":{
            "type":{"anyOf":[kind("confirm"),kind("select"),kind("freetext")],
                "description":"The interaction type: confirm, select, or freetext."},
            "message":{"type":"string","description":"What you are asking, with what someone \
who has not followed your work needs to answer it: the situation, why you need their input, and \
what they could do, even if you are unsure (say so). Markdown renders. If you gave that context \
in prose just before, the question alone will do."},
            "options":{"type":"array","description":"Required for select type. The list of options to present.",
                "items":{"type":"object","properties":{
                    "value":{"type":"string"},"label":{"type":"string"},"description":{"type":"string"}},
                  "required":["value","label"]}},
            "recommended":{"type":"integer","minimum":0,
                "description":"For select type: zero-based index of the recommended option."},
            "placeholder":{"type":"string","description":"For freetext type: placeholder hint text \
shown in the input. Accepted for compatibility but unused by freetext."}},
          "required":["type","message"]}})
}

/// One option to choose.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub description: Option<String>,
    pub value: String,
}

#[derive(Debug, PartialEq)]
pub(super) enum Kind {
    Confirm,
    Select,
    Freetext,
}

#[derive(Debug)]
pub(super) struct Question {
    pub kind: Kind,
    pub message: String,
    pub options: Vec<Choice>,
    pub recommended: Option<usize>,
}

impl Question {
    /// The question a call asks; otherwise the error the model receives, in Scramjet's words.
    pub fn parse(arguments: &Value) -> Result<Self, String> {
        let kind = match arguments["type"].as_str() {
            Some("confirm") => Kind::Confirm,
            Some("select") => Kind::Select,
            Some("freetext") => Kind::Freetext,
            other => {
                return Err(format!(
                    "Unknown interaction type: {}",
                    other.unwrap_or("(none)")
                ));
            }
        };
        let message = arguments["message"].as_str().unwrap_or_default();
        if message.trim().is_empty() {
            return Err("Validation error: 'message' is required and must be non-empty.".into());
        }
        let mut question = Self {
            kind,
            message: message.to_owned(),
            options: Vec::new(),
            recommended: None,
        };
        if question.kind == Kind::Confirm {
            question.options = ["Yes", "No"].map(Choice::plain).to_vec();
        }
        if question.kind != Kind::Select {
            return Ok(question);
        }
        let options = arguments["options"].as_array().filter(|o| !o.is_empty());
        let Some(options) = options else {
            return Err(
                "Validation error: 'options' is required and must be a non-empty array \
for select type."
                    .into(),
            );
        };
        for (index, option) in options.iter().enumerate() {
            let text = |field: &str| {
                option[field]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
            };
            let (Some(value), Some(label)) = (text("value"), text("label")) else {
                return Err(format!(
                    "Validation error: 'options[{index}]' must include non-empty string 'value' \
and 'label'."
                ));
            };
            question.options.push(Choice {
                label: label.to_owned(),
                description: option["description"].as_str().map(str::to_owned),
                value: value.to_owned(),
            });
        }
        if let Some(recommended) = arguments.get("recommended").filter(|r| !r.is_null()) {
            match recommended.as_u64().map(|r| r as usize) {
                Some(index) if index < options.len() => question.recommended = Some(index),
                _ => {
                    return Err(format!(
                        "Validation error: 'recommended' index {recommended} is out of range \
(0-{}).",
                        options.len() - 1
                    ));
                }
            }
        }
        Ok(question)
    }

    /// The question asked as a freetext one, its options listed under its message for the
    /// user's reply.
    pub fn into_freetext(self) -> Self {
        let options: Vec<String> = (self.options.iter())
            .map(|option| match &option.description {
                Some(description) => format!("- {} — {description}", option.label),
                None => format!("- {}", option.label),
            })
            .collect();
        Self {
            kind: Kind::Freetext,
            message: format!(
                "{}\n\nReply with one of:\n{}",
                self.message,
                options.join("\n")
            ),
            options: Vec::new(),
            recommended: None,
        }
    }

    /// What the model receives for the user's choice (`None`: cancelled), and whether the turn
    /// ends there, as Scramjet ends it after a freetext question or a cancelled one. `parked`:
    /// a freetext question left the active command waiting for the reply.
    pub fn answer(&self, chosen: Option<usize>, parked: bool) -> (String, bool) {
        let (answer, ends) = match (&self.kind, chosen) {
            (Kind::Freetext, _) => (json!({ "parked": parked }), true),
            (_, None) => (json!({"cancelled": true}), true),
            (Kind::Confirm, Some(index)) => (json!({"confirmed": index == 0}), false),
            (Kind::Select, Some(index)) => (json!({"selected": self.options[index].value}), false),
        };
        (answer.to_string(), ends)
    }
}

impl Choice {
    pub(super) fn plain(label: &str) -> Self {
        Self {
            label: label.into(),
            description: None,
            value: label.into(),
        }
    }
}

/// The question and what the user answered, as Scramjet's result row shows them, from the call's
/// arguments and its result; `None` for a result that is an error.
pub fn shown(arguments: &Value, output: &str) -> Option<(String, String)> {
    let question = Question::parse(arguments).ok()?;
    let answer: Value = serde_json::from_str(output).ok()?;
    let option = |choice: &Choice| match &choice.description {
        Some(description) => format!("{} — {description}", choice.label),
        None => choice.label.clone(),
    };
    let text = match question.kind {
        _ if answer.get("parked").is_some() => match answer["parked"] == true {
            true => "Waiting for your reply".to_owned(),
            false => "Reply at the prompt".to_owned(),
        },
        Kind::Freetext => return None,
        _ if answer["cancelled"] == true && question.kind == Kind::Select => {
            let options = question.options.iter().map(|o| format!("- {}", option(o)));
            ["Options:".to_owned()]
                .into_iter()
                .chain(options)
                .chain(["Cancelled".to_owned()])
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ if answer["cancelled"] == true => "Cancelled".to_owned(),
        Kind::Confirm => match answer["confirmed"].as_bool()? {
            true => "Answer: Yes".to_owned(),
            false => "Answer: No".to_owned(),
        },
        Kind::Select => {
            let selected = answer["selected"].as_str()?;
            let rows = question.options.iter().map(|choice| {
                let mark = if choice.value == selected {
                    "→ "
                } else {
                    "  "
                };
                format!("{mark}{}", option(choice))
            });
            rows.collect::<Vec<_>>().join("\n")
        }
    };
    Some((question.message, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_questions_get_scramjets_errors() {
        let error = |arguments: Value| Question::parse(&arguments).unwrap_err();
        assert_eq!(
            error(json!({"type":"choose","message":"Which?"})),
            "Unknown interaction type: choose"
        );
        assert_eq!(
            error(json!({"type":"confirm","message":"  "})),
            "Validation error: 'message' is required and must be non-empty."
        );
        assert_eq!(
            error(json!({"type":"select","message":"Which?","options":[]})),
            "Validation error: 'options' is required and must be a non-empty array for select type."
        );
        assert_eq!(
            error(json!({"type":"select","message":"Which?",
                "options":[{"value":"a","label":"A"},{"value":"b","label":" "}]})),
            "Validation error: 'options[1]' must include non-empty string 'value' and 'label'."
        );
        assert_eq!(
            error(json!({"type":"select","message":"Which?","recommended":2,
                "options":[{"value":"a","label":"A"},{"value":"b","label":"B"}]})),
            "Validation error: 'recommended' index 2 is out of range (0-1)."
        );
        // Options are only read for a select question.
        let freetext = json!({"type":"freetext","message":"Why?","options":[]});
        assert!(Question::parse(&freetext).is_ok());
    }

    #[test]
    fn answers_are_scramjets_and_shown_with_the_question() {
        let select = json!({"type":"select","message":"Which **one**?","recommended":1,
            "options":[{"value":"a","label":"Alpha","description":"first"},{"value":"b","label":"Beta"}]});
        let question = Question::parse(&select).unwrap();
        assert_eq!(question.recommended, Some(1));
        let (chosen, ends) = question.answer(Some(1), false);
        assert_eq!((chosen.as_str(), ends), (r#"{"selected":"b"}"#, false));
        assert_eq!(
            shown(&select, &chosen).unwrap(),
            ("Which **one**?".into(), "  Alpha — first\n→ Beta".into())
        );
        let (cancelled, ends) = question.answer(None, false);
        assert_eq!((cancelled.as_str(), ends), (r#"{"cancelled":true}"#, true));
        assert_eq!(
            shown(&select, &cancelled).unwrap().1,
            "Options:\n- Alpha — first\n- Beta\nCancelled"
        );

        let confirm = json!({"type":"confirm","message":"Go?"});
        let question = Question::parse(&confirm).unwrap();
        assert_eq!(question.options.len(), 2);
        for (chosen, sent, text) in [
            (Some(0), r#"{"confirmed":true}"#, "Answer: Yes"),
            (Some(1), r#"{"confirmed":false}"#, "Answer: No"),
            (None, r#"{"cancelled":true}"#, "Cancelled"),
        ] {
            let (answer, ends) = question.answer(chosen, false);
            assert_eq!((answer.as_str(), ends), (sent, chosen.is_none()));
            assert_eq!(shown(&confirm, &answer).unwrap().1, text);
        }

        let freetext = json!({"type":"freetext","message":"Why?"});
        let (reply, ends) = Question::parse(&freetext).unwrap().answer(None, false);
        assert_eq!((reply.as_str(), ends), (r#"{"parked":false}"#, true));
        assert_eq!(shown(&freetext, &reply).unwrap().1, "Reply at the prompt");
        let (parked, _) = Question::parse(&freetext).unwrap().answer(None, true);
        assert_eq!(
            shown(&freetext, &parked).unwrap().1,
            "Waiting for your reply"
        );
        // An error result is shown as it is.
        assert_eq!(shown(&json!({"type":"confirm"}), "Validation error"), None);
        assert_eq!(shown(&confirm, "UI interaction failed"), None);
    }
}
