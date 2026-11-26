//! `/loop`, as Claude Code's: `/loop [INTERVAL] PROMPT` or `/loop PROMPT every N UNIT` sends
//! the prompt at once, then again each interval while the full screen waits for the user
//! (`Agent::loop_due`, polled by `Interactive::read`). `/loop` says the loop, `/loop stop`
//! ends it.
use super::interactive::Interactive;
use super::style;
use scramjet::agent::recurring::{LIFETIME, interval};
use std::time::{Duration, Instant};

/// What `/loop` takes to stop the loop, as `/goal` takes to clear the goal.
const STOP: [&str; 6] = ["stop", "off", "clear", "cancel", "reset", "none"];

/// The interval without one given. Claude Code lets the model pace itself then, through a tool
/// Scramjet does not have; its `/loop` defaulted to 10 minutes before.
const DEFAULT: Duration = Duration::from_secs(10 * 60);

const USAGE: &str = "Usage: /loop [INTERVAL] PROMPT, /loop PROMPT every INTERVAL, /loop stop \
(an interval as 30s, 5m, 2h or 1d; 10m when none is given)";

impl Interactive {
    pub(super) fn looping(&mut self, argument: &str) {
        let agent = &mut self.session.agent;
        if argument.is_empty() {
            let said = match agent.looping() {
                None => format!("No loop set. {USAGE}"),
                Some(looping) => {
                    let next = looping.due_in(Instant::now());
                    let next = match next.is_zero() {
                        true => "next run when this turn ends".to_owned(),
                        false => format!("next in {}", interval(rounded(next))),
                    };
                    format!(
                        "Loop: {} every {}, sent {} {}, {next}; /loop stop ends it",
                        looping.prompt,
                        interval(looping.every),
                        looping.runs,
                        if looping.runs == 1 { "time" } else { "times" },
                    )
                }
            };
            return self.say_styled(style::MUTED, &said);
        }
        if STOP.contains(&argument.to_lowercase().as_str()) {
            let said = match agent.stop_loop() {
                Some(looping) => format!("Loop stopped: {}", looping.prompt),
                None => "No loop set".to_owned(),
            };
            self.footer_changed();
            return self.say_styled(style::MUTED, &said);
        }
        if !self.interactive() {
            let said = "/loop runs on the full screen only: here nothing can send a prompt while \
                Scramjet waits for yours";
            return self.say(said);
        }
        let (every, prompt) = match parse(argument) {
            Ok(parsed) => parsed,
            Err(error) => return self.say(&format!("{error}. {USAGE}")),
        };
        if prompt.split_whitespace().next() == Some("/loop") {
            return self.say("A loop cannot run /loop");
        }
        let agent = &mut self.session.agent;
        let replaced = agent.stop_loop().is_some();
        agent.start_loop(&prompt, every, Instant::now());
        self.footer_changed();
        let when = match self.in_turn {
            true => "first run when this turn ends",
            false => "running it now",
        };
        let said = format!(
            "Loop {}: {prompt} every {}, {when}; /loop stop ends it, and it ends by itself \
            after 7 days",
            if replaced { "replaced" } else { "set" },
            interval(every)
        );
        self.say_styled(style::MUTED, &said);
    }
}

/// `next` to the second, or the minute once it is over an hour, for saying it.
fn rounded(next: Duration) -> Duration {
    let unit = if next >= Duration::from_secs(3600) {
        60
    } else {
        1
    };
    let seconds = next.as_secs() + u64::from(next.subsec_nanos() > 0);
    Duration::from_secs(seconds.div_ceil(unit) * unit)
}

/// The interval and the prompt, by Claude Code's rules: an interval as the first word (`5m`),
/// else a trailing `every 20m` or `every 5 minutes`, else the whole argument is the prompt.
fn parse(argument: &str) -> Result<(Duration, String), String> {
    let words: Vec<&str> = argument.split_whitespace().collect();
    let (every, prompt) = match words.as_slice() {
        [first, rest @ ..] if let Some(every) = short(first) => (every, rest.join(" ")),
        [rest @ .., "every", amount, unit] if let Some(every) = long(amount, unit) => {
            (every, rest.join(" "))
        }
        [rest @ .., "every", amount] if let Some(every) = short(amount) => (every, rest.join(" ")),
        _ => (Some(DEFAULT), argument.to_owned()),
    };
    let every = every.ok_or("The interval must be more than 0")?;
    match prompt.trim() {
        "" => Err("No prompt given".to_owned()),
        prompt => Ok((every, prompt.to_owned())),
    }
}

/// An interval written `5m`: `None` inside when it is 0.
fn short(word: &str) -> Option<Option<Duration>> {
    let split = word.find(|c: char| !c.is_ascii_digit())?;
    let (amount, unit) = word.split_at(split);
    long(amount, unit)
}

/// An interval written `5 minutes` (or `5 m`): `None` inside when it is 0.
fn long(amount: &str, unit: &str) -> Option<Option<Duration>> {
    if amount.is_empty() || !amount.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds = match unit.to_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        _ => return None,
    };
    let amount: u64 = amount.parse().ok()?;
    let seconds = amount.checked_mul(seconds)?.min(LIFETIME.as_secs());
    Some((seconds > 0).then(|| Duration::from_secs(seconds)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minutes(minutes: u64) -> Duration {
        Duration::from_secs(minutes * 60)
    }

    #[test]
    fn the_interval_is_read_as_claude_codes_loop_reads_it() {
        let parsed = parse;
        assert_eq!(
            parsed("5m /babysit-prs"),
            Ok((minutes(5), "/babysit-prs".to_owned()))
        );
        assert_eq!(
            parsed("check the deploy every 20m"),
            Ok((minutes(20), "check the deploy".to_owned()))
        );
        assert_eq!(
            parsed("run tests every 5 minutes"),
            Ok((minutes(5), "run tests".to_owned()))
        );
        assert_eq!(
            parsed("1h /standup 1"),
            Ok((minutes(60), "/standup 1".to_owned()))
        );
        assert_eq!(
            parsed("30s ping"),
            Ok((Duration::from_secs(30), "ping".to_owned()))
        );
        assert_eq!(
            parsed("check every PR"),
            Ok((minutes(10), "check every PR".to_owned()))
        );
        assert_eq!(
            parsed("check the deploy"),
            Ok((minutes(10), "check the deploy".to_owned()))
        );
        assert_eq!(parsed("5m"), Err("No prompt given".to_owned()));
        assert_eq!(
            parsed("0m ping"),
            Err("The interval must be more than 0".to_owned())
        );
    }

    #[test]
    fn the_time_to_the_next_run_is_said_to_the_second_or_the_minute() {
        assert_eq!(interval(rounded(Duration::from_millis(61_500))), "1m 2s");
        assert_eq!(interval(rounded(Duration::from_secs(3601))), "1h 1m");
    }
}
