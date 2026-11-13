//! The session every front end drives: the agent, the user's settings and the models Ctrl+P
//! cycles through, and what happens between turns (a completed command's next step), apart
//! from how each front end shows it (`Front`).
use super::settings::{Delivery, Settings};
use scramjet::agent::{
    Agent, AgentError, AgentUi, Completed, McpServer, Navigated, Next, Providers, Remembered,
    Reopened, Sandbox, SandboxMode, SessionInfo, Source, Step, Suggested, shortened,
};
use std::path::Path;
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

impl Session {
    /// `/reload`'s part in the session: the settings read again, with `servers` (a client's) over
    /// their MCP servers, and the shell, tools, command sets, skills, MCP servers, retries,
    /// compaction and providers taken from them.
    pub(super) fn reload(&mut self, servers: &[McpServer]) {
        self.settings.reload();
        with_servers(&mut self.settings, servers);
        let settings = &self.settings;
        scramjet::agent::set_shell(
            settings.shell_path.clone(),
            settings.shell_command_prefix.clone(),
        );
        scramjet::agent::set_user_shell(settings.default_shell);
        scramjet::agent::set_images(settings.resize_images, settings.block_images);
        self.agent.reload(
            settings.tools.clone(),
            settings.command_sets.clone(),
            settings.skills.clone(),
            settings.mcp_servers.clone(),
            settings.disabled_mcp_servers.clone(),
        );
        self.agent.set_retries(settings.retries());
        self.agent.set_compacting(settings.compacting);
        self.agent.set_providers(Providers::read(
            settings.models_files(),
            scramjet::agent::login::Logins::user(),
        ));
    }

    /// Makes `change` to the agent's sandbox for the rest of the session, as `/sandbox` does;
    /// returns what to say and the warnings, or why it was not made.
    pub(super) fn change_sandbox(
        &mut self,
        change: SandboxChange,
    ) -> Result<(String, Vec<String>), String> {
        let root = self.agent.root().to_owned();
        let current = self.agent.sandbox().cloned();
        match change {
            SandboxChange::Mode(None) => {
                self.agent.set_sandbox(None);
                let said = "No sandbox: the agent's tools run with your access";
                Ok((said.to_owned(), Vec::new()))
            }
            SandboxChange::Mode(Some(mode)) => {
                let mut warnings = Vec::new();
                let sandbox = match current {
                    Some(sandbox) => Sandbox { mode, ..sandbox },
                    None => {
                        Sandbox::check()?;
                        let mut sandbox = Sandbox {
                            mode,
                            network: self.settings.sandbox_network,
                            writable: Vec::new(),
                        };
                        warnings = sandbox.grant_remembered(&root);
                        sandbox
                    }
                };
                self.agent.set_sandbox(Some(sandbox));
                Ok((format!("Sandbox: {}", mode.name()), warnings))
            }
            SandboxChange::Network(network) => {
                let Some(sandbox) = current else {
                    return Err("No sandbox: commands reach the network as you can".to_owned());
                };
                self.agent.set_sandbox(Some(Sandbox { network, ..sandbox }));
                let said = match network {
                    true => "Sandbox: commands reach the network",
                    false => "Sandbox: no network",
                };
                Ok((said.to_owned(), Vec::new()))
            }
            SandboxChange::Add(directory) => {
                let Some(mut sandbox) = current else {
                    let said = "No sandbox: the agent can change every file you can already";
                    return Err(said.to_owned());
                };
                let path = sandbox.grant(&root, &directory)?;
                self.agent.set_sandbox(Some(sandbox));
                let said = format!("{} is writable for this session", path.display());
                Ok((said, Vec::new()))
            }
            SandboxChange::Remove(directory) => {
                let mut sandbox = current;
                let revoked =
                    (sandbox.as_mut()).and_then(|sandbox| sandbox.revoke(&root, &directory));
                let path = (revoked.clone()).unwrap_or_else(|| Sandbox::resolve(&root, &directory));
                let forgotten = match Remembered::user().map(|kept| kept.remove(&root, &path)) {
                    Some(Ok(forgotten)) => forgotten,
                    Some(Err(error)) => return Err(error),
                    None => false,
                };
                if revoked.is_some() {
                    self.agent.set_sandbox(sandbox);
                }
                let said = match (revoked.is_some(), forgotten) {
                    (true, true) => format!(
                        "{} is no longer granted in this project; read-only again",
                        path.display()
                    ),
                    (false, true) => {
                        format!("{} is no longer granted in this project", path.display())
                    }
                    (true, false) => format!("{} is read-only again", path.display()),
                    (false, false) => return Err(format!("{directory} was not granted")),
                };
                Ok((said, Vec::new()))
            }
        }
    }
}

/// A change of the session's sandbox, as `/sandbox` takes it.
#[derive(Debug)]
pub(super) enum SandboxChange {
    /// A mode, `None` for no sandbox.
    Mode(Option<SandboxMode>),
    Network(bool),
    /// A directory made writable too.
    Add(String),
    /// A directory's grant taken back, and forgotten if it was granted always in the project.
    Remove(String),
}

/// Puts `servers` in place of the settings' MCP servers of the same names, enabled.
pub(super) fn with_servers(settings: &mut Settings, servers: &[McpServer]) {
    let given = |name: &String| servers.iter().any(|server| &server.name == name);
    (settings.mcp_servers).retain(|server| !given(&server.name));
    (settings.disabled_mcp_servers).retain(|name| !given(name));
    settings.mcp_servers.extend(servers.iter().cloned());
}

/// Messages given while a turn runs, until the agent takes them: steering for the model's next
/// request, follow-ups for when the turn would end. ACP keeps each with its prompt's request.
pub(super) struct Messages<T = String> {
    pub(super) steering: Vec<T>,
    pub(super) follow_ups: Vec<T>,
}

impl<T> Default for Messages<T> {
    fn default() -> Self {
        Messages {
            steering: Vec::new(),
            follow_ups: Vec::new(),
        }
    }
}

impl<T> Messages<T> {
    pub(super) fn push(&mut self, message: T, follow_up: bool) {
        match follow_up {
            true => self.follow_ups.push(message),
            false => self.steering.push(message),
        }
    }
    /// The steering messages `delivery` gives the agent at once (`AgentUi::steering`).
    pub(super) fn steer(&mut self, delivery: Delivery) -> Vec<T> {
        deliver(&mut self.steering, delivery)
    }
    /// The follow-ups `delivery` gives the agent at once (`AgentUi::follow_up`).
    pub(super) fn follow_up(&mut self, delivery: Delivery) -> Vec<T> {
        deliver(&mut self.follow_ups, delivery)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_ups.is_empty()
    }
    /// All the messages, in the order they are sent, leaving none.
    pub(super) fn take(&mut self) -> Vec<T> {
        let mut all = std::mem::take(&mut self.steering);
        all.append(&mut self.follow_ups);
        all
    }
}

fn deliver<T>(queued: &mut Vec<T>, delivery: Delivery) -> Vec<T> {
    match delivery {
        Delivery::All => std::mem::take(queued),
        Delivery::OneAtATime if queued.is_empty() => Vec::new(),
        Delivery::OneAtATime => vec![queued.remove(0)],
    }
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
    /// The agent suggested them (`suggest_scramjet_next_steps`), outside a command.
    pub(super) suggestion: bool,
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
    /// Shows that a turn starts.
    fn turn_starts(&mut self);
    /// Runs `prompt` as a model turn, or with none goes on with the turn the conversation was
    /// left in (`Agent::resume_turn`), until it ends or pauses (`AgentError::Paused`) for what
    /// the user did meanwhile.
    fn turn(&mut self, prompt: Option<&str>) -> Result<(), AgentError>;
    /// Does what the user asked for while the turn ran, which paused it to go on after.
    fn paused(&mut self);
    /// Shows that the turn stopped, before what it stopped with.
    fn turn_stops(&mut self);
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
    /// Shows that the agent's session was replaced (a new, resumed or forked one, or a move in
    /// the tree), with `message`.
    fn session_changed(&mut self, message: &str, change: Change);
}

/// What a session change brought besides another conversation.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Change {
    /// The prompts typed so far stay those Up recalls: a new session, a fork, a clone, a move
    /// in the tree.
    Kept,
    /// Another session's prompts are those Up recalls: a resumed or imported one.
    Loaded,
    /// As `Loaded`, in another folder, whose themes, completions and problems now apply.
    Moved,
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
    front.turn_starts();
    let mut result = front.turn(prompt);
    while let Err(AgentError::Paused) = result {
        front.paused();
        result = front.turn(None);
    }
    front.turn_stops();
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
        front.say(&offered_here(&offer));
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

/// Names the steps of the next-step list `offer`, for a front that does not offer it again.
pub(super) fn offered_here(offer: &Reopened) -> String {
    let steps: Vec<_> = (offer.steps.iter().enumerate())
        .map(|(at, step)| {
            let recommended = match offer.recommended == Some(at) {
                true => " (recommended)",
                false => "",
            };
            format!("{}{}{recommended}", step.label(), step.fresh_mark())
        })
        .collect();
    format!("Next steps offered here: {}", steps.join(", "))
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
        suggestion: source == Source::Suggestion,
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
    if step.fresh_session && !new_session(front) {
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

/// Starts a new session (`/clear`, a next step's fresh session); returns whether it started.
pub(super) fn new_session(front: &mut dyn Front) -> bool {
    let agent = &mut front.session().agent;
    match agent.new_session() {
        Ok(()) => {
            let saved = super::interactive::saved_line(agent);
            front.session_changed(&format!("✓ New session started\n{saved}"), Change::Kept);
            true
        }
        Err(error) => {
            front.say(&format!("Error: {error}"));
            false
        }
    }
}

/// Continues in `session` (`/resume`); returns whether it did. As in Scramjet, a session
/// continues in its own folder; one whose folder is gone is forked into this project.
pub(super) fn resume(front: &mut dyn Front, session: &SessionInfo) -> bool {
    let agent = &mut front.session().agent;
    let path = &session.path;
    let root = (session.root.as_deref()).filter(|root| root.is_dir());
    let here = agent.session_here(path);
    let moved = root.filter(|root| *root != agent.root() || !here);
    let done = match (moved, here) {
        (Some(root), _) => (agent.resume_session_in(path, root))
            .map(|()| format!("Resumed session in {}", folder(session))),
        (None, true) => agent
            .resume_session(path)
            .map(|()| "Resumed session".to_owned()),
        (None, false) => agent.fork_session_file(path).map(|()| {
            let gone = match session.root {
                Some(_) => ", as that folder is gone",
                None => "",
            };
            format!(
                "Forked session from {} into this project{gone}",
                folder(session)
            )
        }),
    };
    let change = match moved {
        Some(_) => Change::Moved,
        None => Change::Loaded,
    };
    match done {
        Ok(done) => changed(front, &done, change),
        Err(error) => {
            front.say(&format!("Error: {error}"));
            return false;
        }
    }
    true
}

/// Continues in a copy of the session file at `path` (`/import`).
pub(super) fn import(front: &mut dyn Front, path: &str) {
    match front.session().agent.import_session(Path::new(path)) {
        Ok(()) => changed(
            front,
            &format!("Imported session from {path}"),
            Change::Loaded,
        ),
        Err(error) => front.say(&format!("Error: failed to import the session: {error}")),
    }
}

/// Continues in a new session holding the conversation before the prompt at `index` (`/fork`),
/// that prompt to edit and send again, or its next-step list offered again.
pub(super) fn fork(front: &mut dyn Front, index: usize) {
    match front.session().agent.fork(index) {
        Ok(Some((prompt, offer))) => {
            changed(front, "Forked to new session", Change::Kept);
            match offer {
                Some(offer) => reopen(front, offer, Some(prompt)),
                None => front.edit(prompt),
            }
        }
        Ok(None) => front.say(&format!(
            "Error: no message numbered {} (/fork lists them)",
            index + 1
        )),
        Err(error) => front.say(&format!("Error: {error}")),
    }
}

/// Continues in a new session holding the whole conversation (`/branch`); returns whether it
/// did.
pub(super) fn clone_session(front: &mut dyn Front) -> bool {
    match front.session().agent.clone_session() {
        Ok(true) => {
            changed(front, "Cloned to new session", Change::Kept);
            true
        }
        Ok(false) => {
            front.say("Nothing to clone yet");
            false
        }
        Err(error) => {
            front.say(&format!("Error: {error}"));
            false
        }
    }
}

/// Shows where `Agent::navigate` moved the conversation (`/tree`), then asks again the
/// question gone back to, offers again the list, or starts the next prompt with the prompt.
pub(super) fn navigated(front: &mut dyn Front, navigated: Navigated) {
    let Navigated::Moved {
        prompt,
        offer,
        question,
    } = navigated
    else {
        return front.say("Already at this point");
    };
    front.session_changed("✓ Navigated to selected point", Change::Kept);
    if question {
        return ask_again(front);
    }
    match (offer, prompt) {
        (Some(offer), prompt) => reopen(front, offer, prompt),
        (None, Some(prompt)) => front.edit(prompt),
        (None, None) => {}
    }
}

/// Shows the session `done` changed to, with its model and where it is saved.
pub(super) fn changed(front: &mut dyn Front, done: &str, change: Change) {
    let agent = &mut front.session().agent;
    agent.load_model_info();
    let message = format!(
        "✓ {done} · {}\n{}",
        super::interactive::model_line(agent),
        super::interactive::saved_line(agent),
    );
    front.session_changed(&message, change);
}

/// The folder of the project a session belongs to, `~` for the home directory's.
pub(super) fn folder(session: &SessionInfo) -> String {
    match &session.root {
        Some(root) => shortened(&root.display().to_string()),
        None => "an unknown folder".to_owned(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_messages_are_delivered_as_the_modes_say() {
        let mut messages = Messages::<String>::default();
        for text in ["one", "two"] {
            messages.push(text.into(), false);
        }
        assert_eq!(messages.steer(Delivery::OneAtATime), ["one"]);
        assert_eq!(messages.steer(Delivery::OneAtATime), ["two"]);
        assert!(messages.steer(Delivery::OneAtATime).is_empty());
        for text in ["one", "two"] {
            messages.push(text.into(), true);
        }
        assert_eq!(messages.follow_up(Delivery::All), ["one", "two"]);
        assert!(messages.is_empty());
        messages.push("later".into(), true);
        messages.push("steer".into(), false);
        assert_eq!(messages.take(), ["steer", "later"]);
        assert!(messages.is_empty());
    }
}
