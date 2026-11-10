//! scramjet-rs's own settings file, `$XDG_CONFIG_HOME/scramjet-rs/settings.json` (by default
//! under `~/.config`), apart from Scramjet's, in its format and with its key names where the
//! behavior matches.
use super::banner::Banner;
use super::design::Design;
use super::keymap::{self, Binding};
use super::packages::{self, Source};
use super::sessions::Chains;
use super::tree::Filter;
use super::{background, theme};
use scramjet_rs::agent::{
    CODING_TOOLS, Compacting, DEFAULT_TOOLS, McpServer, McpTransport, Origin, Retries, SandboxMode,
    Set, offline, sets_in, shortened, state_directory, write_replacing,
};
use serde_json::{Map, Value, json};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

/// Scramjet's thinking levels; its `off` is the `none` requests send.
pub(super) const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Scramjet's name for an effort level as requests send it.
pub(super) fn scramjet_level(level: &str) -> &str {
    if level == "none" { "off" } else { level }
}

/// The name requests send for one of Scramjet's thinking levels, or for their own `none`;
/// `None` for another word.
pub(super) fn card_level(level: &str) -> Option<&str> {
    match level {
        "off" | "none" => Some("none"),
        level => LEVELS.contains(&level).then_some(level),
    }
}

/// Scramjet's layout preferences the full screen follows as they change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Layout {
    /// `scrollWheelStep`: the lines a notch of the mouse wheel scrolls.
    pub(super) wheel_step: usize,
    /// `editorMaxHeightPercent`: the share of the screen's rows the prompt's text takes at most.
    pub(super) editor_share: usize,
    /// `autocompleteMaxVisible`: the most entries a completion list shows at once.
    pub(super) completion_rows: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            wheel_step: 3,
            editor_share: 30,
            completion_rows: 5,
        }
    }
}

/// Each layout key, its range as Scramjet keeps it, and the values `/settings` cycles through.
const LAYOUT_KEYS: [(Row, &str, u64, u64, &[usize]); 3] = [
    (
        Row::WheelStep,
        "scrollWheelStep",
        1,
        20,
        &[
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
        ],
    ),
    (
        Row::PromptHeight,
        "editorMaxHeightPercent",
        10,
        50,
        &[10, 15, 20, 25, 30, 35, 40, 45, 50],
    ),
    (
        Row::CompletionRows,
        "autocompleteMaxVisible",
        3,
        20,
        &[3, 5, 7, 10, 15, 20],
    ),
];

impl Layout {
    fn slot(&mut self, row: Row) -> &mut usize {
        match row {
            Row::WheelStep => &mut self.wheel_step,
            Row::PromptHeight => &mut self.editor_share,
            _ => &mut self.completion_rows,
        }
    }
}

/// What Escape twice on an empty prompt opens (`doubleEscapeAction`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DoubleEscape {
    Tree,
    Fork,
    Nothing,
}

/// When the agent's coding tools run with the credential stores hidden (`hideCredentials`, a
/// key of scramjet-rs's own).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum HideCredentials {
    /// In turns that no one started: a next step run at once, and `-p`.
    Unattended,
    Always,
    Never,
}

impl HideCredentials {
    /// Whether a turn hides them; `unattended`: it started without the user.
    pub(super) fn hides(self, unattended: bool) -> bool {
        match self {
            Self::Unattended => unattended,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// The model a launch starts a new session with (`startupModel`), as Scramjet's: the one last
/// chosen in the project, else anywhere, or the one last chosen anywhere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum StartupModel {
    PerDirectory,
    Global,
}

/// How a Markdown file the agent reads shows (`markdown.toolDisplay`), as Scramjet's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MarkdownReads {
    /// As a document.
    Rendered,
    Source,
}

/// How many queued messages go to the model at once (`steeringMode`, `followUpMode`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Delivery {
    All,
    OneAtATime,
}

/// A row of `/settings`, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Row {
    ThinkingLevel,
    StartupModel,
    ThinkingBlocks,
    MarkdownReads,
    Theme,
    Look,
    Banner,
    Steering,
    FollowUp,
    DoubleEscape,
    CopyOnSelect,
    WheelStep,
    RetainTranscript,
    Changelog,
    PromptHeight,
    CompletionRows,
    TerminalProgress,
    SpellCheck,
    AutoCompact,
    ResizeImages,
    BlockImages,
    BranchSummary,
    TreeFilter,
    Tools,
    Credentials,
    Autopilot,
    NextSteps,
    Publishing,
    Installed,
}

pub(super) const ROWS: [Row; 29] = [
    Row::ThinkingLevel,
    Row::StartupModel,
    Row::ThinkingBlocks,
    Row::MarkdownReads,
    Row::Theme,
    Row::Look,
    Row::Banner,
    Row::Steering,
    Row::FollowUp,
    Row::DoubleEscape,
    Row::CopyOnSelect,
    Row::WheelStep,
    Row::RetainTranscript,
    Row::Changelog,
    Row::PromptHeight,
    Row::CompletionRows,
    Row::TerminalProgress,
    Row::SpellCheck,
    Row::AutoCompact,
    Row::ResizeImages,
    Row::BlockImages,
    Row::BranchSummary,
    Row::TreeFilter,
    Row::Tools,
    Row::Credentials,
    Row::Autopilot,
    Row::NextSteps,
    Row::Publishing,
    Row::Installed,
];

impl Row {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ThinkingLevel => "Thinking level",
            Self::StartupModel => "Startup model",
            Self::ThinkingBlocks => "Thinking blocks",
            Self::MarkdownReads => "Markdown reads",
            Self::Theme => "Theme",
            Self::Look => "Look",
            Self::Banner => "Banner",
            Self::Steering => "Steering messages",
            Self::FollowUp => "Follow-up messages",
            Self::DoubleEscape => "Escape twice",
            Self::CopyOnSelect => "Copy on select",
            Self::WheelStep => "Wheel scrolls",
            Self::RetainTranscript => "Retain transcript",
            Self::Changelog => "Changelog",
            Self::PromptHeight => "Prompt height",
            Self::CompletionRows => "Completion rows",
            Self::TerminalProgress => "Terminal progress",
            Self::SpellCheck => "Spelling",
            Self::AutoCompact => "Auto-compact",
            Self::ResizeImages => "Resize images",
            Self::BlockImages => "Block images",
            Self::BranchSummary => "Leaving a branch",
            Self::TreeFilter => "Tree entries",
            Self::Tools => "Tools",
            Self::Credentials => "Credential stores",
            Self::Autopilot => "Autopilot",
            Self::NextSteps => "Next steps",
            Self::Publishing => "Publishing",
            Self::Installed => "Installed",
        }
    }

    /// What the row is about, shown under the list while it is selected.
    pub(super) fn description(self) -> String {
        // Its key and what it does, after the sentence that comes first: ` Ctrl+T toggles it.`
        let then = |binding, does: &str| {
            keymap::key(binding).map_or_else(String::new, |key| format!(" {key} {does}"))
        };
        let with = |binding, words: &str| {
            keymap::key(binding).map_or_else(String::new, |key| format!("{words}{key}"))
        };
        let text = match self {
            Self::ThinkingLevel => {
                let then = then(Binding::ThinkingCycle, "cycles it.");
                return format!(
                    "How much the model reasons; also the level new sessions start at.{then}"
                );
            }
            Self::StartupModel => {
                "The model a new session starts with when scramjet-rs starts, unless --model names \
                 one: the one last chosen in this project (else anywhere), or anywhere."
            }
            Self::ThinkingBlocks => {
                let then = then(Binding::ThinkingToggle, "toggles it.");
                return format!("The model's reasoning in the conversation.{then}");
            }
            Self::MarkdownReads => {
                let then = then(Binding::ToolsExpand, "expands either.");
                return format!(
                    "Markdown files the agent reads: shown as documents, or as their source.{then}"
                );
            }
            Self::Theme => "The colors of the conversation and of everything around it.",
            Self::Look => {
                "How the conversation shows prompts, reasoning, tool calls and replies: panels, \
                 on the theme's backgrounds (a theme with them, such as scramjet-dark), or \
                 classic, rails or margin."
            }
            Self::Banner => {
                let then = then(Binding::ToolsExpand, "shows every key and what was loaded.");
                return format!(
                    "What a session shows as it starts: header, as Scramjet's, or plain, card \
                     or logo, Scramjet's logo.{then}"
                );
            }
            Self::Steering => {
                let with = with(Binding::Submit, " with ");
                return format!(
                    "Sent{with} during a turn: how many the model gets before its next request."
                );
            }
            Self::FollowUp => {
                let with = with(Binding::FollowUp, " with ");
                return format!(
                    "Sent{with} during a turn: how many the model gets when it would stop."
                );
            }
            Self::DoubleEscape => {
                return match keymap::key(Binding::Interrupt) {
                    Some(key) => format!("{key} pressed twice on an empty prompt."),
                    None => "The interrupt key pressed twice on an empty prompt; it has none now."
                        .to_owned(),
                };
            }
            Self::CopyOnSelect => {
                let with = with(Binding::Copy, " or ");
                return format!(
                    "Text selected with the mouse is copied as soon as it is selected, ready to \
                     paste anywhere (Command+V on a Mac). A right click{with} copies a selection \
                     either way."
                );
            }
            Self::WheelStep => "The lines of the conversation a notch of the mouse wheel scrolls.",
            Self::RetainTranscript => {
                "On exit, the whole conversation, the prompt and the footer stay in the \
                 terminal's scrollback, as they were on the screen."
            }
            Self::Changelog => {
                "How What's new, at the first start after an update, and /changelog show each \
                 version: whole, or only its title line. d switches between them there."
            }
            Self::PromptHeight => {
                "The most rows the prompt's text takes, as a share of the screen's; a longer \
                 prompt scrolls within them."
            }
            Self::CompletionRows => {
                "The most entries a completion list under the prompt shows at once (commands, \
                 files, arguments)."
            }
            Self::TerminalProgress => {
                "While the agent works, the terminal's tab and taskbar show it busy, in terminals \
                 that show progress (OSC 9;4), such as Windows Terminal."
            }
            Self::SpellCheck => {
                return format!(
                    "Words in the prompt the dictionary lacks are underlined. Words of your own, \
                     one a line, go in {}, beside the settings file.",
                    super::spelling::WORDS
                );
            }
            Self::AutoCompact => {
                "The conversation's older part is summarized by itself as the context nears the \
                 model's window, and after a request exceeds it. /compact works either way."
            }
            Self::ResizeImages => {
                "Images the agent reads or a prompt attaches that are larger than 2000x2000 pixels \
                 or 4.5 MB are scaled down before they are sent; else they are sent as they are."
            }
            Self::BlockImages => {
                "No image goes to any model: each is replaced by a note saying image reading is \
                 disabled."
            }
            Self::BranchSummary => {
                "When /tree moves to another branch: ask whether to summarize the one left behind."
            }
            Self::TreeFilter => {
                let there = keymap::key(Binding::TreeFilterCycleForward)
                    .map_or_else(String::new, |key| {
                        format!("; there {key} cycles through the choices")
                    });
                return format!("The entries /tree shows when it opens{there}.");
            }
            Self::Tools => "The coding tools the agent has. Subagents have their own.",
            Self::Credentials => {
                "~/.ssh, ~/.netrc and other known stores, kept from the agent's tools: in turns no one \
                 started (a next step run at once, -p), always or never. Subagents never see them."
            }
            Self::Autopilot => {
                "On: when a command finishes, the next step it recommends runs after a 3-second \
                 countdown, unless you choose. For this session; /autopilot switches it too."
            }
            Self::NextSteps => {
                "What follows a command that finishes and recommends a next one: continue \
                 automatically, ask you, or let autopilot decide; command sets can suggest which. \
                 Kept in Scramjet's autonomy.yaml files, which Scramjet reads too."
            }
            Self::Publishing => {
                "Whether the issues, pull requests and comments a command publishes on GitHub wait \
                 for your approval, in the project's own repository; elsewhere they always do. \
                 Kept in Scramjet's autonomy.yaml for all projects, which Scramjet reads too."
            }
            Self::Installed => {
                "What this session loaded: command sets, commands, agents, skills, system prompt, \
                 context and settings files, where each comes from, and what is wrong there."
            }
        };
        text.to_owned()
    }
}

#[derive(Debug)]
pub(super) struct Settings {
    /// `None` without `XDG_CONFIG_HOME` and `HOME`: nothing is read or saved.
    path: Option<PathBuf>,
    pub(super) hide_thinking_block: bool,
    pub(super) double_escape: DoubleEscape,
    /// `copyOnSelect`: a completed mouse selection is copied; by default only on macOS, whose
    /// terminals keep Command+C to themselves.
    pub(super) copy_on_select: bool,
    /// `spellCheck`: misspelled words in the prompt are underlined.
    pub(super) spell_check: bool,
    /// `images.autoResize`: images too large to send are scaled down, as Scramjet's.
    pub(super) resize_images: bool,
    /// `images.blockImages`: no image is sent to any model, as Scramjet's.
    pub(super) block_images: bool,
    pub(super) layout: Layout,
    /// `retainTranscriptOnExit`: leaving the full screen for good writes the conversation into
    /// the terminal's scrollback, as Scramjet's.
    pub(super) retain_transcript: bool,
    /// `collapseChangelog`: What's new and `/changelog` show each version's title line first.
    pub(super) collapse_changelog: bool,
    /// `lastChangelogVersion`: the version What's new last showed, or that first started.
    pub(super) last_changelog_version: Option<String>,
    /// `terminal.showTerminalProgress`: the full screen tells the terminal it is busy while a
    /// turn runs (OSC 9;4), as Scramjet's.
    pub(super) terminal_progress: bool,
    /// `branchSummary.skipPrompt`: leaving a branch in `/tree` summarizes nothing, unasked.
    pub(super) skip_branch_summary_prompt: bool,
    /// `treeFilterMode`: the entries `/tree` shows when it opens.
    pub(super) tree_filter: Filter,
    /// `resumeChains`: how `/resume` shows chains of sessions, the form last chosen there.
    pub(super) resume_chains: Chains,
    pub(super) markdown_reads: MarkdownReads,
    /// `theme`: the name of the theme everything is shown in; `None`: one that follows the
    /// terminal's background (`Settings::theme_name`).
    pub(super) theme: Option<String>,
    /// `look`: how the full screen shows prompts, reasoning, calls and replies.
    pub(super) look: Design,
    /// `banner`: what the full screen shows as a session starts.
    pub(super) banner: Banner,
    /// `defaultThinkingLevel`, named as requests send levels.
    pub(super) default_thinking_level: Option<String>,
    /// `defaultProvider` and `defaultModel`: the model last chosen in any project, as Scramjet
    /// saves it; a `defaultModel` alone names a model as `--model` does.
    pub(super) default_provider: Option<String>,
    pub(super) default_model: Option<String>,
    pub(super) startup_model: StartupModel,
    /// The file of the model last chosen in each project, `last-used-models.json` in the state
    /// directory, as Scramjet keeps it apart from the settings; `None`: none is kept.
    recall: Option<PathBuf>,
    /// `tools`: the agent's coding tools, Scramjet's by default.
    pub(super) tools: Vec<String>,
    pub(super) hide_credentials: HideCredentials,
    /// `sandbox`: how the agent's tools are confined; `None`: not at all.
    pub(super) sandbox: Option<SandboxMode>,
    /// `sandboxNetwork`: whether commands in the sandbox reach the network.
    pub(super) sandbox_network: bool,
    pub(super) steering_mode: Delivery,
    pub(super) follow_up_mode: Delivery,
    /// The directories of the installed command sets: `commandSets`' (a relative one is
    /// relative to the settings file's directory), then those in the clones of `packages`.
    pub(super) command_sets: Vec<PathBuf>,
    /// `packages`: the git repositories installed, as Scramjet's `packages` lists them.
    pub(super) packages: Vec<Source>,
    /// `packages`' directories, as Pi takes a local path: each brings its sets and its `skills`
    /// and `themes` directories, as a clone does.
    pub(super) local_packages: Vec<PathBuf>,
    /// Where packages are cloned (`packages::clones`); `None`: nowhere.
    clones: Option<PathBuf>,
    /// Whether reading the file clones the packages missing, as a session's start does.
    cloning: bool,
    /// `skills`: skill files and directories besides those found by default, as `commandSets`'.
    pub(super) skills: Vec<PathBuf>,
    /// `themes`: theme files and directories besides those found by default, as `skills`.
    pub(super) themes: Vec<PathBuf>,
    /// `sessionDir`: where sessions are saved and found, as typed: as Scramjet's, a relative one
    /// is from the current directory, which `--session-dir` replaces.
    pub(super) session_dir: Option<String>,
    /// `shellPath`: the shell commands run with, as Scramjet's; `None`: bash.
    pub(super) shell_path: Option<PathBuf>,
    /// `shellCommandPrefix`: lines run before each command, as Scramjet's.
    pub(super) shell_command_prefix: Option<String>,
    /// `retry`: how a request that failed transiently is sent again.
    retry: Retries,
    /// `compaction` and `branchSummary.reserveTokens`.
    pub(super) compacting: Compacting,
    /// `--retries`: the number of retries for the run, over the settings' and kept at `/reload`.
    pub(super) retries_given: Option<u32>,
    /// `enabledModels`: the patterns of the models Ctrl+P cycles through (`scope::resolve`);
    /// empty: all.
    pub(super) enabled_models: Vec<String>,
    /// `mcpServers`: the MCP servers, by name.
    pub(super) mcp_servers: Vec<McpServer>,
    /// `disabledMcpServers`: the names of those not started, as Claude Code's (`/mcp disable`).
    pub(super) disabled_mcp_servers: Vec<String>,
    /// What is wrong in the file; those keys keep their defaults.
    pub(super) warnings: Vec<String>,
}

impl Settings {
    pub(super) fn load() -> Self {
        Self::loaded(false)
    }

    #[cfg(test)]
    fn read(path: Option<PathBuf>) -> Self {
        Self::read_with(path, None, false)
    }

    /// The settings as `load` reads them, cloning first the packages not cloned yet, as
    /// Scramjet's start does, unless `SCRAMJET_OFFLINE` is set. Git has the terminal, so this
    /// runs before the full screen takes it, and `/reload` only warns of a missing clone.
    pub(super) fn load_cloning() -> Self {
        Self::loaded(!offline())
    }

    fn loaded(cloning: bool) -> Self {
        let path = scramjet_rs::os::base_directory("XDG_CONFIG_HOME", ".config")
            .map(|config| config.join("scramjet-rs").join("settings.json"));
        let mut settings = Self::read_with(path, packages::clones(), cloning);
        settings.recall = (state_directory().ok()).map(|state| state.join("last-used-models.json"));
        settings
    }

    /// The settings in `path`, with packages cloned under `clones`, those missing cloned
    /// there first when `cloning`.
    fn read_with(path: Option<PathBuf>, clones: Option<PathBuf>, cloning: bool) -> Self {
        let mut settings = Self::defaults(path);
        settings.clones = clones;
        let Some(path) = &settings.path else {
            return settings;
        };
        settings.cloning = cloning;
        match read_object(path) {
            Ok(object) => settings.apply(&object),
            Err(error) => settings
                .warnings
                .push(format!("{error}; using the defaults")),
        }
        settings.cloning = false;
        settings
    }

    /// `/reload`: the file read again; one that cannot be read leaves the settings as they were,
    /// as Scramjet's reload does.
    pub(super) fn reload(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        match read_object(path) {
            Ok(object) => {
                let (recall, clones) = (self.recall.take(), self.clones.take());
                let retries_given = self.retries_given;
                *self = Self::defaults(self.path.take());
                (self.recall, self.clones) = (recall, clones);
                self.retries_given = retries_given;
                self.apply(&object);
            }
            Err(error) => self.warnings = vec![format!("{error}; kept as they were")],
        }
    }

    fn defaults(path: Option<PathBuf>) -> Self {
        Self {
            path,
            hide_thinking_block: false,
            double_escape: DoubleEscape::Tree,
            copy_on_select: cfg!(target_os = "macos"),
            spell_check: true,
            resize_images: true,
            block_images: false,
            layout: Layout::default(),
            retain_transcript: false,
            collapse_changelog: false,
            last_changelog_version: None,
            terminal_progress: false,
            skip_branch_summary_prompt: false,
            tree_filter: Filter::Default,
            resume_chains: Chains::Flat,
            markdown_reads: MarkdownReads::Rendered,
            theme: None,
            look: Design::Panels,
            banner: Banner::Header,
            default_thinking_level: None,
            default_provider: None,
            default_model: None,
            startup_model: StartupModel::PerDirectory,
            recall: None,
            tools: DEFAULT_TOOLS.map(str::to_owned).to_vec(),
            hide_credentials: HideCredentials::Unattended,
            sandbox: None,
            sandbox_network: true,
            steering_mode: Delivery::All,
            follow_up_mode: Delivery::OneAtATime,
            command_sets: Vec::new(),
            packages: Vec::new(),
            local_packages: Vec::new(),
            clones: None,
            cloning: false,
            skills: Vec::new(),
            themes: Vec::new(),
            session_dir: None,
            shell_path: None,
            shell_command_prefix: None,
            retry: Retries::default(),
            compacting: Compacting::default(),
            retries_given: None,
            enabled_models: Vec::new(),
            mcp_servers: Vec::new(),
            disabled_mcp_servers: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn apply(&mut self, object: &Map<String, Value>) {
        let path = shown(self.path.as_deref().unwrap_or(Path::new("")));
        let mut wrong = |key: &str, expected: &str| {
            self.warnings
                .push(format!("{path}: {key} must be {expected}; ignored"))
        };
        match object.get("hideThinkingBlock") {
            None => {}
            Some(Value::Bool(hide)) => self.hide_thinking_block = *hide,
            Some(_) => wrong("hideThinkingBlock", "true or false"),
        }
        match object.get("doubleEscapeAction").map(|value| value.as_str()) {
            None => {}
            Some(Some("tree")) => self.double_escape = DoubleEscape::Tree,
            Some(Some("fork")) => self.double_escape = DoubleEscape::Fork,
            Some(Some("none")) => self.double_escape = DoubleEscape::Nothing,
            Some(_) => wrong("doubleEscapeAction", r#""tree", "fork" or "none""#),
        }
        match object.get("copyOnSelect") {
            None => {}
            Some(Value::Bool(copy)) => self.copy_on_select = *copy,
            Some(_) => wrong("copyOnSelect", "true or false"),
        }
        match object.get("retainTranscriptOnExit") {
            None => {}
            Some(Value::Bool(retain)) => self.retain_transcript = *retain,
            Some(_) => wrong("retainTranscriptOnExit", "true or false"),
        }
        match object.get("collapseChangelog") {
            None => {}
            Some(Value::Bool(collapse)) => self.collapse_changelog = *collapse,
            Some(_) => wrong("collapseChangelog", "true or false"),
        }
        match object.get("lastChangelogVersion") {
            None => {}
            Some(Value::String(version)) => self.last_changelog_version = Some(version.clone()),
            Some(_) => wrong("lastChangelogVersion", "a version"),
        }
        match object.get("spellCheck") {
            None => {}
            Some(Value::Bool(check)) => self.spell_check = *check,
            Some(_) => wrong("spellCheck", "true or false"),
        }
        match object.get("images") {
            None => {}
            Some(Value::Object(images)) => {
                for key in ["autoResize", "blockImages"] {
                    match (images.get(key), key) {
                        (None, _) => {}
                        (Some(Value::Bool(on)), "autoResize") => self.resize_images = *on,
                        (Some(Value::Bool(on)), _) => self.block_images = *on,
                        (Some(_), _) => wrong(&format!("images.{key}"), "true or false"),
                    }
                }
            }
            Some(_) => wrong("images", "an object"),
        }
        // Scramjet's other `terminal` keys are for images, which are not shown here.
        match object.get("terminal") {
            None => {}
            Some(Value::Object(terminal)) => match terminal.get("showTerminalProgress") {
                None => {}
                Some(Value::Bool(on)) => self.terminal_progress = *on,
                Some(_) => wrong("terminal.showTerminalProgress", "true or false"),
            },
            Some(_) => wrong("terminal", "an object"),
        }
        // As Scramjet's, a number outside its range is taken as its nearest end.
        for (row, key, low, high, _) in LAYOUT_KEYS {
            match object.get(key).map(Value::as_f64) {
                None => {}
                Some(Some(number)) if number.is_finite() => {
                    *self.layout.slot(row) = number.floor().clamp(low as f64, high as f64) as usize
                }
                Some(_) => wrong(key, &format!("a number from {low} to {high}")),
            }
        }
        match object
            .get("branchSummary")
            .map(|value| value.get("skipPrompt"))
        {
            None | Some(None) => {}
            Some(Some(Value::Bool(skip))) => self.skip_branch_summary_prompt = *skip,
            Some(Some(_)) => wrong("branchSummary.skipPrompt", "true or false"),
        }
        match object
            .get("branchSummary")
            .map(|value| value.get("reserveTokens"))
        {
            None | Some(None) => {}
            Some(Some(value)) => match value.as_u64() {
                Some(reserve) => self.compacting.branch_reserve = reserve,
                None => wrong("branchSummary.reserveTokens", TOKENS),
            },
        }
        match object.get("compaction") {
            None => {}
            Some(Value::Object(compaction)) => {
                match compaction.get("enabled") {
                    None => {}
                    Some(Value::Bool(on)) => self.compacting.automatic = *on,
                    Some(_) => wrong("compaction.enabled", "true or false"),
                }
                for key in ["reserveTokens", "keepRecentTokens"] {
                    let Some(value) = compaction.get(key) else {
                        continue;
                    };
                    match (value.as_u64(), key) {
                        (Some(reserve), "reserveTokens") => self.compacting.reserve = reserve,
                        (Some(keep), _) => self.compacting.keep_recent = keep,
                        (None, _) => wrong(&format!("compaction.{key}"), TOKENS),
                    }
                }
            }
            Some(_) => wrong("compaction", "an object"),
        }
        match object.get("treeFilterMode").map(|value| value.as_str()) {
            None => {}
            Some(Some(name))
                if let Some(&filter) = Filter::CYCLE.iter().find(|f| f.setting() == name) =>
            {
                self.tree_filter = filter
            }
            Some(_) => wrong(
                "treeFilterMode",
                &Filter::CYCLE
                    .map(|f| format!("\"{}\"", f.setting()))
                    .join(", "),
            ),
        }
        match object.get("resumeChains").map(|value| value.as_str()) {
            None => {}
            Some(Some(name))
                if let Some(&chains) = Chains::CYCLE.iter().find(|c| c.setting() == name) =>
            {
                self.resume_chains = chains
            }
            Some(_) => wrong(
                "resumeChains",
                &Chains::CYCLE
                    .map(|c| format!("\"{}\"", c.setting()))
                    .join(", "),
            ),
        }
        match object.get("markdown").map(|value| value.get("toolDisplay")) {
            None | Some(None) => {}
            Some(Some(Value::String(mode))) if mode == "rendered" => {
                self.markdown_reads = MarkdownReads::Rendered
            }
            Some(Some(Value::String(mode))) if mode == "source" => {
                self.markdown_reads = MarkdownReads::Source
            }
            Some(Some(_)) => wrong("markdown.toolDisplay", r#""rendered" or "source""#),
        }
        match object.get("theme") {
            None => {}
            Some(Value::String(name)) if !name.trim().is_empty() => self.theme = Some(name.clone()),
            Some(_) => wrong("theme", "a theme's name"),
        }
        match object.get("look").map(|value| value.as_str()) {
            None => {}
            Some(Some(name)) if let Some(design) = Design::named(name) => self.look = design,
            Some(_) => wrong(
                "look",
                &Design::ALL.map(|d| format!("\"{}\"", d.name())).join(", "),
            ),
        }
        match object.get("banner").map(|value| value.as_str()) {
            None => {}
            Some(Some(name)) if let Some(banner) = Banner::named(name) => self.banner = banner,
            Some(_) => wrong(
                "banner",
                &Banner::ALL.map(|b| format!("\"{}\"", b.name())).join(", "),
            ),
        }
        match object
            .get("defaultThinkingLevel")
            .map(|value| value.as_str())
        {
            None => {}
            Some(Some("off")) => self.default_thinking_level = Some("none".to_owned()),
            Some(Some(level)) if LEVELS.contains(&level) => {
                self.default_thinking_level = Some(level.to_owned())
            }
            Some(_) => wrong(
                "defaultThinkingLevel",
                &LEVELS.map(|l| format!("\"{l}\"")).join(", "),
            ),
        }
        match object.get("defaultProvider") {
            None => {}
            Some(Value::String(provider)) if !provider.trim().is_empty() => {
                self.default_provider = Some(provider.clone())
            }
            Some(_) => wrong("defaultProvider", "a provider's name"),
        }
        match object.get("defaultModel") {
            None => {}
            Some(Value::String(model)) if !model.trim().is_empty() => {
                self.default_model = Some(model.clone())
            }
            Some(_) => wrong("defaultModel", "a model's id"),
        }
        match object.get("startupModel").map(|value| value.as_str()) {
            None => {}
            Some(Some("per-directory")) => self.startup_model = StartupModel::PerDirectory,
            Some(Some("global")) => self.startup_model = StartupModel::Global,
            Some(_) => wrong("startupModel", r#""per-directory" or "global""#),
        }
        match object.get("hideCredentials").map(|value| value.as_str()) {
            None => {}
            Some(Some("unattended")) => self.hide_credentials = HideCredentials::Unattended,
            Some(Some("always")) => self.hide_credentials = HideCredentials::Always,
            Some(Some("never")) => self.hide_credentials = HideCredentials::Never,
            Some(_) => wrong("hideCredentials", r#""unattended", "always" or "never""#),
        }
        match object.get("sandbox").map(|value| value.as_str()) {
            None => {}
            Some(Some(name)) if let Some(mode) = SandboxMode::parse(name) => self.sandbox = mode,
            Some(_) => wrong(
                "sandbox",
                r#""read-only", "workspace-write" or "danger-full-access""#,
            ),
        }
        match object.get("sandboxNetwork") {
            None => {}
            Some(Value::Bool(network)) => self.sandbox_network = *network,
            Some(_) => wrong("sandboxNetwork", "true or false"),
        }
        for (key, mode) in [
            ("steeringMode", &mut self.steering_mode),
            ("followUpMode", &mut self.follow_up_mode),
        ] {
            match object.get(key).map(|value| value.as_str()) {
                None => {}
                Some(Some("all")) => *mode = Delivery::All,
                Some(Some("one-at-a-time")) => *mode = Delivery::OneAtATime,
                Some(_) => wrong(key, r#""all" or "one-at-a-time""#),
            }
        }
        match object.get("tools") {
            None => {}
            Some(Value::Array(names)) => self.tools = self.read_tools(&path, names),
            Some(_) => wrong(
                "tools",
                &format!("a list of tool names ({})", CODING_TOOLS.join(", ")),
            ),
        }
        match command_sets(object) {
            Ok(entries) => {
                self.command_sets = (entries.iter()).map(|entry| self.resolve(entry)).collect()
            }
            Err(()) => self.warnings.push(format!(
                "{path}: commandSets must be a list of directories; ignored"
            )),
        }
        match paths(object, "packages") {
            Ok(entries) => {
                for entry in entries {
                    match Source::parse(&entry) {
                        Some(Ok(source)) => self.packages.push(source),
                        Some(Err(why)) => {
                            (self.warnings).push(format!("{path}: packages: {why}; ignored"))
                        }
                        None => self.local_packages.push(self.resolve(&entry)),
                    }
                }
            }
            Err(()) => self.warnings.push(format!(
                "{path}: packages must be a list of git repositories and directories; ignored"
            )),
        }
        match paths(object, "skills") {
            Ok(entries) => {
                self.skills = (entries.iter()).map(|entry| self.resolve(entry)).collect()
            }
            Err(()) => self.warnings.push(format!(
                "{path}: skills must be a list of files and directories; ignored"
            )),
        }
        match paths(object, "themes") {
            Ok(entries) => {
                self.themes = (entries.iter()).map(|entry| self.resolve(entry)).collect()
            }
            Err(()) => self.warnings.push(format!(
                "{path}: themes must be a list of files and directories; ignored"
            )),
        }
        self.add_packages();
        match object.get("sessionDir") {
            None => {}
            Some(Value::String(directory)) if !directory.trim().is_empty() => {
                self.session_dir = Some(directory.clone())
            }
            Some(_) => {
                (self.warnings).push(format!("{path}: sessionDir must be a directory; ignored"))
            }
        }
        match object.get("shellPath") {
            None => {}
            Some(Value::String(shell)) if !shell.trim().is_empty() => {
                self.shell_path = Some(self.resolve(shell))
            }
            Some(_) => {
                (self.warnings).push(format!("{path}: shellPath must be a program; ignored"))
            }
        }
        match object.get("shellCommandPrefix") {
            None => {}
            Some(Value::String(prefix)) => self.shell_command_prefix = Some(prefix.clone()),
            Some(_) => {
                (self.warnings).push(format!("{path}: shellCommandPrefix must be text; ignored"))
            }
        }
        match object.get("retry") {
            None => {}
            Some(Value::Object(retry)) => self.apply_retry(&path, retry),
            Some(_) => (self.warnings).push(format!("{path}: retry must be an object; ignored")),
        }
        match paths(object, "enabledModels") {
            Ok(patterns) => self.enabled_models = patterns,
            Err(()) => self.warnings.push(format!(
                "{path}: enabledModels must be a list of model patterns; ignored"
            )),
        }
        match object.get("mcpServers") {
            None => {}
            Some(Value::Object(servers)) => {
                for (name, entry) in servers {
                    match mcp_server(name, entry) {
                        Ok(server) => self.mcp_servers.push(server),
                        Err(why) => (self.warnings)
                            .push(format!("{path}: mcpServers: {name}: {why}; ignored")),
                    }
                }
            }
            Some(_) => self.warnings.push(format!(
                "{path}: mcpServers must be an object of servers by name; ignored"
            )),
        }
        match paths(object, "disabledMcpServers") {
            Ok(names) => self.disabled_mcp_servers = names,
            Err(()) => self.warnings.push(format!(
                "{path}: disabledMcpServers must be a list of server names; ignored"
            )),
        }
    }

    /// Scramjet's `retry` keys, each wrong one named and left at its default.
    fn apply_retry(&mut self, path: &str, retry: &Map<String, Value>) {
        const MAX_MS: u64 = 2_147_483_647;
        let mut wrong = Vec::new();
        let mut number = |object: &Map<String, Value>, key: &str, max: u64| {
            let value = object.get(key.rsplit('.').next().unwrap_or(key))?;
            let number = value.as_u64().filter(|n| *n <= max);
            if number.is_none() {
                wrong.push((key.to_owned(), format!("a whole number from 0 to {max}")));
            }
            number
        };
        if let Some(retries) = number(retry, "maxRetries", u64::from(u32::MAX)) {
            self.retry.attempts = retries as u32;
        }
        if let Some(ms) = number(retry, "baseDelayMs", MAX_MS) {
            self.retry.delay = Duration::from_millis(ms);
        }
        let provider = retry.get("provider");
        // Scramjet's older `retry.maxDelayMs`, which it moves to `provider.maxRetryDelayMs`.
        let max_wait = match provider.and_then(Value::as_object) {
            Some(provider) if provider.contains_key("maxRetryDelayMs") => {
                number(provider, "provider.maxRetryDelayMs", MAX_MS)
            }
            _ => number(retry, "maxDelayMs", MAX_MS),
        };
        if let Some(ms) = max_wait {
            self.retry.max_wait = (ms > 0).then(|| Duration::from_millis(ms));
        }
        if provider.is_some_and(|provider| !provider.is_object()) {
            wrong.push(("provider".to_owned(), "an object".to_owned()));
        }
        match retry.get("enabled") {
            None | Some(Value::Bool(true)) => {}
            Some(Value::Bool(false)) => self.retry.attempts = 0,
            Some(_) => wrong.push(("enabled".to_owned(), "true or false".to_owned())),
        }
        for (key, expected) in wrong {
            (self.warnings).push(format!("{path}: retry.{key} must be {expected}; ignored"));
        }
    }

    /// How the run's requests are retried: the settings' `retry`, with `--retries` over it.
    pub(super) fn retries(&self) -> Retries {
        match self.retries_given {
            Some(attempts) => Retries {
                attempts,
                ..self.retry
            },
            None => self.retry,
        }
    }

    /// A path in the settings, as Pi resolves one: `~` is the home directory, and a relative one
    /// is relative to the settings file's directory.
    fn resolve(&self, entry: &str) -> PathBuf {
        let entry = entry.trim();
        if let Some((rest, home)) = scramjet_rs::os::after_tilde(entry).zip(std::env::home_dir()) {
            return scramjet_rs::os::join(&home, rest);
        }
        let directory = self.path.as_deref().and_then(Path::parent);
        scramjet_rs::os::join(directory.unwrap_or(Path::new("/")), entry)
    }

    /// Installs `directory` by reference: a command set goes in `commandSets`; another directory
    /// holding sets (as a package's clone holds them, `sets_in`), a `skills` or a `themes`
    /// directory goes in `packages`, as Pi installs a local path, and its sets installed in
    /// `commandSets` before move into it. Refused when a set of one's name is installed from
    /// elsewhere. Each is written by its absolute path, with `~` for the home directory so that
    /// the settings serve another machine's home too.
    pub(super) fn install_directory(&mut self, directory: &Path) -> Result<Installed, String> {
        let directory = std::path::absolute(directory).map_err(|e| e.to_string())?;
        let why = match Set::at(&directory, Origin::Installed) {
            Ok(set) => return self.install_set(set).map(Installed::Set),
            Err(why) => why,
        };
        let held = sets_in(&directory);
        let (skills, themes) = extras(&directory);
        if held.is_empty() && skills.is_none() && themes.is_none() {
            return Err(match directory.is_dir() {
                true => format!(
                    "{why}, and holds no set (none in its top directories, nor one with a \
                     set.yaml deeper) and no skills or themes directory"
                ),
                false => why,
            });
        }
        if (self.local_packages.iter()).any(|known| same_directory(known, &directory)) {
            return Err(format!("{} is already installed", shown(&directory)));
        }
        let sets: Vec<Set> = (held.iter())
            .map(|set| Set::at(set, Origin::Installed))
            .collect::<Result<_, _>>()?;
        for (number, set) in sets.iter().enumerate() {
            if let Some(other) = sets[..number].iter().find(|other| other.name == set.name) {
                return Err(format!(
                    "{} holds two command sets named {}: {} and {}",
                    directory.display(),
                    set.name,
                    other.directory.display(),
                    set.directory.display()
                ));
            }
        }
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let entries = command_sets(&object).map_err(|()| {
            format!(
                "{}: commandSets must be a list of directories",
                shown(&path)
            )
        })?;
        let mut packages = paths(&object, "packages").map_err(|()| {
            format!(
                "{}: packages must be a list of git repositories and directories",
                shown(&path)
            )
        })?;
        let brought = |known: &Path| (held.iter()).any(|set| same_directory(known, set));
        let (moved, kept): (Vec<String>, Vec<String>) =
            (entries.into_iter()).partition(|entry| brought(&self.resolve(entry)));
        let moved: Vec<PathBuf> = moved.iter().map(|entry| self.resolve(entry)).collect();
        let others = (self.command_sets.iter()).filter(|known| !moved.contains(known));
        for set in &sets {
            let name = set.directory.file_name();
            if let Some(known) = others.clone().find(|known| known.file_name() == name) {
                return Err(format!(
                    "a command set named {} is installed from {}; remove it first",
                    set.name,
                    known.display()
                ));
            }
        }
        if !moved.is_empty() {
            object.insert("commandSets".into(), json!(kept));
        }
        packages.push(scramjet_rs::os::tilde(&directory));
        object.insert("packages".into(), json!(packages));
        write_object(&path, object)?;
        self.command_sets.retain(|known| !moved.contains(known));
        (self.command_sets).extend(sets.iter().map(|set| set.directory.clone()));
        self.skills.extend(skills.clone());
        self.themes.extend(themes.clone());
        self.local_packages.push(directory.clone());
        let moved = (sets.iter())
            .filter(|set| {
                moved
                    .iter()
                    .any(|known| same_directory(known, &set.directory))
            })
            .map(|set| set.name.clone())
            .collect();
        Ok(Installed::Package {
            directory,
            sets,
            moved,
            skills,
            themes,
        })
    }

    /// Adds `set` to `commandSets`.
    fn install_set(&mut self, set: Set) -> Result<Set, String> {
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let mut entries = command_sets(&object).map_err(|()| {
            format!(
                "{}: commandSets must be a list of directories",
                shown(&path)
            )
        })?;
        let known = (entries.iter())
            .map(|entry| self.resolve(entry))
            .chain(self.command_sets.iter().cloned())
            .find(|known| known.file_name() == set.directory.file_name());
        if let Some(known) = known {
            return Err(match same_directory(&known, &set.directory) {
                true => format!("{} is already installed", set.name),
                false => format!(
                    "a command set named {} is installed from {}; remove it first",
                    set.name,
                    known.display()
                ),
            });
        }
        entries.push(scramjet_rs::os::tilde(&set.directory));
        object.insert("commandSets".into(), json!(entries));
        write_object(&path, object)?;
        self.command_sets.push(set.directory.clone());
        Ok(set)
    }

    /// Adds to `command_sets` the sets in each package's clone and directory, and to `skills`
    /// and `themes` its `skills` and `themes` directories.
    fn add_packages(&mut self) {
        let mut loaded = Vec::new();
        for source in &self.packages {
            let Some(root) = &self.clones else {
                (self.warnings).push(format!(
                    "{}: not loaded, as neither XDG_DATA_HOME nor HOME is set",
                    source.written
                ));
                continue;
            };
            let directory = source.clone_directory(root);
            let mut failed = None;
            if self.cloning && !directory.exists() {
                eprintln!("Cloning {} into {}…", source.written, shown(&directory));
                failed = packages::clone(source, &directory, root).err();
            }
            match directory.is_dir() {
                false => self.warnings.push(format!(
                    "{}: not cloned into {}{}; `scramjet-rs update` clones it",
                    source.written,
                    shown(&directory),
                    failed.map(|why| format!(" ({why})")).unwrap_or_default()
                )),
                true => loaded.push((source.written.clone(), directory)),
            }
        }
        for directory in &self.local_packages {
            match directory.is_dir() {
                false => self.warnings.push(format!(
                    "{0}: not a directory; `scramjet-rs remove {0}` takes it out of packages",
                    shown(directory)
                )),
                true => loaded.push((shown(directory), directory.clone())),
            }
        }
        for (written, directory) in loaded {
            let sets = sets_in(&directory);
            let (skills, themes) = extras(&directory);
            if sets.is_empty() && skills.is_none() && themes.is_none() {
                self.warnings.push(format!(
                    "{written}: {} holds no command set, skills or themes",
                    shown(&directory)
                ));
            }
            self.command_sets.extend(sets);
            self.skills.extend(skills);
            self.themes.extend(themes);
        }
    }

    /// The package directory `reference` names, taken out of `packages` with what it brought.
    /// `None` when it names none.
    pub(super) fn remove_local_package(
        &mut self,
        reference: &str,
    ) -> Result<Option<PathBuf>, String> {
        let given = match scramjet_rs::os::after_tilde(reference).zip(std::env::home_dir()) {
            Some((rest, home)) => scramjet_rs::os::join(&home, rest),
            None => std::path::absolute(reference).map_err(|e| e.to_string())?,
        };
        let Some(directory) = (self.local_packages.iter())
            .find(|known| same_directory(known, &given))
            .cloned()
        else {
            return Ok(None);
        };
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let mut entries = paths(&object, "packages").map_err(|()| {
            format!(
                "{}: packages must be a list of git repositories and directories",
                shown(&path)
            )
        })?;
        entries.retain(|entry| {
            Source::parse(entry).is_some() || !same_directory(&self.resolve(entry), &directory)
        });
        object.insert("packages".into(), json!(entries));
        write_object(&path, object)?;
        self.local_packages.retain(|known| *known != directory);
        self.command_sets.retain(|set| !set.starts_with(&directory));
        let (skills, themes) = (directory.join("skills"), directory.join("themes"));
        self.skills.retain(|known| *known != skills);
        self.themes.retain(|known| *known != themes);
        Ok(Some(directory))
    }

    /// The package directory that brings `set`.
    pub(super) fn local_package(&self, set: &Path) -> Option<&Path> {
        (self.local_packages.iter())
            .map(PathBuf::as_path)
            .find(|directory| set.starts_with(directory))
    }

    /// Where packages are cloned.
    pub(super) fn clones(&self) -> Result<&Path, String> {
        (self.clones.as_deref()).ok_or_else(|| {
            "no directory for packages: neither XDG_DATA_HOME nor HOME is set".into()
        })
    }

    /// The installed package `source` names (in any of its forms, at any ref), or the one that
    /// brings the command set named `source`.
    pub(super) fn package(&self, source: &str) -> Option<&Source> {
        if let Some(Ok(source)) = Source::parse(source) {
            let identity = source.identity();
            return self
                .packages
                .iter()
                .find(|known| known.identity() == identity);
        }
        let root = self.clones.as_deref()?;
        let set = (self.command_sets.iter())
            .find(|set| set.file_name().is_some_and(|name| name == source))?;
        (self.packages.iter()).find(|known| set.starts_with(known.clone_directory(root)))
    }

    /// Adds `source`, cloned with its `sets`, to `packages`.
    pub(super) fn add_package(&mut self, source: Source, sets: Vec<PathBuf>) -> Result<(), String> {
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let mut entries = paths(&object, "packages").map_err(|()| {
            format!(
                "{}: packages must be a list of git repositories and directories",
                shown(&path)
            )
        })?;
        entries.push(source.written.clone());
        object.insert("packages".into(), json!(entries));
        write_object(&path, object)?;
        let (skills, themes) = extras(&source.clone_directory(self.clones()?));
        self.packages.push(source);
        self.command_sets.extend(sets);
        self.skills.extend(skills);
        self.themes.extend(themes);
        Ok(())
    }

    /// Removes `source`, as `package` found it, from `packages`. Returns its clone's directory,
    /// which is left for the caller to delete.
    pub(super) fn remove_package(&mut self, source: &Source) -> Result<PathBuf, String> {
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let directory = source.clone_directory(self.clones()?);
        let mut object = read_object(&path)?;
        let mut entries = paths(&object, "packages").map_err(|()| {
            format!(
                "{}: packages must be a list of git repositories and directories",
                shown(&path)
            )
        })?;
        let identity = source.identity();
        entries.retain(|entry| {
            !matches!(Source::parse(entry), Some(Ok(known)) if known.identity() == identity)
        });
        object.insert("packages".into(), json!(entries));
        write_object(&path, object)?;
        self.packages.retain(|known| known.identity() != identity);
        self.command_sets.retain(|set| !set.starts_with(&directory));
        self.skills.retain(|known| !known.starts_with(&directory));
        self.themes.retain(|known| !known.starts_with(&directory));
        Ok(directory)
    }

    /// Removes from `commandSets` the set named `reference`, or installed from that directory,
    /// else those installed from inside it. Returns the directories they were installed from.
    pub(super) fn remove_set(&mut self, reference: &str) -> Result<Vec<PathBuf>, String> {
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let mut entries = command_sets(&object).map_err(|()| {
            format!(
                "{}: commandSets must be a list of directories",
                shown(&path)
            )
        })?;
        // `~` too, which Windows' shells leave as typed.
        let given = match scramjet_rs::os::after_tilde(reference).zip(std::env::home_dir()) {
            Some((rest, home)) => Some(scramjet_rs::os::join(&home, rest)),
            None => std::path::absolute(reference).ok(),
        };
        let position = entries.iter().position(|entry| {
            let known = self.resolve(entry);
            known.file_name().is_some_and(|name| name == reference)
                || given
                    .as_deref()
                    .is_some_and(|given| same_directory(&known, given))
        });
        let removed: Vec<String> = match position {
            Some(position) => vec![entries.remove(position)],
            None => {
                let holder = given
                    .as_deref()
                    .and_then(|given| std::fs::canonicalize(given).ok());
                let held = |entry: &String| {
                    let known = std::fs::canonicalize(self.resolve(entry));
                    holder
                        .as_deref()
                        .zip(known.ok())
                        .is_some_and(|(holder, known)| known.starts_with(holder) && known != holder)
                };
                let (held, kept) = entries.into_iter().partition(held);
                entries = kept;
                held
            }
        };
        if removed.is_empty() {
            return Err(format!(
                "no installed command set is named {reference} or installed from there"
            ));
        }
        let removed: Vec<PathBuf> = removed.iter().map(|entry| self.resolve(entry)).collect();
        object.insert("commandSets".into(), json!(entries));
        write_object(&path, object)?;
        (self.command_sets).retain(|directory| !removed.contains(directory));
        Ok(removed)
    }

    pub(super) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The user's words for spelling, beside the settings file.
    pub(super) fn words(&self) -> Option<PathBuf> {
        Some(self.path.as_ref()?.with_file_name(super::spelling::WORDS))
    }

    /// The `models.json` files providers come from: the one beside the settings file, then each
    /// installed command set's.
    pub(super) fn models_files(&self) -> Vec<PathBuf> {
        let own = self.path.as_deref().and_then(Path::parent);
        let own = own.map(|directory| directory.join("models.json"));
        let sets = self.command_sets.iter().map(|set| set.join("models.json"));
        own.into_iter().chain(sets).collect()
    }

    /// The settings file, shortened for display.
    pub(super) fn shown_path(&self) -> Option<String> {
        self.path.as_deref().map(shown)
    }

    /// The coding tools `names` lists that there are.
    fn read_tools(&mut self, path: &str, names: &[Value]) -> Vec<String> {
        let mut tools: Vec<String> = Vec::new();
        for name in names {
            match name.as_str() {
                Some(tool) if CODING_TOOLS.contains(&tool) => {
                    if !tools.iter().any(|known| known == tool) {
                        tools.push(tool.to_owned());
                    }
                }
                _ => self.warnings.push(format!(
                    "{path}: tools: {name} is not one of {}; ignored",
                    CODING_TOOLS.join(", ")
                )),
            }
        }
        tools
    }

    /// Ctrl+T's choice, saved when it changed.
    pub(super) fn set_hide_thinking_block(&mut self, hide: bool) -> io::Result<()> {
        if self.hide_thinking_block == hide {
            return Ok(());
        }
        self.hide_thinking_block = hide;
        self.save("hideThinkingBlock", json!(hide))
    }

    /// The version What's new showed last, saved when it changed.
    pub(super) fn set_last_changelog_version(&mut self, version: &str) -> io::Result<()> {
        if self.last_changelog_version.as_deref() == Some(version) {
            return Ok(());
        }
        self.last_changelog_version = Some(version.to_owned());
        self.save("lastChangelogVersion", json!(version))
    }

    /// The form `/resume` showed chains in last, saved when it changed.
    pub(super) fn set_resume_chains(&mut self, chains: Chains) -> io::Result<()> {
        if self.resume_chains == chains {
            return Ok(());
        }
        self.resume_chains = chains;
        self.save("resumeChains", json!(chains.setting()))
    }

    /// The name of the theme shown: the setting's, else the one that follows the terminal's
    /// background as it answered or the environment says.
    pub(super) fn theme_name(&self) -> &str {
        (self.theme.as_deref()).unwrap_or_else(|| theme::following(background::terminal()))
    }

    /// The theme in `/settings`, saved when it changed.
    pub(super) fn set_theme(&mut self, name: &str) -> io::Result<()> {
        if self.theme.as_deref() == Some(name) {
            return Ok(());
        }
        self.theme = Some(name.to_owned());
        self.save("theme", json!(name))
    }

    /// A reasoning effort chosen with Shift+Tab, saved when it changed.
    pub(super) fn set_default_thinking_level(&mut self, level: &str) -> io::Result<()> {
        if self.default_thinking_level.as_deref() == Some(level) {
            return Ok(());
        }
        self.default_thinking_level = Some(level.to_owned());
        self.save("defaultThinkingLevel", json!(scramjet_level(level)))
    }

    /// The models chosen before that a launch in the project `root` may start a new session
    /// with, as Scramjet's, by reference (`provider/id`): the one last chosen there (with
    /// `startupModel` `per-directory`), and `defaultProvider` and `defaultModel`.
    pub(super) fn remembered_models(&self, root: &Path) -> (Option<String>, Option<String>) {
        let here = match (self.startup_model, &self.recall) {
            (StartupModel::PerDirectory, Some(recall)) => (recalled(recall))
                .remove(&*root.to_string_lossy())
                .and_then(|model| model.as_str().map(str::to_owned)),
            _ => None,
        };
        let anywhere = (self.default_model.as_ref()).map(|model| match &self.default_provider {
            Some(provider) => format!("{provider}/{model}"),
            None => model.clone(),
        });
        (here, anywhere)
    }

    /// The model `id` of `provider` (`None` for a model no provider offers), chosen during a
    /// session in the project `root`, remembered as Scramjet remembers one: as `defaultProvider`
    /// and `defaultModel`, and as the one last chosen there. Saves what changed.
    pub(super) fn remember_model(
        &mut self,
        root: &Path,
        provider: Option<&str>,
        id: &str,
    ) -> io::Result<()> {
        let reference = provider.map_or_else(|| id.to_owned(), |p| format!("{p}/{id}"));
        let recalled = self.recall_model(root, &reference);
        let same = self.default_provider.as_deref() == provider
            && self.default_model.as_deref() == Some(id);
        let saved = match same {
            true => Ok(()),
            false => {
                self.default_provider = provider.map(str::to_owned);
                self.default_model = Some(id.to_owned());
                self.update(|object| {
                    match provider {
                        Some(provider) => object.insert("defaultProvider".into(), json!(provider)),
                        None => object.remove("defaultProvider"),
                    };
                    object.insert("defaultModel".into(), json!(id));
                })
            }
        };
        recalled.and(saved)
    }

    fn recall_model(&self, root: &Path, model: &str) -> io::Result<()> {
        let Some(path) = &self.recall else {
            return Ok(());
        };
        let key = root.to_string_lossy();
        if recalled(path).get(&*key).and_then(Value::as_str) == Some(model) {
            return Ok(());
        }
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        // Another session's choice between the read and the rename would be lost without it.
        let lock = File::create(path.with_extension("lock"))?;
        lock.lock()?;
        let mut models = recalled(path);
        models.insert(key.into_owned(), json!(model));
        let text = serde_json::to_string_pretty(&models).map_err(io::Error::other)?;
        write_replacing(path, &(text + "\n"))
    }

    /// `/scoped-models`' choice: the models Ctrl+P cycles through, in order; none: all of them,
    /// which leaves `enabledModels` out, as Scramjet saves it.
    pub(super) fn set_enabled_models(&mut self, models: Vec<String>) -> io::Result<()> {
        self.enabled_models = models.clone();
        self.update(|object| match models.is_empty() {
            true => drop(object.remove("enabledModels")),
            false => drop(object.insert("enabledModels".into(), json!(models))),
        })
    }

    /// `/mcp enable` and `disable`'s choice: the MCP servers not started; none leaves
    /// `disabledMcpServers` out.
    pub(super) fn set_disabled_mcp_servers(&mut self, names: Vec<String>) -> io::Result<()> {
        self.disabled_mcp_servers = names.clone();
        self.update(|object| match names.is_empty() {
            true => drop(object.remove("disabledMcpServers")),
            false => drop(object.insert("disabledMcpServers".into(), json!(names))),
        })
    }

    /// The value `row` shows in `/settings`; the thinking level, autopilot, the next steps,
    /// publishing and what is installed are the session's, not the settings'.
    pub(super) fn shown(&self, row: Row) -> String {
        let delivery = |mode| match mode {
            Delivery::All => "all at once",
            Delivery::OneAtATime => "one at a time",
        };
        match row {
            Row::ThinkingLevel
            | Row::Autopilot
            | Row::NextSteps
            | Row::Publishing
            | Row::Installed => String::new(),
            Row::Theme => self.theme_name().to_owned(),
            Row::Look => self.look.name().into(),
            Row::Banner => self.banner.name().into(),
            Row::StartupModel => match self.startup_model {
                StartupModel::PerDirectory => "last chosen here",
                StartupModel::Global => "last chosen anywhere",
            }
            .into(),
            Row::ThinkingBlocks if self.hide_thinking_block => "hidden".into(),
            Row::ThinkingBlocks => "shown".into(),
            Row::MarkdownReads => match self.markdown_reads {
                MarkdownReads::Rendered => "rendered",
                MarkdownReads::Source => "source",
            }
            .into(),
            Row::Steering => delivery(self.steering_mode).into(),
            Row::FollowUp => delivery(self.follow_up_mode).into(),
            Row::DoubleEscape => match self.double_escape {
                DoubleEscape::Tree => "opens /tree",
                DoubleEscape::Fork => "opens /fork",
                DoubleEscape::Nothing => "does nothing",
            }
            .into(),
            Row::CopyOnSelect if self.copy_on_select => "on".into(),
            Row::CopyOnSelect => "off".into(),
            Row::WheelStep => match self.layout.wheel_step {
                1 => "1 line".into(),
                lines => format!("{lines} lines"),
            },
            Row::PromptHeight => format!("{}% of the screen", self.layout.editor_share),
            Row::CompletionRows => self.layout.completion_rows.to_string(),
            Row::RetainTranscript if self.retain_transcript => "on".into(),
            Row::RetainTranscript => "off".into(),
            Row::Changelog if self.collapse_changelog => "titles".into(),
            Row::Changelog => "whole".into(),
            Row::TerminalProgress if self.terminal_progress => "on".into(),
            Row::TerminalProgress => "off".into(),
            Row::SpellCheck if self.spell_check => "on".into(),
            Row::SpellCheck => "off".into(),
            Row::AutoCompact if self.compacting.automatic => "on".into(),
            Row::AutoCompact => "off".into(),
            Row::ResizeImages if self.resize_images => "on".into(),
            Row::ResizeImages => "off".into(),
            Row::BlockImages if self.block_images => "on".into(),
            Row::BlockImages => "off".into(),
            Row::BranchSummary if self.skip_branch_summary_prompt => "no summary".into(),
            Row::BranchSummary => "asks for a summary".into(),
            Row::TreeFilter => match self.tree_filter {
                Filter::Default => "all but model changes",
                Filter::NoTools => "no tool results",
                Filter::UserOnly => "prompts only",
                Filter::Labeled => "labeled only",
                Filter::All => "everything",
            }
            .into(),
            Row::Tools if self.tools.is_empty() => "none".into(),
            Row::Tools => (CODING_TOOLS.iter())
                .filter(|tool| self.tools.iter().any(|known| known == *tool))
                .copied()
                .collect::<Vec<_>>()
                .join(", "),
            Row::Credentials => match self.hide_credentials {
                HideCredentials::Unattended => "hidden when unattended",
                HideCredentials::Always => "always hidden",
                HideCredentials::Never => "never hidden",
            }
            .into(),
        }
    }

    /// Moves `row` to its next value, as Enter in `/settings` does, and saves it. Those that are
    /// not in the file, and the tools, which have a list of their own, stay.
    pub(super) fn cycle(&mut self, row: Row) -> io::Result<()> {
        let next = |mode| match mode {
            Delivery::All => Delivery::OneAtATime,
            Delivery::OneAtATime => Delivery::All,
        };
        let delivery = |mode| match mode {
            Delivery::All => "all",
            Delivery::OneAtATime => "one-at-a-time",
        };
        match row {
            Row::ThinkingLevel
            | Row::Tools
            | Row::Theme
            | Row::Autopilot
            | Row::NextSteps
            | Row::Publishing
            | Row::Installed => Ok(()),
            Row::StartupModel => {
                let (startup, name) = match self.startup_model {
                    StartupModel::PerDirectory => (StartupModel::Global, "global"),
                    StartupModel::Global => (StartupModel::PerDirectory, "per-directory"),
                };
                self.startup_model = startup;
                self.save("startupModel", json!(name))
            }
            Row::ThinkingBlocks => self.set_hide_thinking_block(!self.hide_thinking_block),
            Row::Look => {
                self.look = self.look.next();
                self.save("look", json!(self.look.name()))
            }
            Row::Banner => {
                self.banner = self.banner.next();
                self.save("banner", json!(self.banner.name()))
            }
            Row::MarkdownReads => {
                let (reads, name) = match self.markdown_reads {
                    MarkdownReads::Rendered => (MarkdownReads::Source, "source"),
                    MarkdownReads::Source => (MarkdownReads::Rendered, "rendered"),
                };
                self.markdown_reads = reads;
                self.save_in("markdown", "toolDisplay", json!(name))
            }
            Row::Steering => {
                self.steering_mode = next(self.steering_mode);
                self.save("steeringMode", json!(delivery(self.steering_mode)))
            }
            Row::FollowUp => {
                self.follow_up_mode = next(self.follow_up_mode);
                self.save("followUpMode", json!(delivery(self.follow_up_mode)))
            }
            Row::DoubleEscape => {
                let (action, name) = match self.double_escape {
                    DoubleEscape::Tree => (DoubleEscape::Fork, "fork"),
                    DoubleEscape::Fork => (DoubleEscape::Nothing, "none"),
                    DoubleEscape::Nothing => (DoubleEscape::Tree, "tree"),
                };
                self.double_escape = action;
                self.save("doubleEscapeAction", json!(name))
            }
            Row::CopyOnSelect => {
                self.copy_on_select = !self.copy_on_select;
                self.save("copyOnSelect", json!(self.copy_on_select))
            }
            Row::RetainTranscript => {
                self.retain_transcript = !self.retain_transcript;
                self.save("retainTranscriptOnExit", json!(self.retain_transcript))
            }
            Row::Changelog => {
                self.collapse_changelog = !self.collapse_changelog;
                self.save("collapseChangelog", json!(self.collapse_changelog))
            }
            Row::TerminalProgress => {
                self.terminal_progress = !self.terminal_progress;
                self.save_in(
                    "terminal",
                    "showTerminalProgress",
                    json!(self.terminal_progress),
                )
            }
            Row::SpellCheck => {
                self.spell_check = !self.spell_check;
                self.save("spellCheck", json!(self.spell_check))
            }
            Row::ResizeImages => {
                self.resize_images = !self.resize_images;
                self.save_in("images", "autoResize", json!(self.resize_images))
            }
            Row::BlockImages => {
                self.block_images = !self.block_images;
                self.save_in("images", "blockImages", json!(self.block_images))
            }
            Row::WheelStep | Row::PromptHeight | Row::CompletionRows => {
                let Some((_, key, _, _, values)) =
                    LAYOUT_KEYS.into_iter().find(|entry| entry.0 == row)
                else {
                    return Ok(());
                };
                let slot = self.layout.slot(row);
                *slot = (values.iter().copied())
                    .find(|value| value > slot)
                    .unwrap_or(values[0]);
                let value = *slot;
                self.save(key, json!(value))
            }
            Row::AutoCompact => {
                self.compacting.automatic = !self.compacting.automatic;
                self.save_in("compaction", "enabled", json!(self.compacting.automatic))
            }
            Row::BranchSummary => {
                self.skip_branch_summary_prompt = !self.skip_branch_summary_prompt;
                let skip = self.skip_branch_summary_prompt;
                self.save_in("branchSummary", "skipPrompt", json!(skip))
            }
            Row::TreeFilter => {
                let at = Filter::CYCLE.iter().position(|&f| f == self.tree_filter);
                let next = (at.unwrap_or(0) + 1) % Filter::CYCLE.len();
                self.tree_filter = Filter::CYCLE[next];
                self.save("treeFilterMode", json!(self.tree_filter.setting()))
            }
            Row::Credentials => {
                let (hide, name) = match self.hide_credentials {
                    HideCredentials::Unattended => (HideCredentials::Always, "always"),
                    HideCredentials::Always => (HideCredentials::Never, "never"),
                    HideCredentials::Never => (HideCredentials::Unattended, "unattended"),
                };
                self.hide_credentials = hide;
                self.save("hideCredentials", json!(name))
            }
        }
    }

    /// The agent's coding tools, chosen in `/settings`, saved in `CODING_TOOLS`' order.
    pub(super) fn set_tools(&mut self, tools: Vec<String>) -> io::Result<()> {
        let tools: Vec<String> = (CODING_TOOLS.iter())
            .filter(|tool| tools.iter().any(|chosen| chosen == *tool))
            .map(|tool| tool.to_string())
            .collect();
        let saved = json!(tools);
        self.tools = tools;
        self.save("tools", saved)
    }

    /// Sets `key` in the file as it is now, keeping the rest of it; a file that cannot be read
    /// is left as it is.
    fn save(&self, key: &str, value: Value) -> io::Result<()> {
        self.update(|object| {
            object.insert(key.to_owned(), value);
        })
    }

    /// Sets `key` in the object `parent`, keeping its other keys.
    fn save_in(&self, parent: &str, key: &str, value: Value) -> io::Result<()> {
        self.update(|object| {
            let entry = object.entry(parent).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            if let Value::Object(inner) = entry {
                inner.insert(key.to_owned(), value);
            }
        })
    }

    fn update(&self, change: impl FnOnce(&mut Map<String, Value>)) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut object = read_object(path).map_err(io::Error::other)?;
        change(&mut object);
        write_object(path, object).map_err(io::Error::other)
    }
}

/// The models last chosen in each project that the file `path` keeps; none when it cannot be
/// read, and the next choice replaces it, as they only choose a session's first model.
fn recalled(path: &Path) -> Map<String, Value> {
    let text = std::fs::read(path).ok();
    text.and_then(|text| serde_json::from_slice(&text).ok())
        .unwrap_or_default()
}

/// An entry of `mcpServers`, as Claude Code's: `command`, with `args` and `env`, or for a server
/// over streamable HTTP (`type` `http`), `url` with `headers`. Their `${VAR}` and `${VAR:-default}`
/// are the environment's variables, as Claude Code expands them.
fn mcp_server(name: &str, entry: &Value) -> Result<McpServer, String> {
    let Value::Object(entry) = entry else {
        return Err("must be an object with a command or a url".to_owned());
    };
    if name.trim().is_empty() {
        return Err("a server needs a name".to_owned());
    }
    let http = match entry.get("type").map(Value::as_str) {
        None => entry.contains_key("url") && !entry.contains_key("command"),
        Some(Some("stdio")) => false,
        Some(Some("http" | "streamable-http")) => true,
        Some(Some("sse")) => {
            return Err(r#"the older HTTP with SSE transport is not supported; give the server's streamable HTTP address with "type": "http""#.to_owned());
        }
        Some(_) => return Err(r#"type must be "stdio" or "http""#.to_owned()),
    };
    let strings = |key: &str| match entry.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Object(values)) => (values.iter())
            .map(|(name, value)| match value.as_str() {
                Some(value) => Ok((name.clone(), expanded(value)?)),
                None => Err(format!("the values of {key} must be strings")),
            })
            .collect(),
        Some(_) => Err(format!("{key} must be an object of strings")),
    };
    if http {
        let url = match entry.get("url") {
            Some(Value::String(url)) if !url.trim().is_empty() => expanded(url.trim())?,
            _ => return Err("url must be the server's address".to_owned()),
        };
        return McpServer::http(name, &url, strings("headers")?);
    }
    let command = match entry.get("command") {
        Some(Value::String(command)) if !command.trim().is_empty() => expanded(command)?,
        _ => return Err("command must be the program to run".to_owned()),
    };
    let args = match entry.get("args") {
        None => Vec::new(),
        Some(Value::Array(args)) => (args.iter())
            .map(|arg| {
                arg.as_str()
                    .ok_or("args must be a list of strings".to_owned())
            })
            .map(|arg| expanded(arg?))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("args must be a list of strings".to_owned()),
    };
    Ok(McpServer {
        name: name.to_owned(),
        transport: McpTransport::Program {
            command,
            args,
            env: strings("env")?,
        },
    })
}

/// `text` with each `${VAR}` replaced by the environment's variable `VAR`, and each
/// `${VAR:-default}` by it or, when it is not set, by `default`.
fn expanded(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let inner = after.find('}').map(|end| &after[..end]);
        let (name, default) = match inner.map(|inner| inner.split_once(":-")) {
            Some(Some((name, default))) => (name, Some(default)),
            _ => (inner.unwrap_or(""), None),
        };
        let named = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let Some(inner) = inner.filter(|_| named) else {
            out.push_str("${");
            rest = after;
            continue;
        };
        match (std::env::var(name), default) {
            (Ok(value), _) => out.push_str(&value),
            (Err(_), Some(default)) => out.push_str(default),
            (Err(_), None) => return Err(format!("${{{name}}} is not set")),
        }
        rest = &after[inner.len() + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `commandSets`' entries; an error when it is not a list of strings.
fn command_sets(object: &Map<String, Value>) -> Result<Vec<String>, ()> {
    paths(object, "commandSets")
}

/// What a token count in the settings must be.
const TOKENS: &str = "a whole number of tokens";

/// The entries of the list of paths `key`; an error when it is not a list of strings.
fn paths(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, ()> {
    match object.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => (entries.iter())
            .map(|entry| entry.as_str().map(str::to_owned).ok_or(()))
            .collect(),
        Some(_) => Err(()),
    }
}

/// What `install_directory` installed.
pub(super) enum Installed {
    Set(Set),
    /// A package directory, with the names of its sets that were in `commandSets` before.
    Package {
        directory: PathBuf,
        sets: Vec<Set>,
        moved: Vec<String>,
        skills: Option<PathBuf>,
        themes: Option<PathBuf>,
    },
}

/// A package's `skills` and `themes` directories, those it has.
pub(super) fn extras(package: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    let directory = |name| Some(package.join(name)).filter(|path| path.is_dir());
    (directory("skills"), directory("themes"))
}

/// Whether `a` and `b` are the same directory, also through links.
fn same_directory(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

fn write_object(path: &Path, object: Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    write_replacing(path, &(text + "\n")).map_err(|error| format!("{}: {error}", shown(path)))
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}

/// The settings in `path`; none when there is no file.
fn read_object(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("{}: {error}", shown(path))),
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(format!("{}: not a JSON object", shown(path))),
        Err(error) => Err(format!("{}: {error}", shown(path))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "scramjet-rs-settings-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn settings_with(scratch: &Scratch, text: &str) -> Settings {
        let path = scratch.0.join("settings.json");
        std::fs::write(&path, text).unwrap();
        Settings::read(Some(path))
    }

    #[test]
    fn without_a_file_the_defaults_hold() {
        let scratch = Scratch::new("missing");
        let settings = Settings::read(Some(scratch.0.join("config/settings.json")));
        assert!(!settings.hide_thinking_block);
        assert_eq!(settings.double_escape, DoubleEscape::Tree);
        assert_eq!(settings.copy_on_select, cfg!(target_os = "macos"));
        assert!(!settings.skip_branch_summary_prompt);
        assert_eq!(settings.tree_filter, Filter::Default);
        assert_eq!(settings.markdown_reads, MarkdownReads::Rendered);
        assert_eq!(settings.default_thinking_level, None);
        assert_eq!(settings.tools, DEFAULT_TOOLS);
        assert_eq!(settings.hide_credentials, HideCredentials::Unattended);
        assert_eq!(settings.steering_mode, Delivery::All);
        assert_eq!(settings.follow_up_mode, Delivery::OneAtATime);
        assert!(settings.skills.is_empty());
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    }

    #[test]
    fn the_sandbox_settings_take_codexs_modes_and_a_network_switch() {
        let scratch = Scratch::new("sandbox");
        let default = settings_with(&scratch, "{}");
        assert_eq!((default.sandbox, default.sandbox_network), (None, true));
        for (value, mode) in [
            ("read-only", Some(SandboxMode::ReadOnly)),
            ("workspace-write", Some(SandboxMode::WorkspaceWrite)),
            ("danger-full-access", None),
        ] {
            let text = format!(r#"{{"sandbox": "{value}", "sandboxNetwork": false}}"#);
            let settings = settings_with(&scratch, &text);
            assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
            assert_eq!((settings.sandbox, settings.sandbox_network), (mode, false));
        }
        let wrong = settings_with(&scratch, r#"{"sandbox": true, "sandboxNetwork": "no"}"#);
        assert_eq!((wrong.sandbox, wrong.sandbox_network), (None, true));
        assert_eq!(wrong.warnings.len(), 2, "{:?}", wrong.warnings);
    }

    #[test]
    fn retry_takes_scramjets_keys_and_retries_given_stay_over_them() {
        let scratch = Scratch::new("retry");
        let defaults = settings_with(&scratch, "{}");
        assert_eq!(defaults.retries(), Retries::default());
        let mut settings = settings_with(
            &scratch,
            r#"{"retry": {"maxRetries": 5, "baseDelayMs": 500,
                "provider": {"maxRetryDelayMs": 0, "timeoutMs": 9}}}"#,
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let expected = Retries {
            attempts: 5,
            delay: Duration::from_millis(500),
            max_wait: None,
        };
        assert_eq!(settings.retries(), expected);
        settings.retries_given = Some(1);
        let given = Retries {
            attempts: 1,
            ..expected
        };
        assert_eq!(settings.retries(), given);
        settings.reload();
        assert_eq!(settings.retries(), given);
        let older = settings_with(
            &scratch,
            r#"{"retry": {"enabled": false, "maxDelayMs": 9000}}"#,
        );
        let older = older.retries();
        assert_eq!(
            (older.attempts, older.max_wait),
            (0, Some(Duration::from_secs(9)))
        );
        let wrong = settings_with(
            &scratch,
            r#"{"retry": {"enabled": 1, "maxRetries": -1, "baseDelayMs": 2147483648,
                "provider": {"maxRetryDelayMs": "1"}}}"#,
        );
        assert_eq!(wrong.retries(), Retries::default());
        let path = shown(&scratch.0.join("settings.json"));
        let number = |key: &str, max: u64| {
            format!("{path}: retry.{key} must be a whole number from 0 to {max}; ignored")
        };
        assert_eq!(
            wrong.warnings,
            [
                number("maxRetries", u32::MAX.into()),
                number("baseDelayMs", 2_147_483_647),
                number("provider.maxRetryDelayMs", 2_147_483_647),
                format!("{path}: retry.enabled must be true or false; ignored"),
            ]
        );
        let not_object = settings_with(&scratch, r#"{"retry": 3}"#);
        assert_eq!(
            not_object.warnings,
            [format!("{path}: retry must be an object; ignored")]
        );
    }

    #[test]
    fn images_take_scramjets_keys_and_name_wrong_ones() {
        let scratch = Scratch::new("images");
        let defaults = settings_with(&scratch, "{}");
        assert_eq!(
            (defaults.resize_images, defaults.block_images),
            (true, false)
        );
        let mut settings = settings_with(
            &scratch,
            r#"{"images": {"autoResize": false, "blockImages": true, "showImages": false}}"#,
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert_eq!(
            (settings.resize_images, settings.block_images),
            (false, true)
        );
        settings.reload();
        assert_eq!(
            (settings.resize_images, settings.block_images),
            (false, true)
        );
        let path = shown(&scratch.0.join("settings.json"));
        let wrong = settings_with(
            &scratch,
            r#"{"images": {"autoResize": "no", "blockImages": 1}}"#,
        );
        assert_eq!((wrong.resize_images, wrong.block_images), (true, false));
        assert_eq!(
            wrong.warnings,
            [
                format!("{path}: images.autoResize must be true or false; ignored"),
                format!("{path}: images.blockImages must be true or false; ignored"),
            ]
        );
        let wrong = settings_with(&scratch, r#"{"images": true}"#);
        assert_eq!(
            wrong.warnings,
            [format!("{path}: images must be an object; ignored")]
        );
    }

    #[test]
    fn the_changelog_settings_take_scramjets_keys() {
        let scratch = Scratch::new("changelog");
        let settings = settings_with(&scratch, "{}");
        assert!(!settings.collapse_changelog);
        assert_eq!(settings.last_changelog_version, None);
        let settings = settings_with(
            &scratch,
            r#"{"collapseChangelog": true, "lastChangelogVersion": "0.9.0"}"#,
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(settings.collapse_changelog);
        assert_eq!(settings.last_changelog_version.as_deref(), Some("0.9.0"));
        let path = shown(&scratch.0.join("settings.json"));
        let wrong = settings_with(&scratch, r#"{"lastChangelogVersion": 9}"#);
        assert_eq!(wrong.last_changelog_version, None);
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: lastChangelogVersion must be a version; ignored"
            )]
        );
    }

    #[test]
    fn retaining_the_transcript_takes_scramjets_key() {
        let scratch = Scratch::new("retain-transcript");
        assert!(!settings_with(&scratch, "{}").retain_transcript);
        let settings = settings_with(&scratch, r#"{"retainTranscriptOnExit": true}"#);
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(settings.retain_transcript);
        let path = shown(&scratch.0.join("settings.json"));
        let wrong = settings_with(&scratch, r#"{"retainTranscriptOnExit": "yes"}"#);
        assert!(!wrong.retain_transcript);
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: retainTranscriptOnExit must be true or false; ignored"
            )]
        );
    }

    #[test]
    fn terminal_progress_takes_scramjets_key_beside_its_image_ones() {
        let scratch = Scratch::new("terminal-progress");
        assert!(!settings_with(&scratch, "{}").terminal_progress);
        let settings = settings_with(
            &scratch,
            r#"{"terminal": {"showTerminalProgress": true, "showImages": false, "imageWidthCells": 40}}"#,
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        assert!(settings.terminal_progress);
        let path = shown(&scratch.0.join("settings.json"));
        let wrong = settings_with(&scratch, r#"{"terminal": {"showTerminalProgress": "on"}}"#);
        assert!(!wrong.terminal_progress);
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: terminal.showTerminalProgress must be true or false; ignored"
            )]
        );
        let wrong = settings_with(&scratch, r#"{"terminal": true}"#);
        assert_eq!(
            wrong.warnings,
            [format!("{path}: terminal must be an object; ignored")]
        );
    }

    #[test]
    fn compaction_takes_scramjets_keys_and_names_wrong_ones() {
        let scratch = Scratch::new("compaction");
        let defaults = settings_with(&scratch, "{}");
        assert_eq!(defaults.compacting, Compacting::default());
        let mut settings = settings_with(
            &scratch,
            r#"{"compaction": {"enabled": false, "reserveTokens": 8000, "keepRecentTokens": 0},
                "branchSummary": {"reserveTokens": 4000}}"#,
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let expected = Compacting {
            automatic: false,
            reserve: 8000,
            keep_recent: 0,
            branch_reserve: 4000,
        };
        assert_eq!(settings.compacting, expected);
        settings.reload();
        assert_eq!(settings.compacting, expected);
        let wrong = settings_with(
            &scratch,
            r#"{"compaction": {"enabled": "no", "reserveTokens": -1, "keepRecentTokens": 1.5},
                "branchSummary": {"reserveTokens": "4000"}}"#,
        );
        assert_eq!(wrong.compacting, Compacting::default());
        let path = shown(&scratch.0.join("settings.json"));
        let tokens = |key: &str| format!("{path}: {key} must be a whole number of tokens; ignored");
        assert_eq!(
            wrong.warnings,
            [
                tokens("branchSummary.reserveTokens"),
                format!("{path}: compaction.enabled must be true or false; ignored"),
                tokens("compaction.reserveTokens"),
                tokens("compaction.keepRecentTokens"),
            ]
        );
        let not_object = settings_with(&scratch, r#"{"compaction": true}"#);
        assert_eq!(
            not_object.warnings,
            [format!("{path}: compaction must be an object; ignored")]
        );
    }

    #[test]
    fn layout_takes_scramjets_keys_within_their_ranges() {
        let scratch = Scratch::new("layout");
        assert_eq!(settings_with(&scratch, "{}").layout, Layout::default());
        let settings = settings_with(
            &scratch,
            r#"{"scrollWheelStep": 7.9, "editorMaxHeightPercent": 50, "autocompleteMaxVisible": 12}"#,
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let expected = Layout {
            wheel_step: 7,
            editor_share: 50,
            completion_rows: 12,
        };
        assert_eq!(settings.layout, expected);
        // As Scramjet's, numbers past either end are taken as that end.
        let outside = settings_with(
            &scratch,
            r#"{"scrollWheelStep": 0, "editorMaxHeightPercent": 90, "autocompleteMaxVisible": -4}"#,
        );
        assert!(outside.warnings.is_empty(), "{:?}", outside.warnings);
        let ends = Layout {
            wheel_step: 1,
            editor_share: 50,
            completion_rows: 3,
        };
        assert_eq!(outside.layout, ends);
        let wrong = settings_with(
            &scratch,
            r#"{"scrollWheelStep": "3", "editorMaxHeightPercent": null, "autocompleteMaxVisible": [5]}"#,
        );
        assert_eq!(wrong.layout, Layout::default());
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            wrong.warnings,
            [
                format!("{path}: scrollWheelStep must be a number from 1 to 20; ignored"),
                format!("{path}: editorMaxHeightPercent must be a number from 10 to 50; ignored"),
                format!("{path}: autocompleteMaxVisible must be a number from 3 to 20; ignored"),
            ]
        );
    }

    #[test]
    fn hide_credentials_says_which_turns_hide_them() {
        let scratch = Scratch::new("credentials");
        for (value, typed, unattended) in [
            ("unattended", false, true),
            ("always", true, true),
            ("never", false, false),
        ] {
            let settings = settings_with(&scratch, &format!(r#"{{"hideCredentials": "{value}"}}"#));
            assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
            let hides = settings.hide_credentials;
            assert_eq!((hides.hides(false), hides.hides(true)), (typed, unattended));
        }
        let wrong = settings_with(&scratch, r#"{"hideCredentials": true}"#);
        assert_eq!(wrong.hide_credentials, HideCredentials::Unattended);
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            wrong.warnings,
            [format!(
                r#"{path}: hideCredentials must be "unattended", "always" or "never"; ignored"#
            )]
        );
    }

    #[test]
    fn tools_lists_the_agents_coding_tools() {
        let scratch = Scratch::new("tools");
        let settings = settings_with(
            &scratch,
            r#"{"tools": ["read", "grep", "gerp", "bash", 3, "ls", "read", "find"]}"#,
        );
        assert_eq!(settings.tools, ["read", "grep", "bash", "ls", "find"]);
        let path = shown(&scratch.0.join("settings.json"));
        let known = CODING_TOOLS.join(", ");
        assert_eq!(
            settings.warnings,
            [
                format!(r#"{path}: tools: "gerp" is not one of {known}; ignored"#),
                format!("{path}: tools: 3 is not one of {known}; ignored"),
            ]
        );
        let none = settings_with(&scratch, r#"{"tools": []}"#);
        assert!(none.tools.is_empty() && none.warnings.is_empty());
        let wrong = settings_with(&scratch, r#"{"tools": "read,grep"}"#);
        assert_eq!(wrong.tools, DEFAULT_TOOLS);
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: tools must be a list of tool names ({known}); ignored"
            )]
        );
    }

    #[test]
    fn reads_scramjets_keys() {
        let scratch = Scratch::new("keys");
        let settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": true, "doubleEscapeAction": "fork",
                "branchSummary": {"reserveTokens": 1, "skipPrompt": true},
                "defaultThinkingLevel": "off", "theme": "pi-light", "treeFilterMode": "labeled-only",
                "resumeChains": "folded", "steeringMode": "one-at-a-time", "followUpMode": "all",
                "markdown": {"toolDisplay": "source"}, "copyOnSelect": true}"#,
        );
        assert!(settings.copy_on_select);
        assert_eq!(settings.tree_filter, Filter::Labeled);
        assert_eq!(settings.resume_chains, Chains::Folded);
        assert_eq!(settings.markdown_reads, MarkdownReads::Source);
        assert_eq!(settings.theme.as_deref(), Some("pi-light"));
        assert!(settings.hide_thinking_block);
        assert_eq!(settings.steering_mode, Delivery::OneAtATime);
        assert_eq!(settings.follow_up_mode, Delivery::All);
        assert_eq!(settings.double_escape, DoubleEscape::Fork);
        assert!(settings.skip_branch_summary_prompt);
        assert_eq!(settings.default_thinking_level.as_deref(), Some("none"));
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let none = settings_with(&scratch, r#"{"doubleEscapeAction": "none"}"#);
        assert_eq!(none.double_escape, DoubleEscape::Nothing);
    }

    #[test]
    #[cfg_attr(windows, ignore = "expands $HOME, which Windows does not set")]
    fn mcp_servers_are_read_as_claude_codes_entries() {
        let scratch = Scratch::new("mcp");
        let settings = settings_with(
            &scratch,
            r#"{"mcpServers": {
                "time": {"command": "uvx", "args": ["mcp-server-time"], "env": {"TZ": "UTC"}},
                "plain": {"type": "stdio", "command": "server"},
                "remote": {"type": "http", "url": "https://example.com/mcp",
                           "headers": {"Authorization": "Bearer ${SCRAMJET_RS_TEST_UNSET:-none}"}},
                "bare": {"url": "http://localhost:3001/mcp?home=${HOME}"},
                "home": {"command": "${HOME}/bin/server", "args": ["$HOME", "${HOME", "${1x}"]},
                "unset": {"type": "http", "url": "https://example.com/mcp",
                          "headers": {"X-Key": "${SCRAMJET_RS_TEST_UNSET}"}},
                "old": {"type": "sse", "url": "https://example.com/sse"},
                "nowhere": {"type": "http"},
                "own": {"url": "https://example.com/mcp", "headers": {"Accept": "text/html"}},
                "nameless": {"args": ["x"]},
                "odd": {"command": "x", "args": [1]},
                "other": {"type": "websocket", "command": "x"}
            }}"#,
        );
        let home = std::env::var("HOME").unwrap();
        let program = |name: &str, command: &str, args: &[&str], env: &[(&str, &str)]| McpServer {
            name: name.to_owned(),
            transport: McpTransport::Program {
                command: command.to_owned(),
                args: args.iter().map(|arg| arg.to_string()).collect(),
                env: (env.iter())
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            },
        };
        let http = |name: &str, url: &str, headers: &[(&str, &str)]| McpServer {
            name: name.to_owned(),
            transport: McpTransport::Http {
                url: url.to_owned(),
                headers: (headers.iter())
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            },
        };
        assert_eq!(
            settings.mcp_servers,
            [
                http(
                    "bare",
                    &format!("http://localhost:3001/mcp?home={home}"),
                    &[]
                ),
                program(
                    "home",
                    &format!("{home}/bin/server"),
                    &["$HOME", "${HOME", "${1x}"],
                    &[]
                ),
                program("plain", "server", &[], &[]),
                http(
                    "remote",
                    "https://example.com/mcp",
                    &[("Authorization", "Bearer none")]
                ),
                program("time", "uvx", &["mcp-server-time"], &[("TZ", "UTC")]),
            ]
        );
        let path = shown(&scratch.0.join("settings.json"));
        let warning = |name: &str, why: &str| format!("{path}: mcpServers: {name}: {why}; ignored");
        assert_eq!(
            settings.warnings,
            [
                warning("nameless", "command must be the program to run"),
                warning("nowhere", "url must be the server's address"),
                warning("odd", "args must be a list of strings"),
                warning(
                    "old",
                    r#"the older HTTP with SSE transport is not supported; give the server's streamable HTTP address with "type": "http""#
                ),
                warning("other", r#"type must be "stdio" or "http""#),
                warning("own", "headers: Accept is set by the transport"),
                warning("unset", "${SCRAMJET_RS_TEST_UNSET} is not set"),
            ]
        );
        let wrong = settings_with(&scratch, r#"{"mcpServers": ["time"]}"#);
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: mcpServers must be an object of servers by name; ignored"
            )]
        );
    }

    #[test]
    fn skills_and_themes_are_paths_from_home_or_the_settings_directory() {
        let scratch = Scratch::new("skills");
        let settings = settings_with(
            &scratch,
            r#"{"skills": ["~/.claude/skills", "~", "own", "/abs/one.md"], "themes": ["mine.json"]}"#,
        );
        let home = std::env::home_dir().unwrap();
        assert_eq!(
            settings.skills,
            [
                home.join(".claude/skills"),
                home.clone(),
                scratch.0.join("own"),
                // On Windows, on the settings file's drive: here the current one.
                std::path::absolute("/abs/one.md").unwrap(),
            ]
        );
        // Shown with the platform's separators, and `~` alone without one after it.
        let displayed: Vec<String> = (settings.skills.iter().take(2))
            .map(|path| path.display().to_string())
            .collect();
        let claude = home.join(".claude").join("skills");
        assert_eq!(
            displayed,
            [claude.display(), home.display()].map(|path| path.to_string())
        );
        assert_eq!(settings.themes, [scratch.0.join("mine.json")]);
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let wrong = settings_with(&scratch, r#"{"skills": "~/.claude/skills", "themes": {}}"#);
        assert!(wrong.skills.is_empty() && wrong.themes.is_empty());
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            wrong.warnings,
            [
                format!("{path}: skills must be a list of files and directories; ignored"),
                format!("{path}: themes must be a list of files and directories; ignored"),
            ]
        );
    }

    #[test]
    fn a_wrong_value_is_named_and_keeps_its_default() {
        let scratch = Scratch::new("wrong");
        let settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": "yes", "doubleEscapeAction": "tree", "defaultThinkingLevel": "hgih",
                "branchSummary": {"skipPrompt": 1}, "followUpMode": "each", "treeFilterMode": "user",
                "resumeChains": "tree", "markdown": {"toolDisplay": "raw"}, "theme": 1, "copyOnSelect": "on", "banner": "big"}"#,
        );
        assert_eq!(settings.banner, Banner::Header);
        assert_eq!(settings.copy_on_select, cfg!(target_os = "macos"));
        assert_eq!(settings.tree_filter, Filter::Default);
        assert_eq!(settings.resume_chains, Chains::Flat);
        assert_eq!(settings.theme, None);
        assert_eq!(settings.markdown_reads, MarkdownReads::Rendered);
        assert!(!settings.hide_thinking_block);
        assert_eq!(settings.follow_up_mode, Delivery::OneAtATime);
        assert_eq!(settings.default_thinking_level, None);
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            settings.warnings,
            [
                format!("{path}: hideThinkingBlock must be true or false; ignored"),
                format!("{path}: copyOnSelect must be true or false; ignored"),
                format!("{path}: branchSummary.skipPrompt must be true or false; ignored"),
                format!(
                    r#"{path}: treeFilterMode must be "default", "no-tools", "user-only", "labeled-only", "all"; ignored"#
                ),
                format!(r#"{path}: resumeChains must be "flat", "threaded", "folded"; ignored"#),
                format!(r#"{path}: markdown.toolDisplay must be "rendered" or "source"; ignored"#),
                format!("{path}: theme must be a theme's name; ignored"),
                format!(r#"{path}: banner must be "plain", "header", "card", "logo"; ignored"#),
                format!(
                    r#"{path}: defaultThinkingLevel must be "off", "minimal", "low", "medium", "high", "xhigh", "max"; ignored"#
                ),
                format!(r#"{path}: followUpMode must be "all" or "one-at-a-time"; ignored"#),
            ]
        );
    }

    #[test]
    fn a_reload_reads_the_file_afresh_and_keeps_the_settings_when_it_cannot() {
        let scratch = Scratch::new("reload");
        let mut settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": true, "tools": ["read"], "skills": ["x"], "followUpMode": "each"}"#,
        );
        assert_eq!(settings.warnings.len(), 1);
        let path = scratch.0.join("settings.json");
        // Keys taken out go back to their defaults, and the old warnings go.
        std::fs::write(
            &path,
            r#"{"commandSets": ["sets/mach12"], "steeringMode": 1}"#,
        )
        .unwrap();
        settings.reload();
        assert!(!settings.hide_thinking_block);
        assert_eq!(settings.tools, DEFAULT_TOOLS);
        assert!(settings.skills.is_empty());
        assert_eq!(settings.command_sets, [scratch.0.join("sets/mach12")]);
        let shown_path = shown(&path);
        assert_eq!(
            settings.warnings,
            [format!(
                r#"{shown_path}: steeringMode must be "all" or "one-at-a-time"; ignored"#
            )]
        );
        std::fs::write(&path, r#"{"commandSets": ["#).unwrap();
        settings.reload();
        assert_eq!(settings.command_sets, [scratch.0.join("sets/mach12")]);
        assert_eq!(settings.warnings.len(), 1);
        assert!(
            (settings.warnings[0].starts_with(&shown_path))
                && settings.warnings[0].ends_with("; kept as they were"),
            "{:?}",
            settings.warnings
        );
        // Changes are still saved to that file.
        assert_eq!(settings.path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn enabled_models_are_patterns_saved_in_order_and_left_out_for_all() {
        let scratch = Scratch::new("enabled-models");
        let mut settings = settings_with(&scratch, r#"{"enabledModels": "sonnet"}"#);
        assert!(settings.enabled_models.is_empty());
        let path = scratch.0.join("settings.json");
        assert_eq!(
            settings.warnings,
            [format!(
                "{}: enabledModels must be a list of model patterns; ignored",
                shown(&path)
            )]
        );
        std::fs::write(
            &path,
            r#"{"theme": "x", "enabledModels": ["sonnet:high", "gpt-*"]}"#,
        )
        .unwrap();
        settings.reload();
        assert_eq!(settings.enabled_models, ["sonnet:high", "gpt-*"]);
        let saved = || -> Value { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };
        settings
            .set_enabled_models(vec!["b".into(), "a".into()])
            .unwrap();
        assert_eq!(saved(), json!({"theme": "x", "enabledModels": ["b", "a"]}));
        settings.set_enabled_models(Vec::new()).unwrap();
        assert_eq!(
            (saved(), settings.enabled_models.len()),
            (json!({"theme": "x"}), 0)
        );
    }

    #[test]
    fn disabled_mcp_servers_are_names_saved_and_left_out_when_none() {
        let scratch = Scratch::new("disabled-mcp");
        let mut settings = settings_with(&scratch, r#"{"disabledMcpServers": "a"}"#);
        let path = scratch.0.join("settings.json");
        assert!(settings.disabled_mcp_servers.is_empty());
        assert_eq!(
            settings.warnings,
            [format!(
                "{}: disabledMcpServers must be a list of server names; ignored",
                shown(&path)
            )]
        );
        std::fs::write(&path, r#"{"disabledMcpServers": ["a"], "theme": "x"}"#).unwrap();
        settings.reload();
        assert_eq!(settings.disabled_mcp_servers, ["a"]);
        let saved = || -> Value { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };
        let names = vec!["a".to_owned(), "b".to_owned()];
        settings.set_disabled_mcp_servers(names).unwrap();
        assert_eq!(
            saved(),
            json!({"theme": "x", "disabledMcpServers": ["a", "b"]})
        );
        settings.set_disabled_mcp_servers(Vec::new()).unwrap();
        assert_eq!(saved(), json!({"theme": "x"}));
    }

    #[test]
    fn a_chosen_model_is_remembered_for_its_project_and_anywhere() {
        let scratch = Scratch::new("startup-model");
        let mut settings = settings_with(&scratch, r#"{"theme": "x"}"#);
        settings.recall = Some(scratch.0.join("state/last-used-models.json"));
        let (here, there) = (Path::new("/work/here"), Path::new("/work/there"));
        assert_eq!(settings.remembered_models(here), (None, None));
        settings
            .remember_model(here, Some("anthropic"), "claude-opus-5-5")
            .unwrap();
        settings
            .remember_model(there, Some("github-copilot"), "gpt-6-luna")
            .unwrap();
        let read = |path: &Path| -> Value {
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
        };
        let path = scratch.0.join("settings.json");
        assert_eq!(
            read(&path),
            json!({"theme": "x", "defaultProvider": "github-copilot", "defaultModel": "gpt-6-luna"})
        );
        let recall = scratch.0.join("state/last-used-models.json");
        assert_eq!(
            read(&recall),
            json!({"/work/here": "anthropic/claude-opus-5-5",
                "/work/there": "github-copilot/gpt-6-luna"})
        );
        // A later session reads both; another project only the model chosen anywhere.
        let mut later = Settings::read(Some(path.clone()));
        later.recall = Some(recall.clone());
        let anywhere = Some("github-copilot/gpt-6-luna".to_owned());
        assert_eq!(
            later.remembered_models(here),
            (
                Some("anthropic/claude-opus-5-5".to_owned()),
                anywhere.clone()
            )
        );
        assert_eq!(
            later.remembered_models(Path::new("/elsewhere")),
            (None, anywhere.clone())
        );
        // `global` starts every project with the model chosen anywhere.
        assert_eq!(later.shown(Row::StartupModel), "last chosen here");
        later.cycle(Row::StartupModel).unwrap();
        assert_eq!(later.shown(Row::StartupModel), "last chosen anywhere");
        assert_eq!(later.remembered_models(here), (None, anywhere));
        assert_eq!(read(&path)["startupModel"], "global");
        assert_eq!(
            Settings::read(Some(path.clone())).startup_model,
            StartupModel::Global
        );
        // A file that is not one is replaced by the next choice.
        std::fs::write(&recall, "[").unwrap();
        assert_eq!(later.remembered_models(there).0, None);
        later.cycle(Row::StartupModel).unwrap();
        // A model no provider offers has no provider; a `defaultModel` alone is as written.
        later.remember_model(there, None, "gpt-6-terra").unwrap();
        assert_eq!(read(&recall), json!({"/work/there": "gpt-6-terra"}));
        assert_eq!(
            later.remembered_models(there),
            (Some("gpt-6-terra".into()), Some("gpt-6-terra".into()))
        );
        assert_eq!(read(&path).get("defaultProvider"), None);
        let wrong = settings_with(&scratch, r#"{"defaultModel": "", "startupModel": "here"}"#);
        let shown_path = shown(&path);
        assert_eq!(
            (wrong.default_model, wrong.startup_model),
            (None, StartupModel::PerDirectory)
        );
        assert_eq!(
            wrong.warnings,
            [
                format!("{shown_path}: defaultModel must be a model's id; ignored"),
                format!(
                    r#"{shown_path}: startupModel must be "per-directory" or "global"; ignored"#
                ),
            ]
        );
    }

    #[test]
    fn saving_keeps_the_other_keys_and_a_broken_file_is_not_overwritten() {
        let scratch = Scratch::new("save");
        let path = scratch.0.join("config/scramjet-rs/settings.json");
        let mut settings = Settings::read(Some(path.clone()));
        settings.set_hide_thinking_block(true).unwrap();
        settings.set_default_thinking_level("none").unwrap();
        settings.set_resume_chains(Chains::Threaded).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved,
            json!({"hideThinkingBlock": true, "defaultThinkingLevel": "off",
                "resumeChains": "threaded"})
        );
        assert_eq!(
            Settings::read(Some(path.clone())).resume_chains,
            Chains::Threaded
        );

        // Another process changed the file meanwhile: its keys stay.
        std::fs::write(&path, r#"{"theme": "x", "hideThinkingBlock": true}"#).unwrap();
        settings.set_default_thinking_level("high").unwrap();
        settings.set_hide_thinking_block(false).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved,
            json!({"theme": "x", "hideThinkingBlock": false, "defaultThinkingLevel": "high"})
        );
        let reread = Settings::read(Some(path.clone()));
        assert!(!reread.hide_thinking_block);
        assert_eq!(reread.default_thinking_level.as_deref(), Some("high"));

        std::fs::write(&path, "{\"hideThinkingBlock\": tru").unwrap();
        let mut broken = Settings::read(Some(path.clone()));
        assert_eq!(broken.warnings.len(), 1);
        assert!(
            broken.warnings[0].ends_with("; using the defaults"),
            "{:?}",
            broken.warnings
        );
        let error = broken.set_hide_thinking_block(true).unwrap_err();
        assert!(error.to_string().starts_with(&shown(&path)), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"hideThinkingBlock\": tru"
        );
        // Nothing is left beside it.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn settings_rows_cycle_through_their_values_and_save_each() {
        let scratch = Scratch::new("rows");
        let mut settings = settings_with(&scratch, r#"{"branchSummary": {"reserveTokens": 1}}"#);
        let path = scratch.0.join("settings.json");
        let saved =
            || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };
        let seen = |row: Row, settings: &mut Settings| {
            let mut values = vec![settings.shown(row)];
            for _ in 0..3 {
                settings.cycle(row).unwrap();
                values.push(settings.shown(row));
            }
            values
        };
        assert_eq!(
            seen(Row::DoubleEscape, &mut settings),
            ["opens /tree", "opens /fork", "does nothing", "opens /tree"]
        );
        assert_eq!(
            seen(Row::Credentials, &mut settings),
            [
                "hidden when unattended",
                "always hidden",
                "never hidden",
                "hidden when unattended"
            ]
        );
        assert_eq!(
            seen(Row::Steering, &mut settings),
            [
                "all at once",
                "one at a time",
                "all at once",
                "one at a time"
            ]
        );
        assert_eq!(
            seen(Row::BranchSummary, &mut settings),
            [
                "asks for a summary",
                "no summary",
                "asks for a summary",
                "no summary"
            ]
        );
        assert_eq!(
            seen(Row::ThinkingBlocks, &mut settings),
            ["shown", "hidden", "shown", "hidden"]
        );
        assert_eq!(
            seen(Row::Look, &mut settings),
            ["panels", "margin", "classic", "rails"]
        );
        // Then round to the first again, and on.
        settings.cycle(Row::Look).unwrap();
        settings.cycle(Row::Look).unwrap();
        assert_eq!(
            seen(Row::Banner, &mut settings),
            ["header", "card", "logo", "plain"]
        );
        settings.cycle(Row::Banner).unwrap();
        settings.cycle(Row::Banner).unwrap();
        let (default, other) = match cfg!(target_os = "macos") {
            true => ("on", "off"),
            false => ("off", "on"),
        };
        assert_eq!(
            seen(Row::CopyOnSelect, &mut settings),
            [default, other, default, other]
        );
        assert_eq!(
            seen(Row::SpellCheck, &mut settings),
            ["on", "off", "on", "off"]
        );
        assert_eq!(
            seen(Row::AutoCompact, &mut settings),
            ["on", "off", "on", "off"]
        );
        assert_eq!(settings.compacting.branch_reserve, 1);
        assert_eq!(
            seen(Row::ResizeImages, &mut settings),
            ["on", "off", "on", "off"]
        );
        assert_eq!(
            seen(Row::BlockImages, &mut settings),
            ["off", "on", "off", "on"]
        );
        assert_eq!(
            seen(Row::WheelStep, &mut settings),
            ["3 lines", "4 lines", "5 lines", "6 lines"]
        );
        assert_eq!(
            seen(Row::PromptHeight, &mut settings),
            [
                "30% of the screen",
                "35% of the screen",
                "40% of the screen",
                "45% of the screen"
            ]
        );
        assert_eq!(
            seen(Row::CompletionRows, &mut settings),
            ["5", "7", "10", "15"]
        );
        // The last value goes round to the first.
        settings.cycle(Row::CompletionRows).unwrap();
        settings.cycle(Row::CompletionRows).unwrap();
        assert_eq!(settings.shown(Row::CompletionRows), "3");
        assert_eq!(
            seen(Row::RetainTranscript, &mut settings),
            ["off", "on", "off", "on"]
        );
        assert_eq!(
            seen(Row::TerminalProgress, &mut settings),
            ["off", "on", "off", "on"]
        );
        assert_eq!(
            seen(Row::Changelog, &mut settings),
            ["whole", "titles", "whole", "titles"]
        );
        settings.set_last_changelog_version("1.2.3").unwrap();
        assert_eq!(
            seen(Row::MarkdownReads, &mut settings),
            ["rendered", "source", "rendered", "source"]
        );
        assert_eq!(
            seen(Row::TreeFilter, &mut settings),
            [
                "all but model changes",
                "no tool results",
                "prompts only",
                "labeled only"
            ]
        );
        settings.cycle(Row::TreeFilter).unwrap();
        assert_eq!(settings.shown(Row::TreeFilter), "everything");
        settings.cycle(Row::FollowUp).unwrap();
        settings
            .set_tools(vec!["ls".into(), "read".into()])
            .unwrap();
        assert_eq!(settings.shown(Row::Tools), "read, ls");
        assert_eq!(settings.theme, None);
        settings.set_theme("scramjet-dark").unwrap();
        assert_eq!(settings.shown(Row::Theme), "scramjet-dark");
        assert_eq!(
            saved(),
            json!({
                "branchSummary": {"reserveTokens": 1, "skipPrompt": true},
                "compaction": {"enabled": false},
                "images": {"autoResize": false, "blockImages": true},
                "markdown": {"toolDisplay": "source"},
                "doubleEscapeAction": "tree",
                "hideCredentials": "unattended",
                "steeringMode": "one-at-a-time",
                "hideThinkingBlock": true,
                "copyOnSelect": !cfg!(target_os = "macos"),
                "spellCheck": false,
                "scrollWheelStep": 6,
                "editorMaxHeightPercent": 45,
                "autocompleteMaxVisible": 3,
                "retainTranscriptOnExit": true,
                "collapseChangelog": true,
                "lastChangelogVersion": "1.2.3",
                "terminal": {"showTerminalProgress": true},
                "treeFilterMode": "all",
                "followUpMode": "all",
                "tools": ["read", "ls"],
                "theme": "scramjet-dark",
                "look": "margin",
                "banner": "card",
            })
        );
        let reread = Settings::read(Some(path.clone()));
        assert!(reread.warnings.is_empty(), "{:?}", reread.warnings);
        for row in ROWS {
            assert_eq!(reread.shown(row), settings.shown(row), "{row:?}");
        }
        settings.set_tools(Vec::new()).unwrap();
        assert_eq!(settings.shown(Row::Tools), "none");
    }

    fn install_set(settings: &mut Settings, directory: &Path) -> Result<Set, String> {
        match settings.install_directory(directory)? {
            Installed::Set(set) => Ok(set),
            Installed::Package { directory, .. } => panic!("{} as a package", directory.display()),
        }
    }

    #[test]
    fn a_directory_holding_sets_skills_or_themes_is_installed_as_a_package() {
        let scratch = Scratch::new("local-package");
        let path = scratch.0.join("config/settings.json");
        let package = scratch.0.join("kit");
        for directory in ["mach12/commands", "scramjet/agents", "skills/a", "themes"] {
            std::fs::create_dir_all(package.join(directory)).unwrap();
        }
        std::fs::write(package.join("mach12/commands/mach12:x.md"), "X\n").unwrap();
        std::fs::write(package.join("scramjet/agents/scramjet:y.md"), "Y\n").unwrap();
        let other = scratch.0.join("other/scramjet");
        std::fs::create_dir_all(other.join("commands")).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mach12 = scramjet_rs::os::tilde(&package.join("mach12"));
        let elsewhere = scramjet_rs::os::tilde(&other);
        let settings_with = |sets: &[&str]| {
            std::fs::write(&path, json!({"commandSets": sets}).to_string()).unwrap();
            Settings::read(Some(path.clone()))
        };

        let mut settings = settings_with(&[&mach12, &elsewhere]);
        assert_eq!(
            settings.install_directory(&package).err().unwrap(),
            format!(
                "a command set named scramjet is installed from {}; remove it first",
                other.display()
            )
        );
        let mut settings = settings_with(&[&mach12]);
        let Installed::Package {
            sets,
            moved,
            skills,
            themes,
            ..
        } = settings.install_directory(&package).unwrap()
        else {
            panic!("a package")
        };
        let names: Vec<&str> = sets.iter().map(|set| set.name.as_str()).collect();
        assert_eq!(names, ["mach12", "scramjet"]);
        assert_eq!(moved, ["mach12"]);
        assert_eq!(
            (skills, themes),
            (Some(package.join("skills")), Some(package.join("themes")))
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let written = scramjet_rs::os::tilde(&package);
        assert_eq!(saved, json!({"commandSets": [], "packages": [written]}));
        let read = Settings::read(Some(path.clone()));
        assert_eq!(
            read.command_sets,
            [package.join("mach12"), package.join("scramjet")]
        );
        assert_eq!(
            (read.skills, read.themes),
            (
                [package.join("skills")].into(),
                [package.join("themes")].into()
            )
        );
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
        assert_eq!(
            settings.install_directory(&package).err().unwrap(),
            format!("{} is already installed", shown(&package))
        );

        assert_eq!(settings.remove_local_package("mach12").unwrap(), None);
        assert_eq!(
            settings.remove_local_package(&written).unwrap(),
            Some(package.clone())
        );
        assert!(settings.command_sets.is_empty() && settings.skills.is_empty());
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, json!({"commandSets": [], "packages": []}));

        std::fs::write(&path, json!({"packages": [written]}).to_string()).unwrap();
        std::fs::remove_dir_all(&package).unwrap();
        assert_eq!(
            Settings::read(Some(path.clone())).warnings,
            [format!(
                "{0}: not a directory; `scramjet-rs remove {0}` takes it out of packages",
                shown(&package)
            )]
        );
        std::fs::create_dir_all(&package).unwrap();
        assert_eq!(
            Settings::read(Some(path.clone())).warnings,
            [format!(
                "{0}: {0} holds no command set, skills or themes",
                shown(&package)
            )]
        );
    }

    #[test]
    fn command_sets_are_installed_and_removed_by_reference() {
        let scratch = Scratch::new("sets");
        let path = scratch.0.join("config/settings.json");
        let set = |name: &str| {
            let directory = scramjet_rs::os::join(&scratch.0, name);
            std::fs::create_dir_all(directory.join("commands")).unwrap();
            directory
        };
        let (mach12, other) = (set("repo/mach12"), set("other/mach12"));
        let rs = set("rs");
        std::fs::create_dir_all(scratch.0.join("plain")).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"theme": "x", "commandSets": ["../rs"]}"#).unwrap();
        let mut settings = Settings::read(Some(path.clone()));
        assert_eq!(settings.command_sets, [scratch.0.join("config/../rs")]);

        let installed = &install_set(&mut settings, &mach12).unwrap();
        assert_eq!(
            (installed.name.as_str(), &installed.directory),
            ("mach12", &mach12)
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Under the home, as Windows' temporary directory is, it is written with `~`.
        let written = scramjet_rs::os::tilde(&mach12);
        assert_eq!(
            saved,
            json!({"theme": "x", "commandSets": ["../rs", written]})
        );
        assert_eq!(
            Settings::read(Some(path.clone())).command_sets,
            settings.command_sets
        );
        assert_eq!(
            install_set(&mut settings, &scratch.0.join("repo/./mach12")).unwrap_err(),
            "mach12 is already installed"
        );
        assert_eq!(
            install_set(&mut settings, &other).unwrap_err(),
            format!(
                "a command set named mach12 is installed from {}; remove it first",
                mach12.display()
            )
        );
        let plain = scratch.0.join("plain");
        assert_eq!(
            install_set(&mut settings, &plain).unwrap_err(),
            format!(
                "{} is not a command set: it has no commands or agents directory, nor a models.json, \
                 and holds no set (none in its top directories, nor one with a set.yaml deeper) \
                 and no skills or themes directory",
                plain.display()
            )
        );
        // A package of providers alone.
        let package = scratch.0.join("package");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("models.json"), "{}").unwrap();
        assert_eq!(
            install_set(&mut settings, &package).unwrap().name,
            "package"
        );
        assert_eq!(
            settings.models_files().last(),
            Some(&package.join("models.json"))
        );
        settings.remove_set("package").unwrap();

        assert_eq!(
            settings.remove_set("rs").unwrap(),
            [scratch.0.join("config/../rs")]
        );
        assert_eq!(settings.command_sets, std::slice::from_ref(&mach12));
        let gone = rs.display().to_string();
        assert_eq!(
            settings.remove_set(&gone).unwrap_err(),
            format!("no installed command set is named {gone} or installed from there")
        );
        assert!(settings.remove_set(&other.display().to_string()).is_err());
        assert_eq!(
            settings.remove_set(&written).unwrap(),
            std::slice::from_ref(&mach12)
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, json!({"theme": "x", "commandSets": []}));

        std::fs::write(&path, r#"{"commandSets": "../rs"}"#).unwrap();
        let mut wrong = Settings::read(Some(path.clone()));
        assert_eq!(
            wrong.warnings,
            [format!(
                "{}: commandSets must be a list of directories; ignored",
                shown(&path)
            )]
        );
        assert!(install_set(&mut wrong, &rs).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"commandSets": "../rs"}"#
        );
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symbolic_link_writes_its_target() {
        let scratch = Scratch::new("link");
        let target = scratch.0.join("dotfiles.json");
        std::fs::write(&target, "{}").unwrap();
        let path = scratch.0.join("settings.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let mut settings = Settings::read(Some(path.clone()));
        settings.set_hide_thinking_block(true).unwrap();
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("\"hideThinkingBlock\": true")
        );
    }
}
