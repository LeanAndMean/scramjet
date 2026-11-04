//! The models Ctrl+P cycles through, as Scramjet's: those the patterns of `--models` or the
//! settings' `enabledModels` choose, else all the endpoint's.
use super::settings::{Settings, card_level, scramjet_level};
use scramjet_rs::agent::{Agent, ModelOption};

/// A model in the scope, with the effort its pattern sets (`sonnet:high`), named as the model
/// cards name levels.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Scoped {
    pub(super) id: String,
    pub(super) level: Option<String>,
}

/// The models `patterns` choose among `models`, in the patterns' order and each once, as
/// Scramjet's `resolveModelScope`: a glob (`*`, `?`, `[…]`) takes every model whose id or
/// `provider/id` it matches, case aside; another pattern the model of that id, else the newest
/// whose id or name contains it, preferring an alias to a dated id. `:level` after either sets
/// the effort. Also returns what matched nothing.
pub(super) fn resolve(patterns: &[String], models: &[ModelOption]) -> (Vec<Scoped>, Vec<String>) {
    let mut scoped: Vec<Scoped> = Vec::new();
    let mut warnings = Vec::new();
    for pattern in patterns.iter().map(|pattern| pattern.trim()) {
        if pattern.is_empty() {
            continue;
        }
        let (body, level) = match pattern.rsplit_once(':') {
            // An id may have a colon of its own.
            _ if models
                .iter()
                .any(|model| model.id.eq_ignore_ascii_case(pattern)) =>
            {
                (pattern, None)
            }
            Some((body, suffix)) => match card_level(suffix) {
                Some(level) => (body, Some(level.to_owned())),
                None => {
                    warnings.push(format!(
                        "invalid thinking level \"{suffix}\" in pattern \"{pattern}\"; using the \
                         default"
                    ));
                    (body, None)
                }
            },
            None => (pattern, None),
        };
        let matched = matching(body, models);
        if matched.is_empty() {
            warnings.push(format!("no models match pattern \"{pattern}\""));
        }
        for model in matched {
            if !scoped.iter().any(|known| known.id == model.id) {
                let level = level.clone();
                scoped.push(Scoped {
                    id: model.id.clone(),
                    level,
                });
            }
        }
    }
    (scoped, warnings)
}

fn matching<'a>(pattern: &str, models: &'a [ModelOption]) -> Vec<&'a ModelOption> {
    let full = |model: &ModelOption| {
        (model.provider.as_ref()).map(|provider| format!("{provider}/{}", model.id))
    };
    if pattern.contains(['*', '?', '[']) {
        let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
        let matches = |text: &str| glob(&pattern, &text.to_lowercase().chars().collect::<Vec<_>>());
        return (models.iter())
            .filter(|model| matches(&model.id) || full(model).is_some_and(|full| matches(&full)))
            .collect();
    }
    let lower = pattern.to_lowercase();
    let exact = (models.iter()).find(|model| {
        model.id.to_lowercase() == lower
            || full(model).is_some_and(|full| full.to_lowercase() == lower)
    });
    if let Some(model) = exact {
        return vec![model];
    }
    let contains = |text: &str| text.to_lowercase().contains(&lower);
    let newest = (models.iter())
        .filter(|model| contains(&model.id) || model.name.as_deref().is_some_and(contains))
        .max_by(|a, b| (!dated(&a.id), &a.id).cmp(&(!dated(&b.id), &b.id)));
    newest.into_iter().collect()
}

/// Whether `id` ends in a date, as `claude-haiku-4-5-20251001` does, rather than being an alias.
fn dated(id: &str) -> bool {
    let tail = id.rsplit('-').next().unwrap_or("");
    !id.ends_with("-latest") && tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `text` matches the glob `pattern` whole, as minimatch matches a name: `*` any run of
/// characters and `?` any one, neither a `/`; `[…]` one of a set (`a-z` ranges, `!` or `^`
/// first for those not in it).
fn glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len())
            .take_while(|&skip| skip == 0 || text[skip - 1] != '/')
            .any(|skip| glob(rest, &text[skip..])),
        Some(('?', rest)) => text.first().is_some_and(|&c| c != '/') && glob(rest, &text[1..]),
        Some(('[', rest)) if let Some((member, after)) = set(rest) => {
            text.first().is_some_and(|&c| c != '/' && member(c)) && glob(after, &text[1..])
        }
        Some((&c, rest)) => text.first() == Some(&c) && glob(rest, &text[1..]),
    }
}

/// The set a `[` opens in a glob, as a test of a character, and the pattern after its `]`;
/// `None` without a `]`, when the `[` is itself.
fn set(pattern: &[char]) -> Option<(impl Fn(char) -> bool + '_, &[char])> {
    let negated = matches!(pattern.first(), Some('!' | '^'));
    let start = usize::from(negated);
    // A `]` right at the start is one of the set.
    let end = start + 1 + pattern.get(start + 1..)?.iter().position(|&c| c == ']')?;
    let members = &pattern[start..end];
    let test = move |c: char| {
        let mut index = 0;
        let mut found = false;
        while index < members.len() {
            if index + 2 < members.len() && members[index + 1] == '-' {
                found |= (members[index]..=members[index + 2]).contains(&c);
                index += 3;
            } else {
                found |= members[index] == c;
                index += 1;
            }
        }
        found != negated
    };
    Some((test, &pattern[end + 1..]))
}

/// The model `steps` Ctrl+P presses move to from `current` in `scope` (negative: Shift+Ctrl+P),
/// wrapping around; from a model not in it, the first press goes to the first, or back to the
/// last. `None` when that is the current model.
pub(super) fn step<'a>(scope: &'a [Scoped], current: &str, steps: isize) -> Option<&'a Scoped> {
    let count = scope.len() as isize;
    let at = match scope.iter().position(|scoped| scoped.id == current) {
        Some(at) => at as isize,
        None if steps > 0 => -1,
        None => count,
    };
    let next = scope.get((at + steps).checked_rem_euclid(count)? as usize)?;
    (next.id != current).then_some(next)
}

/// Ctrl+P or Shift+Ctrl+P, `steps` times (negative: back): switches to the next model in the
/// scope `patterns` choose, or among all the endpoint's models when they choose none, with the
/// effort a pattern sets, which is saved as the settings' default, as Scramjet's thinking
/// level. Returns what to say under the prompt, as Scramjet's status says it.
pub(super) fn cycle(
    agent: &mut Agent,
    settings: &mut Settings,
    patterns: &[String],
    steps: isize,
) -> String {
    let models = agent.model_options();
    let (scoped, _) = resolve(patterns, &models);
    let all = scoped.is_empty();
    let scope = match all {
        true => (models.iter())
            .map(|model| Scoped {
                id: model.id.clone(),
                level: None,
            })
            .collect(),
        false => scoped,
    };
    let Some(next) = step(&scope, agent.model(), steps) else {
        return match all {
            true => "Only one model available".to_owned(),
            false => "Only one model in scope".to_owned(),
        };
    };
    if let Err(error) = agent.set_model(&next.id) {
        return format!("error: {error}");
    }
    agent.load_model_card();
    let mut unsaved = None;
    if let Some(level) = &next.level {
        if let Err(error) = agent.set_effort(level.clone()) {
            return format!("error: {error}");
        }
        unsaved = settings.set_default_thinking_level(level).err();
    }
    let name = (models.iter())
        .find(|model| model.id == next.id)
        .and_then(|model| model.name.clone())
        .unwrap_or_else(|| next.id.clone());
    let thinking = (agent.effort())
        .filter(|&effort| agent.reasons() && effort != "none")
        .map(|effort| format!(" (thinking: {})", scramjet_level(effort)))
        .unwrap_or_default();
    match unsaved {
        Some(error) => format!("Switched to {name}{thinking} (settings not saved: {error})"),
        None => format!("Switched to {name}{thinking}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: Option<&str>, provider: Option<&str>) -> ModelOption {
        ModelOption {
            id: id.into(),
            name: name.map(str::to_owned),
            provider: provider.map(str::to_owned),
            reasoning: true,
            levels: Vec::new(),
            default_level: None,
        }
    }

    fn proxy() -> Vec<ModelOption> {
        vec![
            model(
                "claude-opus-5-5",
                Some("Claude Opus 5.5"),
                Some("anthropic"),
            ),
            model(
                "claude-sonnet-5",
                Some("Claude Sonnet 5"),
                Some("anthropic"),
            ),
            model(
                "claude-sonnet-5-5",
                Some("Claude Sonnet 5.5"),
                Some("anthropic"),
            ),
            model("claude-haiku-4-5-20251001", None, Some("anthropic")),
            model("claude-haiku-5-5", None, Some("anthropic")),
            model("gpt-6-luna", Some("GPT-6 Luna"), Some("openai")),
            model("gpt-5.6-luna", Some("GPT-5.6 Luna"), Some("openai")),
            model("local:7b", None, None),
        ]
    }

    fn ids(patterns: &[&str]) -> (Vec<(String, Option<String>)>, Vec<String>) {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        let (scoped, warnings) = resolve(&patterns, &proxy());
        let ids = scoped.into_iter().map(|s| (s.id, s.level)).collect();
        (ids, warnings)
    }

    fn just(ids: &[&str]) -> Vec<(String, Option<String>)> {
        ids.iter().map(|id| (id.to_string(), None)).collect()
    }

    #[test]
    fn a_pattern_names_a_model_or_the_newest_alias_that_contains_it() {
        assert_eq!(ids(&["CLAUDE-OPUS-5-5"]).0, just(&["claude-opus-5-5"]));
        assert_eq!(ids(&["openai/gpt-6-luna"]).0, just(&["gpt-6-luna"]));
        // The newest alias first, then a dated id; a display name matches too.
        assert_eq!(
            ids(&["sonnet", "haiku", "luna"]).0,
            just(&["claude-sonnet-5-5", "claude-haiku-5-5", "gpt-6-luna"])
        );
        assert_eq!(ids(&["4-5"]).0, just(&["claude-haiku-4-5-20251001"]));
        assert_eq!(ids(&["GPT-5.6 L"]).0, just(&["gpt-5.6-luna"]));
        assert_eq!(ids(&["local:7b"]).0, just(&["local:7b"]));
    }

    #[test]
    fn a_glob_takes_every_model_it_matches_in_the_endpoints_order_each_once() {
        assert_eq!(
            ids(&["*luna", "anthropic/*sonnet*", "gpt-6-luna"]).0,
            just(&[
                "gpt-6-luna",
                "gpt-5.6-luna",
                "claude-sonnet-5",
                "claude-sonnet-5-5"
            ])
        );
        // `*` does not take a `/`, as minimatch's.
        assert_eq!(
            ids(&["*/gpt-6-luna", "claude-haiku-?-5"]).0,
            just(&["gpt-6-luna", "claude-haiku-5-5"])
        );
        assert_eq!(
            ids(&["claude-[a-o]*", "gpt-[!6]*"]).0,
            just(&[
                "claude-opus-5-5",
                "claude-haiku-4-5-20251001",
                "claude-haiku-5-5",
                "gpt-5.6-luna"
            ])
        );
        assert!(ids(&["anthropic*"]).0.is_empty());
    }

    #[test]
    fn a_level_after_a_pattern_sets_the_effort_and_what_matches_nothing_is_said() {
        let high = |id: &str, level: &str| (id.to_string(), Some(level.to_string()));
        assert_eq!(
            ids(&[
                "sonnet:high",
                "*luna:off",
                "opus:hgih",
                "nothing",
                " ",
                "gemini:low"
            ]),
            (
                vec![
                    high("claude-sonnet-5-5", "high"),
                    high("gpt-6-luna", "none"),
                    high("gpt-5.6-luna", "none"),
                    ("claude-opus-5-5".to_string(), None),
                ],
                vec![
                    "invalid thinking level \"hgih\" in pattern \"opus:hgih\"; using the default"
                        .to_string(),
                    "no models match pattern \"nothing\"".to_string(),
                    "no models match pattern \"gemini:low\"".to_string(),
                ]
            )
        );
    }

    #[test]
    fn ctrl_p_steps_through_the_scope_and_wraps_around() {
        let scope: Vec<Scoped> = ["a", "b", "c"]
            .map(|id| Scoped {
                id: id.into(),
                level: None,
            })
            .to_vec();
        let at = |current: &str, steps| step(&scope, current, steps).map(|s| s.id.as_str());
        assert_eq!(
            [at("a", 1), at("c", 1), at("a", -1), at("b", 4), at("b", -2)],
            [Some("b"), Some("a"), Some("c"), Some("c"), Some("c")]
        );
        // From a model not in it, to the first or the last.
        assert_eq!(
            [at("x", 1), at("x", -1), at("x", 2)],
            [Some("a"), Some("c"), Some("b")]
        );
        // Back where it started, or nowhere to go.
        assert_eq!([at("a", 3), at("a", 0)], [None, None]);
        assert_eq!(step(&scope[..1], "a", 1), None);
        assert_eq!(step(&scope[..1], "x", 1).map(|s| s.id.as_str()), Some("a"));
        assert_eq!(step(&[], "x", 1), None);
    }
}
