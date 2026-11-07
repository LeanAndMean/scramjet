//! `/usage` (also `/cost` and `/stats`): what is left of a subscription's quota, and what the
//! saved sessions spent, from their files: this session, today and the last days, each day of the
//! last two weeks and each week of the last twelve, each provider's and each model's requests,
//! tokens and cost, and each model's speed.

use super::keymap::{self, Binding};
use super::keys::Key;
use super::slash::grouped;
use super::style;
use super::terminal::{self, Terminal};
use super::tree::local;
use scramjet_rs::agent::quota::{Credits, Quota, Window};
use scramjet_rs::agent::usage::{History, Response, civil_days};
use std::path::Path;

const DAY: u64 = 24 * 60 * 60 * 1000;
/// The days the daily rows go back, today among them.
const DAYS: i64 = 14;
/// The weeks the weekly rows go back, this one among them.
const WEEKS: i64 = 12;
const BAR: usize = 20;
const MODEL_WIDTH: usize = 36;
/// The sessions the costliest list shows.
const SESSIONS: usize = 10;
const PROJECT_WIDTH: usize = 16;
const TITLE_WIDTH: usize = 40;

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

/// The date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_date(days: i64) -> (i32, i32, i32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month + 2) / 5 + 1;
    let month = if month < 10 { month + 3 } else { month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month as i32, day as i32)
}

/// The local day of `at`, in days since 1970-01-01.
fn local_day(at: u64) -> Option<i64> {
    local(at).map(|(year, month, day, ..)| civil_days(year, month, day))
}

/// The Monday starting the week of `day` (days since 1970-01-01, a Thursday).
fn monday(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

/// `day` (days since 1970-01-01) as `date` shows it.
fn day_label(day: i64, now: u64) -> String {
    let (year, month, date) = civil_date(day);
    match local(now) {
        Some((this, ..)) if this == year => format!("{month:02}/{date:02}"),
        _ => format!("{:02}/{month:02}/{date:02}", year.rem_euclid(100)),
    }
}

/// What one bar's characters stand for, darkest first; the last stands for the rest when there
/// are more groups than shades.
const SHADES: [char; 4] = ['█', '▓', '▒', '░'];

/// The last days' and weeks' rows, each with a bar split by provider (`by_provider`) or by
/// model, measuring cost, or tokens where nothing shown has a price.
fn charts(responses: &[&Response], now: u64, by_provider: bool, ui: &Terminal) -> String {
    let group = |response: &Response| match (by_provider, response.model.as_str()) {
        (true, model) => provider(model).to_owned(),
        (false, "") => "unknown".to_owned(),
        (false, model) => model.to_owned(),
    };
    let today = local_day(now).unwrap_or(0);
    let first_week = monday(today) - 7 * (WEEKS - 1);
    let dated: Vec<(i64, &Response)> = (responses.iter())
        .filter_map(|&r| Some((local_day(r.at)?, r)))
        .filter(|&(day, _)| day >= first_week.min(today - (DAYS - 1)))
        .collect();
    let by_cost = dated
        .iter()
        .any(|(_, r)| r.cost.is_some_and(|cost| cost > 0.0));
    let measure = |r: &Response| match by_cost {
        true => r.cost.unwrap_or(0.0),
        false => (r.input + r.cache_read + r.cache_write + r.output) as f64,
    };
    let mut groups: Vec<(String, f64)> = Vec::new();
    for (_, response) in &dated {
        let name = group(response);
        match groups.iter_mut().find(|(group, _)| *group == name) {
            Some((_, total)) => *total += measure(response),
            None => groups.push((name, measure(response))),
        }
    }
    groups.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut named: Vec<String> = groups.into_iter().map(|(name, _)| name).collect();
    let others = named.len() > SHADES.len();
    named.truncate(SHADES.len() - usize::from(others));

    let rows = |spans: Vec<(i64, i64)>| -> String {
        let rows: Vec<(i64, Vec<&Response>)> = (spans.into_iter())
            .map(|(from, to)| {
                let span = dated.iter().filter(|&&(day, _)| from <= day && day < to);
                (from, span.map(|&(_, r)| r).collect::<Vec<_>>())
            })
            .filter(|(_, span)| !span.is_empty())
            .collect();
        let parts = |span: &[&Response]| {
            let mut parts = vec![0.0; SHADES.len()];
            for response in span {
                let name = group(response);
                let slot =
                    (named.iter().position(|named| *named == name)).unwrap_or(SHADES.len() - 1);
                parts[slot] += measure(response);
            }
            parts
        };
        let most = (rows.iter())
            .map(|(_, span)| parts(span).iter().sum::<f64>())
            .fold(0.0, f64::max);
        let mut text = String::new();
        for (from, span) in &rows {
            let mut bar = String::new();
            let (mut sum, mut drawn) = (0.0, 0);
            for (shade, part) in SHADES.iter().zip(parts(span)) {
                sum += part;
                let end = match most > 0.0 {
                    true => (sum / most * BAR as f64).round() as usize,
                    false => 0,
                };
                bar.extend(std::iter::repeat_n(*shade, end.saturating_sub(drawn)));
                drawn = drawn.max(end);
            }
            let totals = Totals::of(span.iter().copied());
            text += &format!(
                "{}{:>11}{:>10} {}\n",
                ui.dim(&format!("{:<8}", day_label(*from, now))),
                totals.cost(),
                grouped(totals.requests),
                ui.styled(style::USAGE_BAR, &bar),
            );
        }
        if rows.is_empty() {
            text += &ui.dim("No responses.");
            text.push('\n');
        }
        text
    };

    let heading = |text: &str| format!("\n{}\n", ui.styled(style::INFO_HEADING, text));
    let mut text = heading(&format!("Last {DAYS} days"));
    text += &rows(
        ((today - (DAYS - 1))..=today)
            .map(|day| (day, day + 1))
            .collect(),
    );
    text += &heading(&format!("Last {WEEKS} weeks, from Mondays"));
    let weeks = (0..WEEKS).map(|week| first_week + 7 * week);
    text += &rows(weeks.map(|monday| (monday, monday + 7)).collect());
    if !named.is_empty() {
        let others = others.then(|| (&SHADES[SHADES.len() - 1], "others"));
        let legend: Vec<String> = (SHADES.iter().zip(named.iter().map(String::as_str)))
            .chain(others)
            .map(|(shade, name)| {
                let shade = ui.styled(style::USAGE_BAR, &shade.to_string());
                format!("{shade} {}", ui.dim(name))
            })
            .collect();
        let measure = match by_cost {
            true => "cost",
            false => "tokens",
        };
        let split = match by_provider {
            true => "provider",
            false => "model",
        };
        text += &ui.dim(&format!("Bars: {measure} by {split}: "));
        text += &legend.join("  ");
        text.push('\n');
    }
    text
}

/// `at` (milliseconds since 1970) as a time of day, with its date as `date` writes it when not
/// `now`'s.
fn clock(at: u64, now: u64) -> String {
    match local(at) {
        Some((.., hour, minute)) if same_day(at, now) => format!("{hour:02}:{minute:02}"),
        Some((.., hour, minute)) => format!("{} {hour:02}:{minute:02}", date(at, now)),
        None => "?".to_owned(),
    }
}

/// What a window of `minutes` is called: Codex's five-hour and weekly ones are `5h` and `Weekly`.
fn window_label(minutes: Option<i64>) -> String {
    const DAY: i64 = 24 * 60;
    match minutes {
        Some(m) if m == DAY => "Daily".to_owned(),
        Some(m) if m == 7 * DAY => "Weekly".to_owned(),
        Some(m) if (28 * DAY..=31 * DAY).contains(&m) => "Monthly".to_owned(),
        Some(m) if m > 0 && m % DAY == 0 => format!("{}-day", m / DAY),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
        Some(m) if m > 0 => format!("{m}m"),
        _ => "Usage".to_owned(),
    }
}

/// The quota of `only`, or of every provider, the current one (`current`) first, each as its
/// latest response reported it, with a bar for each window.
fn quotas(history: &History, only: Option<&str>, current: &str, now: u64, ui: &Terminal) -> String {
    let mut shown: Vec<&(String, Quota)> = (history.quotas.iter())
        .filter(|(provider, _)| only.is_none_or(|only| provider == only))
        .collect();
    shown.sort_by_key(|(provider, _)| (provider != current, provider.clone()));
    let mut text = String::new();
    for (provider, quota) in shown {
        let at = clock(quota.observed_at, now);
        let title = match only {
            Some(_) => format!("Quota, as of {at}"),
            None => format!("Quota of {provider}, as of {at}"),
        };
        text += &format!(
            "{}
",
            ui.styled(style::INFO_HEADING, &title)
        );
        // As Codex's rows: a window's length, after the name of the limit it is of.
        let labels: Vec<String> = (quota.windows.iter())
            .map(|window| {
                let label =
                    (window.name.clone()).unwrap_or_else(|| window_label(window.window_minutes));
                match &window.limit {
                    Some(limit) => format!("{limit} {label} limit"),
                    None => format!("{label} limit"),
                }
            })
            .collect();
        let width = (labels.iter())
            .map(|label| label.chars().count() + 1)
            .fold(14, usize::max);
        for (window, label) in quota.windows.iter().zip(labels) {
            let Window {
                used_percent,
                resets_at,
                allowed,
                over,
                ..
            } = window;
            let used = used_percent.clamp(0.0, 100.0);
            let filled = ((used / 100.0 * BAR as f64).round() as usize).min(BAR);
            let bar = ui.styled(style::USAGE_BAR, &"█".repeat(filled))
                + &ui.dim(&"░".repeat(BAR - filled));
            let resets = match resets_at.map(|at| at.max(0) as u64 * 1000) {
                // What was used then no longer counts.
                Some(at) if at <= now => format!(", reset since {}", clock(at, now)),
                Some(at) => format!(", resets {}", clock(at, now)),
                None => String::new(),
            };
            let count = match allowed {
                Some(allowed) => {
                    let made = (*allowed as f64 * used / 100.0).round();
                    format!(" ({made} of {allowed})")
                }
                None => String::new(),
            };
            let over = over.map_or(String::new(), |over| format!(", {over} over"));
            text += &format!(
                "{}{bar} {used:>3.0}% used{count}{over}{resets}\n",
                ui.dim(&format!("{label:<width$}")),
            );
        }
        if let Some(credits) = &quota.credits {
            text += &format!(
                "{}{}\n",
                ui.dim(&format!("{:<width$}", "Credits")),
                credits_text(credits)
            );
        }
        text.push('\n');
    }
    text
}

/// Credits as Codex shows them: `Unlimited`, the balance rounded, or `Available` without one.
fn credits_text(credits: &Credits) -> String {
    let balance = (credits.balance.as_deref())
        .and_then(|balance| balance.trim().parse::<f64>().ok())
        .filter(|balance| balance.is_finite() && *balance > 0.0);
    match (credits.unlimited, balance) {
        (true, _) => "Unlimited".to_owned(),
        (false, Some(balance)) => format!("{} credits", balance.round()),
        (false, None) => "Available".to_owned(),
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
/// (milliseconds since 1970): of `only`'s responses, or of every provider's.
pub(super) fn report(
    history: &History,
    sessions: &Path,
    current: &Path,
    model: &str,
    now: u64,
    only: Option<&str>,
    ui: &Terminal,
) -> String {
    let heading = |text: &str| format!("{}\n", ui.styled(style::INFO_HEADING, text));
    let responses: Vec<&Response> = (history.responses.iter())
        .filter(|r| only.is_none_or(|only| provider(&r.model) == only))
        .collect();
    let title = match only {
        Some(only) => format!("Usage of {only}"),
        None => "Usage".to_owned(),
    };
    let mut text = heading(&title) + "\n";
    text += &quotas(history, only, provider(model), now, ui);
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
    let this = Totals::of(
        responses
            .iter()
            .copied()
            .filter(|r| Some(r.session) == session),
    );
    text += &row("This session", &this);
    let today = Totals::of(responses.iter().copied().filter(|r| same_day(r.at, now)));
    text += &row("Today", &today);
    for days in [7, 30] {
        let since = now.saturating_sub(days * DAY);
        let totals = Totals::of(responses.iter().copied().filter(|r| r.at >= since));
        text += &row(&format!("Last {days} days"), &totals);
    }
    text += &row("All", &Totals::of(responses.iter().copied()));

    text += &charts(&responses, now, only.is_none(), ui);

    let since = now.saturating_sub(30 * DAY);
    let recent = || responses.iter().copied().filter(|r| r.at >= since);
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

    if only.is_none() {
        text += "\n";
        text += &heading("Providers, last 30 days");
        let current = provider(model);
        let mut providers = grouped_by(recent(), |r| provider(&r.model).to_owned());
        if !providers.iter().any(|(name, _)| name == current) {
            providers.push((current.to_owned(), Totals::default()));
        }
        providers.sort_by_key(|(name, _)| *name != current);
        text += &table("Provider", &providers);
    }

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

    let mut costliest = grouped_by(recent(), |r| r.session.to_string());
    costliest.truncate(SESSIONS);
    if !costliest.is_empty() {
        text += "\n";
        text += &heading("Costliest sessions, last 30 days");
        text += &ui.dim(&format!(
            "{:<8}{:>10}{:>9}{:>11}  {:<PROJECT_WIDTH$}  Session",
            "Started", "Requests", "Tokens", "Cost", "Project"
        ));
        text.push('\n');
        for (index, totals) in &costliest {
            let index: usize = index.parse().unwrap_or(usize::MAX);
            let started = (responses.iter())
                .filter(|r| r.session == index)
                .map(|r| r.at)
                .min()
                .unwrap_or(now);
            let summary = history.summaries.get(index);
            let project = (summary.and_then(|summary| summary.root.as_deref()))
                .and_then(Path::file_name)
                .map_or("?".into(), |name| name.to_string_lossy());
            let project = super::text::clip(&project, PROJECT_WIDTH);
            let title = match summary.and_then(|summary| summary.title.as_deref()) {
                Some(title) => super::text::clip(title, TITLE_WIDTH),
                None => ui.dim("(no prompt)"),
            };
            let mark = match Some(index) == session {
                true => ui.dim(" (this one)"),
                false => String::new(),
            };
            text += &format!(
                "{:<8}{:>10}{:>9}{:>11}  {project:<PROJECT_WIDTH$}  {title}{mark}\n",
                date(started, now),
                grouped(totals.requests),
                terminal::tokens(totals.input + totals.output),
                totals.cost(),
            );
        }
    }

    text += "\n";
    let first = responses.iter().map(|r| r.at).min().unwrap_or(now);
    let order = match only {
        None => " The current provider comes first.",
        Some(_) => "",
    };
    let mut said = format!(
        "From {} session files in {}, since {}.{order} First output and rate are medians, beside \
the slowest tenth's (p90 and p10); cost is $? where the provider gives no price.",
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

/// `/usage` on the full screen: a report for each provider, the current one shown first, and
/// one of them all, shown in the conversation's place one at a time.
pub(super) struct UsageView {
    /// Each tab's name and report: every provider's first, then each provider's.
    tabs: Vec<(String, String)>,
    at: usize,
}

/// What a key did in the view.
#[derive(Debug, PartialEq)]
pub(super) enum Took {
    Moved,
    Closed,
    Nothing,
}

impl UsageView {
    /// The reports, as `report` makes them, for every provider, the current provider (`model`'s)
    /// and then the others that have responses, the costliest first.
    pub(super) fn new(
        history: &History,
        sessions: &Path,
        current: &Path,
        model: &str,
        now: u64,
        ui: &Terminal,
    ) -> Self {
        let this = provider(model);
        let mut names = vec![this.to_owned()];
        let others = grouped_by(history.responses.iter(), |r| provider(&r.model).to_owned());
        names.extend((others.into_iter().map(|(name, _)| name)).filter(|name| name != this));
        let report = |only: Option<&str>| report(history, sessions, current, model, now, only, ui);
        let mut tabs = vec![("All".to_owned(), report(None))];
        for name in names {
            let text = report(Some(&name));
            tabs.push((name, text));
        }
        Self { tabs, at: 1 }
    }

    /// The report shown.
    pub(super) fn report(&self) -> &str {
        self.tabs[self.at].1.trim_end()
    }

    pub(super) fn key(&mut self, key: &Key) -> Took {
        let count = self.tabs.len();
        if key.is(Binding::Tab) || key.is(Binding::CursorRight) {
            self.at = (self.at + 1) % count;
            Took::Moved
        } else if key.is(Binding::CursorLeft) {
            self.at = (self.at + count - 1) % count;
            Took::Moved
        } else if key.is(Binding::SelectCancel) || key.is(Binding::SelectConfirm) {
            Took::Closed
        } else {
            Took::Nothing
        }
    }

    /// The dock's rows under the report: its title with the keys, then the tabs, the one shown
    /// marked, each row within `width`.
    pub(super) fn lines(&self, width: usize) -> Vec<String> {
        use style::{MUTED, RESET, SELECTED, SELECTOR_TITLE};
        let keys = keymap::hint([
            keymap::part(&[Binding::Tab], "provider"),
            keymap::part(&[Binding::PageUp, Binding::PageDown], "scroll"),
            keymap::part(&[Binding::SelectCancel], "close"),
        ]);
        let title = format!("{SELECTOR_TITLE}Usage{RESET}  {MUTED}{keys}{RESET}");
        let tabs: Vec<String> = (self.tabs.iter().enumerate())
            .map(|(index, (name, _))| match index == self.at {
                true => format!("{SELECTED} {name} {RESET}"),
                false => format!("{MUTED} {name} {RESET}"),
            })
            .collect();
        let width = width.saturating_sub(1);
        vec![
            super::text::clip(&title, width),
            super::text::clip(&tabs.join(" "), width),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::keys::{Code, Press};
    use scramjet_rs::agent::usage::Summary;
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

    const NOW: u64 = 1_794_000_000_000;

    fn history() -> History {
        let now = NOW;
        History {
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
            summaries: vec![
                Summary {
                    root: Some("/home/me/scramjet-rs".into()),
                    title: Some("/rs:loop".into()),
                },
                Summary::default(),
            ],
            unreadable: Vec::new(),
            quotas: Vec::new(),
        }
    }

    fn ui() -> Terminal {
        Terminal::new(Arc::new(AtomicBool::new(false)), false)
    }

    #[test]
    fn the_report_adds_up_sessions_days_and_models() {
        let (now, history, ui) = (NOW, history(), ui());
        let report = report(
            &history,
            Path::new("/s"),
            Path::new("/s/p/2.jsonl"),
            "b/new",
            now,
            None,
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
        let providers = (lines.iter())
            .position(|line| *line == "Providers, last 30 days")
            .unwrap();
        assert_eq!(lines[13], "Last 12 weeks, from Mondays");
        assert_eq!(
            lines[providers - 2],
            "Bars: cost by provider: █ a  ▓ unknown"
        );
        assert_eq!(
            lines[providers..],
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
                "Costliest sessions, last 30 days",
                "Started   Requests   Tokens       Cost  Project           Session",
                &format!(
                    "{:<8}         2      900    $2.00+?  ?                 (no prompt) (this one)",
                    day(0)
                ),
                &format!(
                    "{:<8}         1      450      $1.00  scramjet-rs       /rs:loop",
                    day(40)
                ),
                "",
                lines[lines.len() - 1],
            ]
        );
        assert!(
            lines[lines.len() - 1].starts_with("From 2 session files in /s, since "),
            "{}",
            lines[lines.len() - 1]
        );
    }

    #[test]
    fn dates_count_days_from_1970_and_weeks_from_mondays() {
        assert_eq!(civil_days(1970, 1, 1), 0);
        assert_eq!(civil_days(2026, 11, 7), 20_764);
        assert_eq!(civil_date(20_764), (2026, 11, 7));
        assert_eq!(civil_date(civil_days(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil_date(-1), (1969, 12, 31));
        assert_eq!(monday(4), 4);
        assert_eq!(monday(10), 4);
        assert_eq!(monday(3), -3);
        assert_eq!(civil_date(monday(20_764)), (2026, 11, 2));
    }

    #[test]
    fn bars_split_by_model_and_count_tokens_where_nothing_is_priced() {
        let ui = ui();
        let today = local_day(NOW).unwrap();
        let week_ago = NOW - 7 * DAY;
        let mut responses: Vec<Response> = (1..=5)
            .map(|cost| response(NOW, &format!("a/m{cost}"), Some(cost as f64), 0))
            .collect();
        responses.push(response(week_ago, "a/m5", Some(15.0), 0));
        let all: Vec<&Response> = responses.iter().collect();
        let text = super::super::text::plain(&charts(&all, NOW, false, &ui));
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let earlier = day_label(local_day(week_ago).unwrap(), NOW);
        let this_week = day_label(monday(today), NOW);
        let last_week = day_label(monday(local_day(week_ago).unwrap()), NOW);
        let split = "█".repeat(7) + &"▓".repeat(5) + &"▒".repeat(4) + &"░".repeat(4);
        assert_eq!(
            lines,
            [
                "",
                "Last 14 days",
                &format!("{earlier:<8}     $15.00         1 {}", "█".repeat(20)),
                &format!("{:<8}     $15.00         5 {split}", day_label(today, NOW)),
                "",
                "Last 12 weeks, from Mondays",
                &format!("{last_week:<8}     $15.00         1 {}", "█".repeat(20)),
                &format!("{this_week:<8}     $15.00         5 {split}"),
                "Bars: cost by model: █ a/m5  ▓ a/m4  ▒ a/m3  ░ others",
            ]
        );

        let unpriced = [
            response(NOW, "c/big", None, 0),
            Response {
                input: 0,
                cache_read: 0,
                output: 150,
                ..response(NOW, "c/small", None, 0)
            },
        ];
        let all: Vec<&Response> = unpriced.iter().collect();
        let text = super::super::text::plain(&charts(&all, NOW, false, &ui));
        let half = "█".repeat(15) + &"▓".repeat(5);
        assert!(text.contains(&format!("$?         2 {half}\n")), "{text}");
        assert!(
            text.ends_with("Bars: tokens by model: █ c/big  ▓ c/small\n"),
            "{text}"
        );
    }

    #[test]
    fn a_providers_report_counts_only_its_responses() {
        let (now, history, ui) = (NOW, history(), ui());
        let current = Path::new("/s/p/2.jsonl");
        let report = report(
            &history,
            Path::new("/s"),
            current,
            "a/one",
            now,
            Some("a"),
            &ui,
        );
        let report = super::super::text::plain(&report);
        let lines: Vec<&str> = report.lines().map(str::trim_end).collect();
        assert_eq!(
            lines[..8],
            [
                "Usage of a",
                "",
                "                Requests    Input   Output  Cached       Cost",
                "This session           1      400       50     75%      $2.00",
                "Today                  1      400       50     75%      $2.00",
                "Last 7 days            2      800      100     75%      $3.00",
                "Last 30 days           2      800      100     75%      $3.00",
                "All                    3     1.2k      150     75%     $12.00",
            ]
        );
        assert!(!report.contains("Providers"), "{report}");
        assert!(
            report.contains("Models, last 30 days\nModel  Requests"),
            "{report}"
        );
        assert!(!report.contains("\nsub "), "{report}");
        assert!(
            !report.contains("The current provider comes first"),
            "{report}"
        );
    }

    #[test]
    fn a_providers_quota_shows_first_with_a_bar_for_each_window() {
        let (now, mut history, ui) = (NOW, history(), ui());
        let window = |used, minutes, resets_at| Window {
            used_percent: used,
            window_minutes: Some(minutes),
            resets_at,
            ..Window::default()
        };
        let in_two_hours = (now / 1000 + 2 * 3600) as i64;
        let quota = |windows| Quota {
            observed_at: now - 60_000,
            windows,
            credits: None,
        };
        let premium = Window {
            name: Some("Premium".into()),
            used_percent: 104.0,
            allowed: Some(300),
            over: Some(12.0),
            ..Window::default()
        };
        history.quotas = vec![
            (
                "a".into(),
                quota(vec![
                    window(41.0, 300, Some(in_two_hours)),
                    window(7.5, 10080, Some((now / 1000 - 60) as i64)),
                ]),
            ),
            ("b".into(), quota(vec![window(100.0, 43200, None)])),
            ("c".into(), quota(vec![premium])),
        ];
        let current = Path::new("/s/p/2.jsonl");
        let text = |only| {
            let text = report(&history, Path::new("/s"), current, "b/new", now, only, &ui);
            super::super::text::plain(&text)
        };
        let (seen, reset, resets) = (
            clock(now - 60_000, now),
            clock((now / 1000 - 60) * 1000, now),
            clock(now + 2 * 3600 * 1000, now),
        );
        let a = text(Some("a"));
        let lines: Vec<&str> = a.lines().collect();
        assert_eq!(
            lines[..5],
            [
                "Usage of a".to_owned(),
                String::new(),
                format!("Quota, as of {seen}"),
                format!(
                    "5h limit      {}{}  41% used, resets {resets}",
                    "█".repeat(8),
                    "░".repeat(12)
                ),
                format!(
                    "Weekly limit  {}{}   8% used, reset since {reset}",
                    "█".repeat(2),
                    "░".repeat(18)
                ),
            ]
        );
        assert_eq!(lines[5], "");
        // Every provider's, the current one first.
        let all = text(None);
        let b = all.find(&format!(
            "Quota of b, as of {seen}\nMonthly limit {} 100% used\n",
            "█".repeat(20)
        ));
        let a = all.find("Quota of a, as of");
        assert!(b.is_some_and(|b| a.is_some_and(|a| b < a)), "{all}");
        assert!(!text(Some("unknown")).contains("Quota"));
        let c = text(Some("c"));
        let premium = format!(
            "Premium limit {} 100% used (300 of 300), 12 over\n",
            "█".repeat(20)
        );
        assert!(c.contains(&premium), "{c}");
        // A model's own limit after the plan's, the column as wide as its label, then credits.
        history.quotas = vec![(
            "c".into(),
            Quota {
                credits: Some(Credits {
                    unlimited: false,
                    balance: Some("41.6".into()),
                }),
                ..quota(vec![
                    window(41.0, 300, None),
                    Window {
                        limit: Some("gpt-sonic".into()),
                        ..window(80.0, 300, None)
                    },
                ])
            },
        )];
        let c = report(
            &history,
            Path::new("/s"),
            current,
            "b/new",
            now,
            Some("c"),
            &ui,
        );
        let c = super::super::text::plain(&c);
        let rows = format!(
            "5h limit           {}{}  41% used\n\
             gpt-sonic 5h limit {}{}  80% used\n\
             Credits            42 credits\n",
            "█".repeat(8),
            "░".repeat(12),
            "█".repeat(16),
            "░".repeat(4)
        );
        assert!(c.contains(&rows), "{c}");
        let unlimited = Credits {
            unlimited: true,
            balance: None,
        };
        assert_eq!(credits_text(&unlimited), "Unlimited");
        let hidden = Credits {
            unlimited: false,
            balance: Some("0".into()),
        };
        assert_eq!(credits_text(&hidden), "Available");
    }

    #[test]
    fn the_view_opens_on_the_current_provider_and_tabs_through_the_others() {
        let (now, history, ui) = (NOW, history(), ui());
        let current = Path::new("/s/p/2.jsonl");
        let mut view = UsageView::new(&history, Path::new("/s"), current, "b/new", now, &ui);
        let names: Vec<&str> = view.tabs.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["All", "b", "a", "unknown"]);
        assert!(
            view.report()
                .starts_with("Usage of b\n\nNo responses saved yet.")
        );
        let tabs = |view: &UsageView| super::super::text::plain(&view.lines(80)[1]);
        assert_eq!(tabs(&view), " All   b   a   unknown ");
        let tab = Key::Press(Press::new(Code::Tab, 0));
        let left = Key::Press(Press::new(Code::Left, 0));
        assert_eq!(view.key(&tab), Took::Moved);
        assert!(view.report().starts_with("Usage of a\n"));
        assert_eq!(view.key(&left), Took::Moved);
        assert_eq!(view.key(&left), Took::Moved);
        assert!(view.report().starts_with("Usage\n"));
        assert_eq!(view.key(&left), Took::Moved);
        assert!(view.report().starts_with("Usage of unknown\n"));
        assert_eq!(view.key(&Key::Text("x".into())), Took::Nothing);
        let escape = Key::Press(Press::new(Code::Escape, 0));
        assert_eq!(view.key(&escape), Took::Closed);
    }
}
