//! Choosing models: `/model`, and the scope Ctrl+P cycles through, as Scramjet's: the models the
//! patterns of `--models` or the settings' `enabledModels` choose, else all the providers';
//! `/scoped-models` chooses them.
use super::editor::Note;
use super::fuzzy::{self, Haystack};
use super::interactive::{Interactive, model_line};
use super::keymap::{self, Binding};
use super::select::{self, Act, Picked, Picker, Spot};
use super::settings::{Settings, card_level};
use super::style::{MODEL_OFF, MODEL_ON, MUTED, RESET};
use super::terminal;
use super::text::{clip, visible};
use scramjet_rs::agent::{Agent, ModelOption};
use unicode_width::UnicodeWidthStr;

/// What the scope is called, after `models`: `Models for Ctrl+P`.
fn scope_name(models: &str) -> String {
    match keymap::key(Binding::ModelCycleForward) {
        Some(key) => format!("{models} for {key}"),
        None => format!("{models} to cycle through"),
    }
}

/// What `/scoped-models`' keys do, under its list.
fn scoped_acts() -> String {
    let switches = match keymap::key(Binding::ModelCycleForward) {
        Some(key) => format!("{key} switches among those on, in this order."),
        None => "Cycling switches among those on, in this order.".to_owned(),
    };
    let act =
        |bindings: &[Binding], what| keymap::keys(bindings).map(|keys| format!("{keys}: {what}"));
    let acts = keymap::hint([
        act(
            &[Binding::ModelsEnableAll, Binding::ModelsClearAll],
            "all shown on/off",
        ),
        act(&[Binding::ModelsToggleProvider], "its provider's"),
        act(
            &[Binding::ModelsReorderUp, Binding::ModelsReorderDown],
            "move it",
        ),
    ]);
    format!("{switches}\n{acts}")
}

/// A model in the scope, with the effort its pattern sets (`sonnet:high`), named as requests
/// send levels.
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

/// The model `reference` names among `models`, as Scramjet's `findExactModelReferenceMatch`:
/// the one whose `provider/id`, else whose id, it is, case aside; `None` when none or several are.
pub(super) fn exact<'a>(reference: &str, models: &'a [ModelOption]) -> Option<&'a ModelOption> {
    let reference = reference.trim().to_lowercase();
    if reference.is_empty() {
        return None;
    }
    let full = |model: &ModelOption| {
        (model.provider.as_ref()).map(|provider| format!("{provider}/{}", model.id).to_lowercase())
    };
    let only = |found: Vec<&'a ModelOption>| match found[..] {
        [model] => Some(model),
        _ => None,
    };
    let canonical: Vec<_> = (models.iter())
        .filter(|model| full(model).is_some_and(|full| full == reference))
        .collect();
    if !canonical.is_empty() {
        return only(canonical);
    }
    only(
        (models.iter())
            .filter(|model| model.id.to_lowercase() == reference)
            .collect(),
    )
}

/// The model `pattern` names among `models`, as Scramjet resolves `--model`: the one whose id or
/// `provider/id` it is (`exact`), else after a provider's `provider/` one of its models, else any,
/// as `named_partly` finds it. `None` when it names none.
pub(super) fn named(pattern: &str, models: &[ModelOption]) -> Option<Scoped> {
    let pattern = pattern.trim();
    if let Some(model) = exact(pattern, models) {
        let id = model.id.clone();
        return Some(Scoped { id, level: None });
    }
    let provider = pattern.split_once('/').and_then(|(provider, rest)| {
        let theirs: Vec<ModelOption> = (models.iter())
            .filter(|model| {
                (model.provider.as_deref()).is_some_and(|p| p.eq_ignore_ascii_case(provider))
            })
            .cloned()
            .collect();
        (!theirs.is_empty()).then_some((theirs, rest))
    });
    (provider.and_then(|(theirs, rest)| named_partly(rest, &theirs)))
        .or_else(|| named_partly(pattern, models))
}

/// The model `pattern` names as Scramjet's strict `parseModelPattern` finds it: the newest whose
/// id or name contains it, preferring an alias (`matching`; a glob names none), else, before a
/// last `:level`, the model the rest names, at that level.
fn named_partly(pattern: &str, models: &[ModelOption]) -> Option<Scoped> {
    if !pattern.contains(['*', '?', '['])
        && let Some(model) = matching(pattern, models).first()
    {
        let id = model.id.clone();
        return Some(Scoped { id, level: None });
    }
    let (body, suffix) = pattern.rsplit_once(':')?;
    let level = Some(card_level(suffix)?.to_owned());
    Some(Scoped {
        level,
        ..named_partly(body, models)?
    })
}

/// The providers' `models` as `--list-models` prints them, as Scramjet's: those `search` matches
/// (all without one) by provider and id, each with its context window, its output limit, and
/// whether it reasons and takes images.
pub(super) fn listing(models: &[ModelOption], search: Option<&str>) -> String {
    let provider = |model: &ModelOption| model.provider.clone().unwrap_or_default();
    let mut shown: Vec<&ModelOption> = match search {
        Some(search) => {
            let rows: Vec<Haystack> = (models.iter())
                .map(|model| Haystack::new(&format!("{} {}", provider(model), model.id)))
                .collect();
            let found = fuzzy::filter(&fuzzy::parse(search), &rows);
            found.into_iter().map(|index| &models[index]).collect()
        }
        None => models.iter().collect(),
    };
    if shown.is_empty() {
        return match search {
            Some(search) => format!("No models matching \"{search}\"\n"),
            None => "No provider offers any model\n".to_owned(),
        };
    }
    shown.sort_by(|a, b| (provider(a), &a.id).cmp(&(provider(b), &b.id)));
    let yes = |yes: bool| if yes { "yes" } else { "no" }.to_owned();
    let header = [
        "provider", "model", "context", "max-out", "thinking", "images",
    ];
    let rows: Vec<[String; 6]> = (Some(header.map(str::to_owned)).into_iter())
        .chain(shown.iter().map(|model| {
            [
                provider(model),
                model.id.clone(),
                token_count(model.window),
                token_count(model.output),
                yes(model.reasoning),
                yes(model.images),
            ]
        }))
        .collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].width())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut text = String::new();
    for row in &rows {
        let cells = row.iter().zip(&widths);
        let line = cells
            .map(|(cell, &width)| format!("{cell}{}", " ".repeat(width - cell.width())))
            .collect::<Vec<_>>()
            .join("  ");
        text += line.trim_end();
        text.push('\n');
    }
    text
}

/// A number of tokens as Scramjet's model list writes it: `200K`, `1M`, `1.1M`; `-` when unknown.
fn token_count(count: Option<u64>) -> String {
    let Some(count) = count else {
        return "-".to_owned();
    };
    let (value, unit) = match count {
        1_000_000.. => (count as f64 / 1e6, "M"),
        1_000.. => (count as f64 / 1e3, "K"),
        _ => return count.to_string(),
    };
    match value.fract() == 0.0 {
        true => format!("{value}{unit}"),
        false => format!("{value:.1}{unit}"),
    }
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
/// scope `patterns` choose, or among all the providers' models when they choose none, with the
/// effort a pattern sets. As in Scramjet, the model is remembered for sessions started later
/// and the effort saved as the settings' default. Returns what to say, as Scramjet's status says
/// it but for a switch, which the footer shows.
pub(super) fn cycle(
    agent: &mut Agent,
    settings: &mut Settings,
    patterns: &[String],
    steps: isize,
) -> Option<Note> {
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
        return Some(Note::Info(
            match all {
                true => "Only one model available",
                false => "Only one model in scope",
            }
            .to_owned(),
        ));
    };
    if let Err(error) = agent.set_model(&next.id) {
        return Some(Note::Error(format!("Error: {error}")));
    }
    agent.load_model_info();
    let mut unsaved = settings.remember_model(agent.root(), &next.id).err();
    if let Some(level) = &next.level {
        if let Err(error) = agent.set_effort(level.clone()) {
            return Some(Note::Error(format!("Error: {error}")));
        }
        unsaved = unsaved.or(settings.set_default_thinking_level(level).err());
    }
    unsaved.map(|error| Note::Error(format!("Error: settings not saved: {error}")))
}

/// The models `/scoped-models` has on, among `all` (the providers', in their order), as
/// Scramjet's list keeps them: `on: None` for all of them, else those on, in the order Ctrl+P
/// takes them.
#[derive(Debug, PartialEq)]
struct Chosen {
    all: Vec<String>,
    on: Option<Vec<String>>,
}

impl Chosen {
    fn is_on(&self, id: &str) -> bool {
        (self.on.as_ref()).is_none_or(|on| on.iter().any(|known| known == id))
    }

    /// The models as the list shows them: those on, in their order, then the others.
    fn order(&self) -> Vec<String> {
        let on = self.on.clone().unwrap_or_default();
        let off = (self.all.iter()).filter(|id| !on.contains(id)).cloned();
        let off: Vec<_> = off.collect();
        [on, off].concat()
    }

    /// Enter: switches `id` on or off; with all on, the first leaves only it on, as Scramjet's.
    fn toggle(&mut self, id: &str) {
        match &mut self.on {
            None => self.on = Some(vec![id.to_owned()]),
            Some(on) => match on.iter().position(|known| known == id) {
                Some(at) => drop(on.remove(at)),
                None => on.push(id.to_owned()),
            },
        }
    }

    /// Ctrl+A: switches `ids` on, after those on; all of them on is all on.
    fn enable(&mut self, ids: &[String]) {
        let Some(on) = &mut self.on else {
            return;
        };
        for id in ids {
            if !on.contains(id) {
                on.push(id.clone());
            }
        }
        if on.len() == self.all.len() {
            self.on = None;
        }
    }

    /// Ctrl+X: switches `ids` off.
    fn clear(&mut self, ids: &[String]) {
        let on = self.on.take().unwrap_or_else(|| self.all.clone());
        self.on = Some(on.into_iter().filter(|id| !ids.contains(id)).collect());
    }

    /// Ctrl+P: switches `ids`, a provider's models, on, or off when all of them are.
    fn provider(&mut self, ids: &[String]) {
        match ids.iter().all(|id| self.is_on(id)) {
            true => self.clear(ids),
            false => self.enable(ids),
        }
    }

    /// Alt+Up (`-1`) or Alt+Down (`1`): moves `id`, when on, a place among those on. Returns
    /// whether it moved.
    fn shift(&mut self, id: &str, by: isize) -> bool {
        let Some(on) = &mut self.on else {
            return false;
        };
        let Some(at) = on.iter().position(|known| known == id) else {
            return false;
        };
        match at.checked_add_signed(by).filter(|&to| to < on.len()) {
            Some(to) => {
                on.swap(at, to);
                true
            }
            None => false,
        }
    }

    /// The scope as the session and the settings keep it: those on, or none for all, which is
    /// also what none on means, as in Scramjet.
    fn scope(&self) -> Vec<String> {
        match &self.on {
            Some(on) if on.len() < self.all.len() => on.clone(),
            _ => Vec::new(),
        }
    }

    /// How many are on, as the list says.
    fn count(&self) -> String {
        match &self.on {
            None => "all on".to_owned(),
            Some(on) => format!("{} of {} on", on.len(), self.all.len()),
        }
    }
}

impl Interactive {
    /// `/model`, as Scramjet's: sets the model `wanted` names (`exact`); otherwise, on the full
    /// screen, lists the models with `wanted` as the query (see `pick_model`). With piped input
    /// and nothing wanted, it lists the providers' models. The model set is remembered for
    /// sessions started later.
    pub(super) fn model(&mut self, wanted: &str) {
        let Some(models) = self.agent.listed_models() else {
            return self.say("No provider offers any model");
        };
        let chosen = match exact(wanted, &models) {
            Some(model) => Some(model.id.clone()),
            None if self.interactive() => self.pick_model(&models, wanted),
            None if wanted.is_empty() => {
                for model in &models {
                    let mark = if model.id == self.agent.model() {
                        "*"
                    } else {
                        " "
                    };
                    eprintln!("{mark} {}", visible(&model.id));
                }
                None
            }
            None => {
                self.say(&format!("Error: no model {wanted} (/model lists them)"));
                None
            }
        };
        if let Some(model) = chosen {
            match self.agent.set_model(&model) {
                Ok(()) => {
                    self.agent.load_model_info();
                    self.say(&format!("✓ Model set to {}", model_line(&self.agent)));
                    self.remember_model();
                }
                Err(error) => self.say(&format!("Error: {error}")),
            }
        }
    }

    /// `/model`'s list, as Scramjet's selector: each model with its provider, and its name under
    /// the list, searched from `query`. With a scope it shows the scope's models in their order,
    /// and Tab switches to all the providers' and back, keeping the query and the model selected.
    fn pick_model(&self, models: &[ModelOption], query: &str) -> Option<String> {
        let screen = self.screen.as_ref()?;
        let (scoped, _) = resolve(&self.scope, models);
        let scoped: Vec<&ModelOption> = (scoped.iter())
            .filter_map(|scoped| models.iter().find(|model| model.id == scoped.id))
            .collect();
        let all: Vec<&ModelOption> = models.iter().collect();
        let current = self.agent.model();
        let mut in_scope = !scoped.is_empty();
        let mut spot = Spot {
            query: query.to_owned(),
            ..Spot::default()
        };
        // The model selected when Tab was pressed, to stay selected in the other list.
        let mut selected: Option<String> = None;
        loop {
            let shown = if in_scope { &scoped } else { &all };
            let at = |id: &str| shown.iter().position(|model| model.id == id);
            let texts = (shown.iter())
                .map(|model| {
                    let provider = model.provider.as_deref().unwrap_or_default();
                    Haystack::new(&format!("{} {provider}", model.id))
                })
                .collect();
            let notes = (shown.iter())
                .map(|model| visible(model.name.as_deref().unwrap_or(&model.id)))
                .collect();
            let row = |index: usize, width| {
                let model = shown[index];
                let provider = (model.provider.as_deref()).map_or_else(String::new, |provider| {
                    format!(" {MUTED}[{}]{RESET}", visible(provider))
                });
                let rate = shown_rate(model.rate);
                clip(&format!("{}{provider}{rate}", visible(&model.id)), width)
            };
            let (title, keys) = match (scoped.is_empty(), in_scope) {
                (true, _) => (
                    "Select model".to_owned(),
                    select::hint("select", &[], "cancel"),
                ),
                (false, true) => {
                    let keys = select::hint("select", &[(Binding::Tab, "all models")], "cancel");
                    (scope_name("Models"), keys)
                }
                (false, false) => {
                    let scope = scope_name("models");
                    let keys = select::hint("select", &[(Binding::Tab, &scope)], "cancel");
                    ("All models".to_owned(), keys)
                }
            };
            let current_row = at(current);
            let initial = current_row.unwrap_or(0);
            let none = "No matching models";
            let picker = Picker::search(
                &title,
                select::VISIBLE,
                texts,
                initial,
                current_row,
                none,
                row,
            );
            let keep = match &selected {
                Some(id) => at(id).or(current_row),
                // A query starts at its best match.
                None if spot.query.is_empty() => current_row,
                None => None,
            };
            let mut picker = picker.noting(keys, notes).held().at(spot, None);
            if !scoped.is_empty() {
                picker = picker.tabbing();
            }
            if let Some(row) = keep {
                picker = picker.on_row(row);
            }
            match screen.pick_or_act(picker) {
                Picked::Tab(left) => {
                    let row = left.shown.get(left.position);
                    selected = row.map(|&row| shown[row].id.clone()).or(selected);
                    spot = Spot {
                        query: left.query,
                        ..Spot::default()
                    };
                    in_scope = !in_scope;
                }
                picked => {
                    screen.dismiss();
                    return match picked {
                        Picked::Row(Some(row)) => Some(shown[row].id.clone()),
                        _ => None,
                    };
                }
            }
        }
    }

    /// `/scoped-models`, as Scramjet's: the providers' models, those Ctrl+P cycles through first
    /// in its order, each switched on or off with Enter (with all on, the first leaves only it
    /// on), all shown with Ctrl+A and Ctrl+X, a provider's with Ctrl+P, moved with Alt+Up and
    /// Alt+Down. Changes apply to the session at once; Ctrl+S saves them as `enabledModels`.
    /// With piped input it says which they are.
    pub(super) fn scoped_models(&mut self) {
        let models = self.agent.model_options();
        let (scoped, _) = resolve(&self.scope, &models);
        let mut chosen = Chosen {
            all: models.iter().map(|model| model.id.clone()).collect(),
            on: (!scoped.is_empty()).then(|| scoped.into_iter().map(|s| s.id).collect()),
        };
        if !self.interactive() {
            let on = chosen
                .on
                .as_ref()
                .map_or("all".to_owned(), |on| on.join(", "));
            return self.say(&format!("{}: {on}", scope_name("Models")));
        }
        let model = |id: &str| models.iter().find(|model| model.id == id);
        let provider = |id: &str| model(id).and_then(|model| model.provider.clone());
        let mut unsaved = false;
        // Where the list was and the model selected, for it to show again so, and what to say.
        let mut place: Option<(Spot, String)> = None;
        let mut status = None;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let order = chosen.order();
            let texts = (order.iter().enumerate())
                .map(|(rank, id)| {
                    let provider = provider(id).unwrap_or_default();
                    Haystack::ranked(&format!("{id} {provider}"), rank)
                })
                .collect();
            let session_only = match keymap::key(Binding::ModelsSave) {
                _ if !unsaved => String::new(),
                Some(key) => format!(", for this session until {key} saves them"),
                None => ", for this session".to_owned(),
            };
            let acts = scoped_acts();
            let notes = (order.iter())
                .map(|id| {
                    let name = model(id).and_then(|model| model.name.clone());
                    let name = name.unwrap_or_else(|| id.clone());
                    format!("{name} · {}{session_only}\n{acts}", chosen.count())
                })
                .collect();
            let row = |index: usize, width| {
                let id = &order[index];
                let provider = (provider(id)).map_or_else(String::new, |provider| {
                    format!(" {MUTED}[{provider}]{RESET}")
                });
                let rate = shown_rate(model(id).and_then(|model| model.rate));
                let mark = match (&chosen.on, chosen.is_on(id)) {
                    (None, _) => String::new(),
                    (Some(_), true) => format!(" {MODEL_ON}✓{RESET}"),
                    (Some(_), false) => format!(" {MODEL_OFF}✗{RESET}"),
                };
                clip(&format!("{id}{provider}{rate}{mark}"), width)
            };
            let picker = Picker::search(
                &scope_name("Models"),
                select::VISIBLE,
                texts,
                0,
                None,
                "No matching models",
                row,
            );
            let mut picker = picker
                .noting(
                    select::hint("on/off", &[(Binding::ModelsSave, "save")], "close"),
                    notes,
                )
                .scoping()
                .held();
            if let Some((spot, id)) = place.take() {
                let at = order.iter().position(|known| *known == id).unwrap_or(0);
                picker = picker.at(spot, status.take()).on_row(at);
            }
            let (act, index, spot) = match screen.pick_or_act(picker) {
                Picked::Act(act, index, spot) => (act, index, spot),
                Picked::Row(_)
                | Picked::Tab(_)
                | Picked::Toggle(..)
                | Picked::Label(_)
                | Picked::Placed(..) => {
                    return screen.dismiss();
                }
            };
            let id = order[index].clone();
            let shown: Vec<String> = spot.shown.iter().map(|&at| order[at].clone()).collect();
            let changed = match act {
                Act::Toggle => {
                    chosen.toggle(&id);
                    true
                }
                Act::EnableAll => {
                    chosen.enable(&shown);
                    true
                }
                Act::ClearAll => {
                    chosen.clear(&shown);
                    true
                }
                Act::Provider => {
                    let theirs = provider(&id);
                    let ids: Vec<String> = (chosen.all.iter())
                        .filter(|other| provider(other) == theirs)
                        .cloned()
                        .collect();
                    chosen.provider(&ids);
                    true
                }
                Act::Earlier => chosen.shift(&id, -1),
                Act::Later => chosen.shift(&id, 1),
                Act::Save | Act::Rename | Act::Delete => false,
            };
            if changed {
                self.scope = chosen.scope();
                self.agent.set_model_scope(chosen.scope());
                unsaved = true;
            }
            if act == Act::Save {
                status = Some(match self.settings.set_enabled_models(chosen.scope()) {
                    Ok(()) => {
                        unsaved = false;
                        ("Model selection saved to settings".to_owned(), false)
                    }
                    Err(error) => (format!("Not saved: {error}"), true),
                });
            }
            place = Some((spot, id));
        }
    }
}

/// A model's output rate after its provider in a list, as Scramjet's selectors show it.
fn shown_rate(rate: Option<f64>) -> String {
    rate.map_or_else(String::new, |rate| {
        format!(" {MUTED}{}{RESET}", terminal::rate(rate))
    })
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
            window: None,
            output: None,
            images: true,
            rate: None,
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
    fn a_reference_names_a_model_by_provider_and_id_or_id_case_aside() {
        let models = proxy();
        let found = |reference| exact(reference, &models).map(|model| model.id.as_str());
        assert_eq!(
            found("anthropic/claude-haiku-5-5"),
            Some("claude-haiku-5-5")
        );
        assert_eq!(found(" OpenAI/GPT-6-Luna "), Some("gpt-6-luna"));
        assert_eq!(found("Local:7B"), Some("local:7b"));
        // Neither part of an id nor an empty reference.
        assert_eq!(found("haiku"), None);
        assert_eq!(found(""), None);
        assert_eq!(found("openai/claude-haiku-5-5"), None);
        // Several of the same id name none, as in Scramjet.
        let twice = [models[4].clone(), models[4].clone()];
        assert_eq!(exact("claude-haiku-5-5", &twice), None);
    }

    #[test]
    fn the_model_option_names_one_model_as_scramjets_does() {
        let models = proxy();
        let named = |pattern| named(pattern, &models).map(|scoped| (scoped.id, scoped.level));
        let at = |id: &str, level: Option<&str>| Some((id.to_owned(), level.map(str::to_owned)));
        assert_eq!(named("Claude-Opus-5-5"), at("claude-opus-5-5", None));
        assert_eq!(named("haiku"), at("claude-haiku-5-5", None));
        assert_eq!(named("Sonnet 5.5"), at("claude-sonnet-5-5", None));
        // A level after the last colon, Scramjet's `off` as requests' `none`; an id's own colon.
        assert_eq!(named("opus:high"), at("claude-opus-5-5", Some("high")));
        assert_eq!(named("luna:off"), at("gpt-6-luna", Some("none")));
        assert_eq!(named("local:7b"), at("local:7b", None));
        assert_eq!(named("local:7b:low"), at("local:7b", Some("low")));
        // A provider's prefix limits the models to its own, else it is part of the pattern.
        assert_eq!(named("openai/luna"), at("gpt-6-luna", None));
        assert_eq!(
            named("anthropic/5-5:max"),
            at("claude-sonnet-5-5", Some("max"))
        );
        assert_eq!(named("openai/opus"), None);
        // As Scramjet's strict --model, an unknown level, a glob or no match names none.
        assert_eq!(named("opus:hgih"), None);
        assert_eq!(named("claude-*"), None);
        assert_eq!(named("mistral"), None);
    }

    #[test]
    fn the_model_list_is_sorted_by_provider_with_scramjets_columns() {
        let card = |id: &str, provider, window, output, reasoning, images| ModelOption {
            window,
            output,
            reasoning,
            images,
            ..model(id, None, provider)
        };
        let models = [
            card(
                "gpt-6-luna",
                Some("openai"),
                Some(1_050_000),
                Some(128_000),
                true,
                true,
            ),
            card(
                "claude-haiku-4-5",
                Some("anthropic"),
                Some(200_000),
                Some(64_000),
                false,
                true,
            ),
            card(
                "deepseek-v4.1-flash",
                Some("deepseek"),
                Some(500_000),
                Some(32_768),
                true,
                false,
            ),
            card("bare", None, None, Some(999), true, true),
        ];
        assert_eq!(
            listing(&models, None),
            "provider   model                context  max-out  thinking  images\n\
             \x20          bare                 -        999      yes       yes\n\
             anthropic  claude-haiku-4-5     200K     64K      no        yes\n\
             deepseek   deepseek-v4.1-flash  500K     32.8K    yes       no\n\
             openai     gpt-6-luna           1.1M     128K     yes       yes\n"
        );
        let found = listing(&models, Some("openai luna"));
        assert_eq!(
            found.lines().skip(1).collect::<Vec<_>>(),
            ["openai    gpt-6-luna  1.1M     128K     yes       yes"]
        );
        assert_eq!(
            listing(&models, Some("zzz")),
            "No models matching \"zzz\"\n"
        );
        assert_eq!(listing(&[], None), "No provider offers any model\n");
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
    fn a_glob_takes_every_model_it_matches_in_the_providers_order_each_once() {
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

    fn chosen(on: Option<&[&str]>) -> Chosen {
        let strings = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        Chosen {
            all: strings(&["a", "b", "c", "d"]),
            on: on.map(strings),
        }
    }

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn the_list_switches_models_as_scramjets_does_and_keeps_those_on_first() {
        let mut list = chosen(None);
        assert_eq!(
            (list.order(), list.count(), list.scope()),
            (strings(&["a", "b", "c", "d"]), "all on".into(), vec![])
        );
        // With all on, the first switch leaves only that one on.
        list.toggle("c");
        assert_eq!(
            (list.order(), list.count(), list.scope()),
            (
                strings(&["c", "a", "b", "d"]),
                "1 of 4 on".into(),
                strings(&["c"])
            )
        );
        list.toggle("a");
        assert_eq!(list.order(), strings(&["c", "a", "b", "d"]));
        list.toggle("c");
        assert_eq!(list, chosen(Some(&["a"])));
        // Those shown, switched on after those on; all of them on is all on.
        list.enable(&strings(&["d", "a", "b"]));
        assert_eq!(list, chosen(Some(&["a", "d", "b"])));
        list.enable(&strings(&["c"]));
        assert_eq!(list, chosen(None));
        list.clear(&strings(&["b", "d"]));
        assert_eq!(list, chosen(Some(&["a", "c"])));
        list.clear(&strings(&["a", "b", "c", "d"]));
        // None on is no scope, as all on.
        assert_eq!((list.count(), list.scope()), ("0 of 4 on".into(), vec![]));
        // A provider's models go on, or off when all of them are on.
        list.provider(&strings(&["b", "c"]));
        assert_eq!(list, chosen(Some(&["b", "c"])));
        list.provider(&strings(&["b", "c"]));
        assert_eq!(list, chosen(Some(&[])));
        let mut all = chosen(None);
        all.provider(&strings(&["b", "c"]));
        assert_eq!(all, chosen(Some(&["a", "d"])));
    }

    #[test]
    fn alt_up_and_down_move_a_model_among_those_on() {
        let mut list = chosen(Some(&["a", "b", "c"]));
        assert!(list.shift("c", -1));
        assert_eq!(list, chosen(Some(&["a", "c", "b"])));
        assert!(list.shift("a", 1));
        assert_eq!(list, chosen(Some(&["c", "a", "b"])));
        // Not past either end, nor one off, nor with all on.
        assert!(!list.shift("c", -1) && !list.shift("b", 1) && !list.shift("d", -1));
        assert!(!chosen(None).shift("a", 1));
        assert_eq!(list, chosen(Some(&["c", "a", "b"])));
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
