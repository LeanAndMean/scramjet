//! `/usage` (also `/cost` and `/stats`): what the saved sessions spent, from their files: this
//! session, today and the last days, each day of the last two weeks, each provider's and each
//! model's requests, tokens and cost, and each model's speed.

use super::slash::grouped;
use super::style;
use super::terminal::{self, Terminal};
use super::tree::local;
use scramjet_rs::agent::usage::{History, Response};
use std::path::Path;

const DAY: u64 = 24 * 60 * 60 * 1000;
/// The days the daily rows go back, today among them.
const DAYS: u64 = 14;
const BAR: usize = 20;
const MODEL_WIDTH: usize = 36;

#[derive(Default)]
struct Totals {
    requests: u64,
    /// With what was read from the cache and written to it.
    input: u64,
    output: u64,
    cache_read: u64,
    cost: f64,
    unpriced: bool,
    first_output: Vec<u64>,
    rates: Vec<f64>,
}

impl Totals {
    fn add(&mut self, response: &Response) {
        self.requests += 1;
        self.input += response.input + response.cache_read + response.cache_write;
        self.output += response.output;
        self.cache_read += response.cache_read;
        match response.cost {
            Some(cost) => self.cost += cost,
            None => self.unpriced = true,
        }
        self.first_output.extend(response.first_output_ms);
        if let Some(duration) = response.duration_ms.filter(|&ms| ms > 0) {
            self.rates
                .push(response.output as f64 * 1000.0 / duration as f64);
        }
    }

    fn of<'a>(responses: impl Iterator<Item = &'a Response>) -> Self {
        let mut totals = Self::default();
        responses.for_each(|response| totals.add(response));
        totals
    }

    fn cost(&self) -> String {
        match terminal::dollars(self.cost, self.unpriced, 2) {
            Some(cost) => cost,
            None if self.unpriced => "$?".to_owned(),
            None => "$0.00".to_owned(),
        }
    }

    fn cached(&self) -> String {
        match self.input {
            0 => "-".to_owned(),
            input => format!("{}%", self.cache_read * 100 / input),
        }
    }
}

/// The value at `share` of `values` (nearest rank), sorting them.
fn percentile<T: PartialOrd + Copy>(values: &mut [T], share: f64) -> Option<T> {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((share * values.len() as f64).ceil() as usize).max(1);
    values.get(rank - 1).copied()
}

fn seconds(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// `11/07`, or `25/11/07` in another year than `now`'s.
fn date(at: u64, now: u64) -> String {
    match (local(at), local(now)) {
        (Some((year, month, day, ..)), Some((this, ..))) if year == this => {
            format!("{month:02}/{day:02}")
        }
        (Some((year, month, day, ..)), _) => {
            format!("{:02}/{month:02}/{day:02}", year.rem_euclid(100))
        }
        _ => "?".to_owned(),
    }
}

fn same_day(a: u64, b: u64) -> bool {
    match (local(a), local(b)) {
        (Some(a), Some(b)) => (a.0, a.1, a.2) == (b.0, b.1, b.2),
        _ => false,
    }
}

/// `model`'s provider: what comes before its `/`, unknown for models saved without one (before
/// 0.17.0).
fn provider(model: &str) -> &str {
    model
        .split_once('/')
        .map_or("unknown", |(provider, _)| provider)
}

/// `responses` added up by `key`, the costliest first, then the busiest.
fn grouped_by<'a>(
    responses: impl Iterator<Item = &'a Response>,
    key: impl Fn(&Response) -> String,
) -> Vec<(String, Totals)> {
    let mut groups: Vec<(String, Totals)> = Vec::new();
    for response in responses {
        let name = key(response);
        match groups.iter_mut().find(|(group, _)| *group == name) {
            Some((_, totals)) => totals.add(response),
            None => {
                let mut totals = Totals::default();
                totals.add(response);
                groups.push((name, totals));
            }
        }
    }
    groups.sort_by(|a, b| (b.1.cost.total_cmp(&a.1.cost)).then(b.1.requests.cmp(&a.1.requests)));
    groups
}

/// The report, `current` being this session's file and `model` its model, at `now`
/// (milliseconds since 1970).
pub(super) fn report(
    history: &History,
    sessions: &Path,
    current: &Path,
    model: &str,
    now: u64,
    ui: &Terminal,
) -> String {
    let heading = |text: &str| format!("{}\n", ui.styled(style::INFO_HEADING, text));
    let responses = &history.responses;
    let mut text = heading("Usage") + "\n";
    if responses.is_empty() {
        text += &ui.dim("No responses saved yet.");
        text.push('\n');
        return text;
    }
    let row = |label: &str, totals: &Totals| {
        format!(
            "{}{:>10}{:>9}{:>9}{:>8}{:>11}\n",
            ui.dim(&format!("{label:<14}")),
            grouped(totals.requests),
            terminal::tokens(totals.input),
            terminal::tokens(totals.output),
            totals.cached(),
            totals.cost(),
        )
    };
    text += &ui.dim(&format!(
        "{:<14}{:>10}{:>9}{:>9}{:>8}{:>11}",
        "", "Requests", "Input", "Output", "Cached", "Cost"
    ));
    text.push('\n');
    let session = history.session(current);
    let this = Totals::of(responses.iter().filter(|r| Some(r.session) == session));
    text += &row("This session", &this);
    let today = Totals::of(responses.iter().filter(|r| same_day(r.at, now)));
    text += &row("Today", &today);
    for days in [7, 30] {
        let since = now.saturating_sub(days * DAY);
        let totals = Totals::of(responses.iter().filter(|r| r.at >= since));
        text += &row(&format!("Last {days} days"), &totals);
    }
    text += &row("All", &Totals::of(responses.iter()));

    text += "\n";
    text += &heading(&format!("Last {DAYS} days"));
    let mut days: Vec<(u64, Totals)> = Vec::new();
    for back in (0..DAYS).rev() {
        let at = now.saturating_sub(back * DAY);
        let totals = Totals::of(responses.iter().filter(|r| same_day(r.at, at)));
        if totals.requests > 0 {
            days.push((at, totals));
        }
    }
    let most = days.iter().map(|(_, day)| day.cost).fold(0.0, f64::max);
    for (at, day) in &days {
        let bar = match most > 0.0 {
            true => "█".repeat((day.cost / most * BAR as f64).round() as usize),
            false => String::new(),
        };
        text += &format!(
            "{}{:>11}{:>10} {}\n",
            ui.dim(&format!("{:<8}", date(*at, now))),
            day.cost(),
            grouped(day.requests),
            ui.styled(style::USAGE_BAR, &bar),
        );
    }
    if days.is_empty() {
        text += &ui.dim("No responses.");
        text.push('\n');
    }

    let since = now.saturating_sub(30 * DAY);
    let recent = || responses.iter().filter(|r| r.at >= since);
    let table = |label: &str, rows: &[(String, Totals)]| {
        let width = (rows.iter().map(|(name, _)| name.chars().count()))
            .chain([label.len()])
            .max()
            .unwrap_or(0)
            .min(MODEL_WIDTH);
        let mut text = ui.dim(&format!(
            "{label:<width$}{:>10}{:>9}{:>9}{:>8}{:>11}",
            "Requests", "Input", "Output", "Cached", "Cost"
        ));
        text.push('\n');
        for (name, totals) in rows {
            text += &format!(
                "{:<width$}{:>10}{:>9}{:>9}{:>8}{:>11}\n",
                super::text::clip(name, width),
                grouped(totals.requests),
                terminal::tokens(totals.input),
                terminal::tokens(totals.output),
                totals.cached(),
                totals.cost(),
            );
        }
        if rows.is_empty() {
            text += &ui.dim("No responses.");
            text.push('\n');
        }
        text
    };

    text += "\n";
    text += &heading("Providers, last 30 days");
    let current = provider(model);
    let mut providers = grouped_by(recent(), |r| provider(&r.model).to_owned());
    if !providers.iter().any(|(name, _)| name == current) {
        providers.push((current.to_owned(), Totals::default()));
    }
    providers.sort_by_key(|(name, _)| *name != current);
    text += &table("Provider", &providers);

    text += "\n";
    text += &heading("Models, last 30 days");
    let mut models = grouped_by(recent(), |r| match r.model.as_str() {
        "" => "unknown".to_owned(),
        name => name.to_owned(),
    });
    text += &table("Model", &models);

    models.retain(|(_, totals)| !totals.first_output.is_empty() || !totals.rates.is_empty());
    if !models.is_empty() {
        text += "\n";
        text += &heading("Speed, last 30 days");
        let width = (models.iter().map(|(name, _)| name.chars().count()))
            .max()
            .unwrap_or(0)
            .clamp(5, MODEL_WIDTH);
        text += &ui.dim(&format!(
            "{:<width$}{:>8} {:>12}{:>8} {:>12}{:>12}",
            "Model", "Timed", "First output", "p90", "Rate", "p10"
        ));
        text.push('\n');
        for (name, totals) in &mut models {
            let mut first =
                |share| percentile(&mut totals.first_output, share).map_or("-".to_owned(), seconds);
            let (first, slow_first) = (first(0.5), first(0.9));
            let mut rate =
                |share| percentile(&mut totals.rates, share).map_or("-".to_owned(), terminal::rate);
            let (rate, slow_rate) = (rate(0.5), rate(0.1));
            let timed = totals.first_output.len().max(totals.rates.len()) as u64;
            text += &format!(
                "{:<width$}{:>8} {first:>12}{slow_first:>8} {rate:>12}{slow_rate:>12}\n",
                super::text::clip(name, width),
                grouped(timed),
            );
        }
    }

    text += "\n";
    let first = responses.iter().map(|r| r.at).min().unwrap_or(now);
    let mut said = format!(
        "From {} session files in {}, since {}. The current provider comes first. First output and rate are \
medians, beside the slowest tenth's (p90 and p10); cost is $? where the provider gives no price.",
        grouped(history.files.len() as u64),
        sessions.display(),
        date(first, now),
    );
    if !history.unreadable.is_empty() {
        said += &format!(" {} could not be read.", history.unreadable.len());
    }
    text += &ui.dim(&said);
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};

    fn response(at: u64, model: &str, cost: Option<f64>, session: usize) -> Response {
        Response {
            at,
            model: model.to_owned(),
            input: 100,
            output: 50,
            cache_read: 300,
            cache_write: 0,
            cost,
            first_output_ms: Some(1500),
            duration_ms: Some(2000),
            session,
        }
    }

    #[test]
    fn the_report_adds_up_sessions_days_and_models() {
        let now = 1_794_000_000_000;
        let history = History {
            responses: vec![
                response(now - 40 * DAY, "a/old", Some(9.0), 0),
                response(now - 3 * DAY, "a/one", Some(1.0), 0),
                Response {
                    first_output_ms: Some(3500),
                    duration_ms: Some(1000),
                    ..response(now - 1000, "a/one", Some(2.0), 1)
                },
                Response {
                    first_output_ms: None,
                    duration_ms: None,
                    ..response(now - 500, "sub", None, 1)
                },
            ],
            files: vec!["/s/p/1.jsonl".into(), "/s/p/2.jsonl".into()],
            unreadable: Vec::new(),
        };
        let ui = Terminal::new(Arc::new(AtomicBool::new(false)), false);
        let report = report(
            &history,
            Path::new("/s"),
            Path::new("/s/p/2.jsonl"),
            "b/new",
            now,
            &ui,
        );
        let report = super::super::text::plain(&report);
        let lines: Vec<&str> = report.lines().map(str::trim_end).collect();
        let day = |back: u64| date(now - back * DAY, now);
        assert_eq!(
            lines[..9],
            [
                "Usage",
                "",
                "                Requests    Input   Output  Cached       Cost",
                "This session           2      800      100     75%    $2.00+?",
                "Today                  2      800      100     75%    $2.00+?",
                "Last 7 days            3     1.2k      150     75%    $3.00+?",
                "Last 30 days           3     1.2k      150     75%    $3.00+?",
                "All                    4     1.6k      200     75%   $12.00+?",
                "",
            ]
        );
        assert_eq!(lines[9], "Last 14 days");
        assert_eq!(
            lines[10],
            format!("{:<8}      $1.00         1 {}", day(3), "█".repeat(10))
        );
        assert_eq!(
            lines[11],
            format!("{:<8}    $2.00+?         2 {}", day(0), "█".repeat(20))
        );
        assert_eq!(
            lines[13..],
            [
                "Providers, last 30 days",
                "Provider  Requests    Input   Output  Cached       Cost",
                "b                0        0        0       -      $0.00",
                "a                2      800      100     75%      $3.00",
                "unknown          1      400       50     75%         $?",
                "",
                "Models, last 30 days",
                "Model  Requests    Input   Output  Cached       Cost",
                "a/one         2      800      100     75%      $3.00",
                "sub           1      400       50     75%         $?",
                "",
                "Speed, last 30 days",
                "Model   Timed First output     p90         Rate         p10",
                "a/one       2         1.5s    3.5s      25tok/s     25tok/s",
                "",
                lines[28],
            ]
        );
        assert!(
            lines[28].starts_with("From 2 session files in /s, since "),
            "{}",
            lines[28]
        );
    }
}
