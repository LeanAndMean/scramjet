use scramjet_rs::agent::{Agent, AgentConfig, AgentError, AgentUi, SessionChoice};
use std::{
    ffi::OsString,
    io::{BufRead, IsTerminal, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const USAGE: &str = "Usage: scramjet-rs agent [--project DIR] [--endpoint ADDR] [--model ID] \
[--effort LEVEL] [--max-output-tokens N] [--continue | --session FILE] [--yes] [-p PROMPT]

Interactive coding agent using a Responses endpoint (default: Proxy at 127.0.0.1:18080).
  --project DIR          project directory (default: current directory)
  --model ID             model (default: gpt-6-luna)
  --effort LEVEL         reasoning effort, e.g. low, medium, high
  --continue             resume the most recent session for this project
  --session FILE         resume a specific session file
  --yes                  approve every tool call without asking
  -p PROMPT              run one prompt and exit (`-p -` reads it from stdin)
In the interactive prompt, end a line with \\ to continue it; /exit or Ctrl-D quits.
Ctrl-C cancels the running turn; press it twice to quit.";

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let mut project = None;
    let mut endpoint: SocketAddr = ([127, 0, 0, 1], 18080).into();
    let mut model = "gpt-6-luna".to_owned();
    let mut effort = None;
    let mut max_output_tokens = None;
    let mut choice = SessionChoice::New;
    let mut approve_all = false;
    let mut prompt = None;
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .and_then(|v| v.into_string().ok())
                .ok_or(format!(
                    "{} requires a value\n\n{USAGE}",
                    argument.to_string_lossy()
                ))
        };
        match argument.to_str() {
            Some("--project") => project = Some(PathBuf::from(value()?)),
            Some("--endpoint") => {
                endpoint = value()?.parse().map_err(|_| "invalid --endpoint address")?
            }
            Some("--model") => model = value()?,
            Some("--effort") => effort = Some(value()?),
            Some("--max-output-tokens") => {
                max_output_tokens = Some(
                    value()?
                        .parse()
                        .map_err(|_| "invalid --max-output-tokens")?,
                )
            }
            Some("--continue") => choice = SessionChoice::Continue,
            Some("--session") => choice = SessionChoice::Open(PathBuf::from(value()?)),
            Some("--yes") => approve_all = true,
            Some("-p" | "--print") => prompt = Some(value()?),
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}\n\n{USAGE}")),
        }
    }
    let root = match project {
        Some(path) => path,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let config = AgentConfig {
        endpoint,
        model,
        root,
        effort,
        max_output_tokens,
        approve_all,
    };
    let mut agent = Agent::start(config, choice).map_err(|e| e.to_string())?;

    let cancel = Arc::new(AtomicBool::new(false));
    // A second Ctrl-C while a cancellation is still pending exits.
    signal_hook::flag::register_conditional_shutdown(
        signal_hook::consts::SIGINT,
        130,
        cancel.clone(),
    )
    .map_err(|e| e.to_string())?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, cancel.clone())
        .map_err(|e| e.to_string())?;
    let mut ui = Terminal::new();

    if let Some(prompt) = prompt {
        let prompt = if prompt == "-" {
            std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
        } else {
            prompt
        };
        let result = agent.turn(&prompt, &mut ui, &cancel);
        ui.clear();
        ui.usage(&agent);
        return result.map_err(|e| e.to_string());
    }

    eprintln!(
        "scramjet-rs agent · {} · {}\nsession {}{}",
        agent.model(),
        agent.root().display(),
        agent.session_path().display(),
        if agent.conversation().is_empty() {
            String::new()
        } else {
            format!(" ({} items resumed)", agent.conversation().len())
        }
    );
    let stdin = std::io::stdin();
    loop {
        let mut prompt = String::new();
        loop {
            eprint!("{}", if prompt.is_empty() { "\n> " } else { ". " });
            let mut line = String::new();
            if stdin
                .lock()
                .read_line(&mut line)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Ok(());
            }
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(start) = line.strip_suffix('\\') {
                prompt.push_str(start);
                prompt.push('\n');
            } else {
                prompt.push_str(line);
                break;
            }
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            continue;
        }
        if matches!(prompt, "/exit" | "/quit") {
            return Ok(());
        }
        cancel.store(false, Ordering::Relaxed);
        let result = agent.turn(prompt, &mut ui, &cancel);
        ui.clear();
        match result {
            Ok(()) => {}
            Err(AgentError::Cancelled) => eprintln!("{}", ui.dim("(cancelled)")),
            Err(error) => eprintln!("error: {error}"),
        }
        cancel.store(false, Ordering::Relaxed);
        ui.usage(&agent);
    }
}

struct Terminal {
    color: bool,
    status_shown: bool,
}

impl Terminal {
    fn new() -> Self {
        Self {
            color: std::io::stderr().is_terminal(),
            status_shown: false,
        }
    }
    fn dim(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
    fn clear(&mut self) {
        if self.status_shown {
            eprint!("\r\x1b[K");
            self.status_shown = false;
        }
    }
    fn usage(&self, agent: &Agent) {
        let usage = agent.usage;
        eprintln!(
            "{}",
            self.dim(&format!(
                "[{} requests · {} input tokens ({} cached) · {} output tokens]",
                usage.requests, usage.input_tokens, usage.cached_input_tokens, usage.output_tokens
            ))
        );
    }
}

impl AgentUi for Terminal {
    fn waiting(&mut self) {
        if self.color {
            eprint!("{}", self.dim("… thinking"));
            self.status_shown = true;
        }
    }
    fn assistant(&mut self, text: &str) {
        self.clear();
        if !text.is_empty() {
            println!("{}\n", text.trim_end());
            let _ = std::io::stdout().flush();
        }
    }
    fn tool_call(&mut self, name: &str, summary: &str) {
        self.clear();
        let mut lines = summary.lines();
        let first = lines.next().unwrap_or("");
        let more = if lines.next().is_some() { " …" } else { "" };
        eprintln!("{}", self.dim(&format!("● {name} {first}{more}")));
    }
    fn tool_result(&mut self, output: &str, failed: bool) {
        const SHOWN: usize = 6;
        let lines: Vec<_> = output.lines().collect();
        let mut text: String = lines
            .iter()
            .take(SHOWN)
            .map(|line| format!("  {line}\n"))
            .collect();
        if lines.len() > SHOWN {
            text.push_str(&format!("  … ({} more lines)\n", lines.len() - SHOWN));
        }
        if failed && self.color {
            eprint!("\x1b[31m{text}\x1b[0m");
        } else {
            eprint!("{}", self.dim(&text));
        }
    }
    fn approve(&mut self, question: &str) -> bool {
        self.clear();
        eprint!("{question}\nAllow? [y/N] ");
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer).is_err() {
            return false;
        }
        matches!(answer.trim(), "y" | "Y" | "yes")
    }
}
