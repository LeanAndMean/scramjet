//! The session every front end drives: the agent, the user's settings and the models Ctrl+P
//! cycles through, and what happens between turns (a completed command's next step), apart
//! from how each front end shows it (`Front`).
use super::settings::Settings;
use scramjet::agent::{
    Agent, AgentError, AgentUi, Completed, Next, Reopened, Source, Step, Suggested,
};
use std::time::Duration;

/// How long `/autopilot on` waits before it chooses the recommended next step, as Scramjet.
pub(super) const AUTOSELECT: Duration = Duration::from_secs(3);

pub(super) struct Session {
    pub(super) agent: Agent,
    pub(super) settings: Settings,
    /// The patterns of the models Ctrl+P cycles through (`scope::resolve`); empty: all.
    pub(super) scope: Vec<String>,
    /// A next step chosen between turns (`reopen`, `ask_again`), to run next as if typed.
    pub(super) chosen: Option<(String, Start)>,
}

/// How a prompt came to run.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Start {
    Typed,
    /// A next step the user chose from those offered.
    Chosen,
    /// A next step run at once (forced, an edge chains it, or `/autopilot on` without a list),
    /// or one `/autopilot on` chose when the user did not.
    Unattended,
}

/// Next steps for the user to choose from.
pub(super) struct Offer<'a> {
    pub(super) title: &'a str,
    pub(super) steps: &'a [Step],
    pub(super) recommended: Option<usize>,
    /// How long the user has before the recommended step is chosen for them (`/autopilot on`).
    pub(super) autoselect: Option<Duration>,
}

/// The step a front end's list chose, at `at`: `timed`, by the countdown.
pub(super) struct Picked {
    pub(super) at: usize,
    pub(super) timed: bool,
    pub(super) routed: Routed,
}

/// What a next step's list chose besides the step: `None` keeps the session's.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Routed {
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    /// Whether the chosen step starts a fresh session, when the list knows it.
    pub(super) fresh: Option<bool>,
}

/// What drives a `Session` and shows it: the full screen, line mode.
pub(super) trait Front {
    fn session(&mut self) -> &mut Session;
    /// Where the agent shows what it does.
    fn ui(&mut self) -> &mut dyn AgentUi;
    /// Runs `prompt` as a model turn, or with none goes on with the turn the conversation was
    /// left in (`Agent::resume_turn`), through any pauses for what the user typed meanwhile.
    fn turn(&mut self, prompt: Option<&str>) -> Result<(), AgentError>;
    /// Shows what a turn ended with, once its stop is shown.
    fn turn_ended(&mut self);
    /// Whether the user can choose from lists and answer questions as they show.
    fn chooses(&self) -> bool;
    /// Lets the user choose one of the steps offered; `None`: dismissed, or nobody to choose.
    fn offer(&mut self, offer: Offer) -> Option<Picked>;
    /// Starts the next prompt with `text`.
    fn edit(&mut self, text: String);
    /// Shows a message.
    fn say(&mut self, text: &str);
    /// Shows a message of less weight.
    fn note(&mut self, text: &str);
    /// Shows routing's notices before what it runs or offers.
    fn notices(&mut self, notices: &[String]);
    /// Shows that a next step was recorded (`Agent::record_next_step`).
    fn recorded(&mut self, record: &str);
    /// Starts a new session; false if it could not.
    fn new_session(&mut self) -> bool;
}

/// Runs `prompt` as a model turn, or with none goes on with the turn the conversation was left
/// in. Returns the next step to run, when the turn's command completed with one, or the user
/// chose one the agent suggested.
pub(super) fn run_turn(
    front: &mut dyn Front,
    prompt: Option<&str>,
    start: Start,
) -> Option<(String, Start)> {
    let Session {
        agent, settings, ..
    } = front.session();
    agent.set_hide_credentials(settings.hide_credentials.hides(start == Start::Unattended));
    agent.set_unattended(start == Start::Unattended);
    let model = agent.model().to_owned();
    let result = front.turn(prompt);
    // The agent's `switch_scramjet_model`, remembered as the user's choices are.
    if front.session().agent.model() != model {
        remember_model(front);
    }
    if let Err(error) = result {
        // As the session saved it, for a resume to show it so.
        let error = match error {
            AgentError::Cancelled => None,
            error => Some(error.to_string()),
        };
        front.ui().stopped(error.as_deref());
        if let Some(notice) = front.session().agent.take_interrupted() {
            front.ui().notice(&notice);
        }
    }
    front.turn_ended();
    let agent = &mut front.session().agent;
    match (agent.take_completed(), agent.take_suggestion()) {
        (Some(completed), _) => next_step(front, &completed),
        (None, Some(suggested)) => self::suggested(front, suggested),
        (None, None) => None,
    }
}

/// Asks again the question going back in the session reached, and goes on with its turn; the
/// next step it ends with runs next.
pub(super) fn ask_again(front: &mut dyn Front) {
    front.session().chosen = run_turn(front, None, Start::Typed);
}

/// Offers again the next-step list going back in the session reached, or names its steps where
/// nobody chooses from lists; the step chosen runs next, and `prompt`, the prompt gone back to,
/// starts the next prompt if none is chosen.
pub(super) fn reopen(front: &mut dyn Front, offer: Reopened, prompt: Option<String>) {
    if !front.chooses() {
        let steps: Vec<_> = (offer.steps.iter().enumerate())
            .map(|(at, step)| {
                let recommended = match offer.recommended == Some(at) {
                    true => " (recommended)",
                    false => "",
                };
                format!("{}{}{recommended}", step.label(), step.fresh_mark())
            })
            .collect();
        front.say(&format!("Next steps offered here: {}", steps.join(", ")));
    } else {
        let title = match offer.command {
            Some(_) => "Select next step",
            None => "Agent suggests a next step",
        };
        let (source, steps) = (offer.source(), &offer.steps);
        if let Some((step, start, routed)) =
            self::offer(front, title, source, steps, offer.recommended, None)
        {
            front.session().chosen = take_step(front, step, start, routed);
            return;
        }
    }
    if let Some(prompt) = prompt {
        front.edit(prompt);
    }
}

/// The next step of a completed command, as Scramjet routes it: an edge set to `chain` or a
/// forced step runs at once; otherwise the user chooses from the steps the model offered, and
/// with `/autopilot on` the recommended command is chosen unless the user acts within
/// `AUTOSELECT`. Without a list, `/autopilot on` runs the recommended command at once. Returns
/// the prompt to run; a text step goes into the editor instead.
pub(super) fn next_step(front: &mut dyn Front, completed: &Completed) -> Option<(String, Start)> {
    let chooses = front.chooses();
    let agent = &mut front.session().agent;
    let autopilot = agent.autopilot();
    let routing = agent.route(completed, chooses, autopilot);
    if routing.notices.is_empty() && routing.next == Next::Nothing {
        return None;
    }
    front.notices(&routing.notices);
    let source = Source::Completion(&routing.source);
    let (step, start, routed) = match routing.next {
        Next::Nothing => return None,
        Next::Run { steps, chosen } => {
            record(front, source, &steps, Some(chosen), Some(chosen));
            (steps[chosen].clone(), Start::Unattended, Routed::default())
        }
        Next::Offer {
            steps,
            recommended,
            autoselect,
        } => {
            let autoselect = autoselect.then_some(AUTOSELECT);
            let title = "Select next step";
            offer(front, title, source, &steps, recommended, autoselect)?
        }
    };
    take_step(front, step, start, routed)
}

/// The step the agent suggested that the user chooses, under Scramjet's title for them, from a
/// list that `/autopilot on` never chooses from by itself, as in Scramjet.
pub(super) fn suggested(front: &mut dyn Front, suggested: Suggested) -> Option<(String, Start)> {
    let (steps, recommended) = (&suggested.steps, suggested.recommended);
    let title = "Agent suggests a next step";
    let (step, start, routed) = offer(front, title, Source::Suggestion, steps, recommended, None)?;
    take_step(front, step, start, routed)
}

/// Lets the user choose one of `steps` from `source` under `title`, `recommended` selected at
/// first, and the model and effort it runs with; with `autoselect`, the recommended one is
/// chosen unless the user acts within it. Records the choice, or that the list was dismissed.
pub(super) fn offer(
    front: &mut dyn Front,
    title: &str,
    source: Source,
    steps: &[Step],
    recommended: Option<usize>,
    autoselect: Option<Duration>,
) -> Option<(Step, Start, Routed)> {
    let chosen = front.offer(Offer {
        title,
        steps,
        recommended,
        autoselect,
    });
    let mut steps = steps.to_vec();
    if let Some(picked) = &chosen
        && let Some(fresh) = picked.routed.fresh
    {
        steps[picked.at].fresh_session = fresh;
    }
    let at = chosen.as_ref().map(|picked| picked.at);
    record(front, source, &steps, recommended, at);
    let mut picked = chosen?;
    picked.routed.fresh = None;
    let start = match picked.timed {
        true => Start::Unattended,
        false => Start::Chosen,
    };
    Some((steps[picked.at].clone(), start, picked.routed))
}

/// Takes a next step with the model and effort `routed` chose: returns its command to run,
/// after starting a new session when it asks for one; a text step goes into the editor.
pub(super) fn take_step(
    front: &mut dyn Front,
    step: Step,
    start: Start,
    routed: Routed,
) -> Option<(String, Start)> {
    let Some(command) = step.command else {
        take_route(front, routed);
        front.edit(step.message);
        return None;
    };
    if step.fresh_session && !front.new_session() {
        return None;
    }
    take_route(front, routed);
    Some((command, start))
}

/// Sets the model and effort chosen for a next step, in the session that runs it; the model is
/// remembered as `/model` remembers one, and the effort saved as the settings' default, as
/// Shift+Tab saves it. A model that cannot be set leaves the step to run with the current one,
/// as in Scramjet.
fn take_route(front: &mut dyn Front, routed: Routed) {
    if routed == Routed::default() {
        return;
    }
    if let Some(model) = &routed.model {
        let agent = &mut front.session().agent;
        match agent.set_model(model) {
            Ok(()) => {
                agent.load_model_info();
                remember_model(front);
            }
            Err(error) => front.say(&format!("Error: model not set to {model}: {error}")),
        }
    }
    if let Some(level) = &routed.effort {
        let session = front.session();
        if let Err(error) = session.agent.set_effort(level) {
            front.say(&format!("Error: {error}"));
        } else if let Err(error) = session.settings.set_default_thinking_level(level) {
            front.say(&format!("Error: settings not saved: {error}"));
        }
    }
    let line = format!(
        "Next step runs with {}",
        super::interactive::model_line(&front.session().agent)
    );
    front.note(&line);
}

/// Remembers the session's model as the one chosen last, here and anywhere, which sessions
/// started later begin with (`Settings::remember_model`).
pub(super) fn remember_model(front: &mut dyn Front) {
    let Session {
        agent, settings, ..
    } = front.session();
    let (provider, id) = (agent.model_provider(), agent.model_id());
    if let Err(error) = settings.remember_model(agent.root(), provider, id) {
        front.say(&format!("Error: settings not saved: {error}"));
    }
}

/// Records and shows the next step chosen, or that the list was dismissed.
fn record(
    front: &mut dyn Front,
    source: Source,
    steps: &[Step],
    recommended: Option<usize>,
    chosen: Option<usize>,
) {
    let agent = &mut front.session().agent;
    match agent.record_next_step(source, steps, recommended, chosen) {
        Ok(record) => front.recorded(&record),
        Err(error) => front.say(&format!("Error: next step not recorded: {error}")),
    }
}
