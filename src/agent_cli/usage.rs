//! `/usage` (also `/cost` and `/stats`): a summary of what is left of each quota and what the
//! saved sessions spent and on what, read from their files, with its details a key away: this
//! session, today and the last days, each day of the last two weeks and each week of the last
//! twelve, each provider's and each model's requests, tokens and cost, and each model's speed.

use super::report::{ReportView, Tab};
use super::slash::grouped;
use super::style;
use super::terminal;
use super::tree::local;
use scramjet_rs::agent::quota::{Credits, Quota, Window};
use scramjet_rs::agent::usage::{History, Response, civil_days};
use std::path::Path;

const HOUR: u64 = 60 * 60 * 1000;
const DAY: u64 = 24 * HOUR;
/// The days the daily rows go back, today among them.
const DAYS: i64 = 14;
/// The weeks the weekly rows go back, this one among them.
const WEEKS: i64 = 12;
const BAR: usize = 20;
/// A quota window's bar.
const QUOTA_BAR: usize = 18;
const MODEL_WIDTH: usize = 36;
/// The sessions the costliest list shows.
const SESSIONS: usize = 10;
const PROJECT_WIDTH: usize = 16;
const TITLE_WIDTH: usize = 40;
/// The models the summary names.
const TOP_MODELS: usize = 3;

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
fn charts(responses: &[&Response], now: u64, by_provider: bool) -> String {
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
                style::mark(style::MUTED, &format!("{:<8}", day_label(*from, now))),
                totals.cost(),
                grouped(totals.requests),
                style::mark(style::USAGE_BAR, &bar),
            );
        }
        if rows.is_empty() {
            text += &style::mark(style::MUTED, "No responses.");
            text.push('\n');
        }
        text
    };

    let heading = |text: &str| format!("\n{}\n", style::mark(style::INFO_HEADING, text));
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
                let shade = style::mark(style::USAGE_BAR, &shade.to_string());
                format!("{shade} {}", style::mark(style::MUTED, name))
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
        text += &style::mark(style::MUTED, &format!("Bars: {measure} by {split}: "));
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

/// When `window` started, in milliseconds since 1970: its length before it resets, a week for
/// Copilot's weekly limit and a calendar month for its monthly requests (reset on a month's
/// first day); none when its length is not known.
fn window_start(window: &Window) -> Option<i64> {
    const DAY: i64 = 86_400_000;
    let resets = window.resets_at?.checked_mul(1000)?;
    match (window.window_minutes, window.name.as_deref()) {
        (Some(minutes), _) if minutes > 0 => Some(resets - minutes * 60_000),
        (None, Some("Weekly")) => Some(resets - 7 * DAY),
        (None, Some("Premium" | "Chat")) => {
            let (day, time) = (resets.div_euclid(DAY), resets.rem_euclid(DAY));
            let (year, month, date) = civil_date(day);
            let (year, month) = if month == 1 {
                (year - 1, 12)
            } else {
                (year, month - 1)
            };
            let length = civil_days(year + i32::from(month == 12), month % 12 + 1, 1)
                - civil_days(year, month, 1);
            Some(civil_days(year, month, date.min(length as i32)) * DAY + time)
        }
        _ => None,
    }
}

/// How a window goes at the pace it was used from its start up to when it was seen.
#[derive(Debug, PartialEq)]
enum Pace {
    Lasts,
    /// When it runs out, in milliseconds since 1970.
    RunsOut(u64),
}

/// How `window` goes if it goes on being used at the pace it was used from its start up to
/// `observed_at`; none when it is used up, it reset since, too little of it has passed to tell
/// a pace, or its start is not known.
fn pace(window: &Window, observed_at: u64) -> Option<Pace> {
    let used = window.used_percent;
    let start = window_start(window)?;
    let resets = window.resets_at? * 1000;
    let (observed, length) = (observed_at as i64, resets - start);
    let elapsed = observed - start;
    if !(0.0..100.0).contains(&used) || observed >= resets || elapsed < length / 20 {
        return None;
    }
    let out = start + (elapsed as f64 * 100.0 / used.max(f64::MIN_POSITIVE)).min(4e15) as i64;
    Some(match out < resets {
        true => Pace::RunsOut(out.max(0) as u64),
        false => Pace::Lasts,
    })
}

/// What a window of `minutes` is called: Codex's five-hour and weekly ones are `5-hour` and
/// `Weekly`.
fn window_label(minutes: Option<i64>) -> String {
    const DAY: i64 = 24 * 60;
    match minutes {
        Some(m) if m == DAY => "Daily".to_owned(),
        Some(m) if m == 7 * DAY => "Weekly".to_owned(),
        Some(m) if (28 * DAY..=31 * DAY).contains(&m) => "Monthly".to_owned(),
        Some(m) if m > 0 && m % DAY == 0 => format!("{}-day", m / DAY),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}-hour", m / 60),
        Some(m) if m > 0 => format!("{m}-minute"),
        _ => "Usage".to_owned(),
    }
}

/// What a window that lasts at its pace lasts: `the week` for a weekly one.
fn span(label: &str) -> &'static str {
    match label {
        "Weekly" => "the week",
        "Monthly" | "Premium" | "Chat" => "the month",
        "Daily" => "the day",
        _ => "until it resets",
    }
}

/// When `at` comes, from `now`: `in 2h 10m` within a day, else its weekday and time within a
/// week, else its date.
fn until(at: u64, now: u64) -> String {
    let minutes = at.saturating_sub(now).div_ceil(60_000);
    if minutes < 60 {
        return format!("in {minutes}m");
    }
    if minutes < 24 * 60 {
        return match minutes % 60 {
            0 => format!("in {}h", minutes / 60),
            rest => format!("in {}h {rest}m", minutes / 60),
        };
    }
    match (local(at), local_day(at)) {
        (Some((.., hour, minute)), Some(day)) if at - now < 7 * DAY => {
            format!(
                "{} {hour:02}:{minute:02}",
                WEEKDAYS[(day + 4).rem_euclid(7) as usize]
            )
        }
        _ => date(at, now),
    }
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// A quota seen longer ago than this says when it was seen.
const STALE: u64 = 15 * 60 * 1000;

/// The quota of `only`, or of every provider, the current one (`current`) first, each as its
/// latest response reported it: a bar for each window, each provider named when all are shown.
fn quotas(history: &History, only: Option<&str>, current: &str, now: u64) -> String {
    let mut shown: Vec<&(String, Quota)> = (history.quotas.iter())
        .filter(|(provider, _)| only.is_none_or(|only| provider == only))
        .collect();
    shown.sort_by_key(|(provider, _)| (provider != current, provider.clone()));
    let mut text = String::new();
    for (provider, quota) in shown {
        let seen = match now.saturating_sub(quota.observed_at) > STALE {
            true => Some(format!("as of {}", clock(quota.observed_at, now))),
            false => None,
        };
        match (only, seen) {
            (None, seen) => {
                let seen = seen.map_or(String::new(), |seen| {
                    style::mark(style::MUTED, &format!(" · {seen}"))
                });
                text += &format!("  {}{seen}\n", style::mark(style::INFO_HEADING, provider));
            }
            (Some(_), Some(seen)) => text += &format!("  {}\n", style::mark(style::MUTED, &seen)),
            (Some(_), None) => {}
        }
        text += &quota_rows(quota, now);
        text.push('\n');
    }
    text
}

/// The quota of `model`'s provider for `/status`, as its latest response reported it: a heading,
/// then `/usage`'s rows; none when no response of it reported one.
pub(super) fn status_quota(quotas: &[(String, Quota)], model: &str, now: u64) -> Option<String> {
    let (_, quota) = quotas.iter().find(|(name, _)| name == provider(model))?;
    let title = format!("Quota, as of {}", clock(quota.observed_at, now));
    Some(format!("{}\n", style::mark(style::INFO_HEADING, &title)) + &quota_rows(quota, now))
}

/// A quota's rows, each window with its bar, how much of it is used, when it resets and whether
/// it lasts at its pace, then its credits.
fn quota_rows(quota: &Quota, now: u64) -> String {
    let mut text = String::new();
    let labels: Vec<String> = (quota.windows.iter())
        .map(|window| (window.name.clone()).unwrap_or_else(|| window_label(window.window_minutes)))
        .collect();
    let width = (labels.iter())
        .map(|label| label.chars().count())
        .chain(quota.credits.as_ref().map(|_| "Credits".len()))
        .fold(8, usize::max);
    for (window, label) in quota.windows.iter().zip(labels) {
        let used = window.used_percent.clamp(0.0, 100.0);
        let filled = ((used / 100.0 * QUOTA_BAR as f64).round() as usize).min(QUOTA_BAR);
        let bar = style::mark(style::USAGE_BAR, &"█".repeat(filled))
            + &style::mark(style::MUTED, &"░".repeat(QUOTA_BAR - filled));
        let count = match (window.allowed, window.over) {
            (Some(allowed), over) => {
                let made = (allowed as f64 * used / 100.0).round();
                let over = over.map_or(String::new(), |over| format!(", {over} over"));
                format!(" ({made} of {allowed}{over})")
            }
            (None, Some(over)) => format!(" ({over} over)"),
            (None, None) => String::new(),
        };
        // A model's own limit, after the plan's, is named by its model.
        let mut said: Vec<String> = window.limit.iter().cloned().collect();
        match window.resets_at.map(|at| at.max(0) as u64 * 1000) {
            // What was used then no longer counts.
            Some(at) if at <= now => said.push(style::mark(
                style::MUTED,
                &format!("reset since {}", clock(at, now)),
            )),
            Some(at) => said.push(format!("resets {}", until(at, now))),
            None => {}
        }
        match pace(window, quota.observed_at) {
            Some(Pace::RunsOut(out)) if out > now => {
                let text = format!("runs out {} at this pace", until(out, now));
                said.push(style::mark(style::WARNING, &text));
            }
            Some(Pace::Lasts) if window.resets_at.is_some_and(|at| at as u64 * 1000 > now) => {
                said.push(style::mark(
                    style::MUTED,
                    &format!("on pace to last {}", span(&label)),
                ));
            }
            _ => {}
        }
        let said = match said.is_empty() {
            true => String::new(),
            false => format!("   {}", said.join(&style::mark(style::MUTED, " · "))),
        };
        text += &format!(
            "  {}  {bar}  {used:>3.0}%{count}{said}\n",
            style::mark(style::MUTED, &format!("{label:<width$}")),
        );
    }
    if let Some(credits) = &quota.credits {
        text += &format!(
            "  {}  {}\n",
            style::mark(style::MUTED, &format!("{:<width$}", "Credits")),
            credits_text(credits)
        );
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

/// Each session's tokens (input with the cache's, and output) under which a row of `sizes`
/// counts it, the last row counting the rest.
const SIZES: [(u64, &str); 5] = [
    (10_000, "Under 10k"),
    (100_000, "10k to 100k"),
    (1_000_000, "100k to 1M"),
    (10_000_000, "1M to 10M"),
    (u64::MAX, "10M and more"),
];

/// How many of `sessions` used how many tokens, a row for each size from the smallest to the
/// largest used, with their cost and a bar of their count.
fn sizes(sessions: &[(String, Totals)]) -> String {
    let mut rows: Vec<(u64, Totals)> = SIZES.iter().map(|_| (0, Totals::default())).collect();
    for (_, totals) in sessions {
        let size = (SIZES.iter())
            .position(|&(under, _)| totals.input + totals.output < under)
            .unwrap_or(SIZES.len() - 1);
        let (count, sum) = &mut rows[size];
        *count += 1;
        sum.cost += totals.cost;
        sum.unpriced |= totals.unpriced;
    }
    let first = rows.iter().position(|(count, _)| *count > 0).unwrap_or(0);
    let last = rows.iter().rposition(|(count, _)| *count > 0).unwrap_or(0);
    let most = rows
        .iter()
        .map(|(count, _)| *count)
        .max()
        .unwrap_or(0)
        .max(1);
    let mut text = style::mark(
        style::MUTED,
        &format!("{:<14}{:>10}{:>11}", "Tokens", "Sessions", "Cost"),
    );
    text.push('\n');
    for ((_, label), (count, totals)) in SIZES.iter().zip(&rows).take(last + 1).skip(first) {
        let width = match *count {
            0 => 0,
            count => ((count * BAR as u64) as f64 / most as f64).round().max(1.0) as usize,
        };
        let bar = style::mark(style::USAGE_BAR, &"█".repeat(width));
        text += &format!(
            "{}{:>10}{:>11} {bar}\n",
            style::mark(style::MUTED, &format!("{label:<14}")),
            grouped(*count),
            totals.cost(),
        );
    }
    text
}

/// `model` without its provider, unknown for none.
fn bare(model: &str) -> &str {
    match model.split_once('/').map_or(model, |(_, id)| id) {
        "" => "unknown",
        id => id,
    }
}

/// `at`'s day said from `now`'s: today, yesterday, a weekday within the week, else a date.
fn day_said(at: u64, now: u64) -> String {
    match (local_day(at), local_day(now)) {
        (Some(day), Some(today)) if day == today => "today".to_owned(),
        (Some(day), Some(today)) if day + 1 == today => "yesterday".to_owned(),
        (Some(day), Some(today)) if day < today && today - day < 7 => {
            format!("on {}", WEEKDAYS[(day + 4).rem_euclid(7) as usize])
        }
        _ => format!("on {}", date(at, now)),
    }
}

/// The summary `/usage` opens on, `current` being this session's file and `model` its model, at
/// `now` (milliseconds since 1970), of `only`'s responses or of every provider's: what is left of
/// each quota, what was spent today, this week and in the last 30 days, the models most of it
/// went to and the costliest session. Spend is in dollars, or in tokens where nothing is priced.
pub(super) fn summary(
    history: &History,
    current: &Path,
    model: &str,
    now: u64,
    only: Option<&str>,
) -> String {
    let title = format!("Usage · {}", only.unwrap_or("All"));
    let mut text = format!("{}\n\n", style::mark(style::INFO_HEADING, &title));
    text += &quotas(history, only, provider(model), now);
    let responses: Vec<&Response> = (history.responses.iter())
        .filter(|r| only.is_none_or(|only| provider(&r.model) == only))
        .collect();
    if responses.is_empty() {
        text += &format!(
            "  {}\n",
            style::mark(style::MUTED, "No responses saved yet.")
        );
        return text;
    }
    let priced = (responses.iter()).any(|r| r.cost.is_some_and(|cost| cost > 0.0));
    let measure = |r: &Response| match priced {
        true => r.cost.unwrap_or(0.0),
        false => (r.input + r.cache_read + r.cache_write + r.output) as f64,
    };
    let amount = |totals: &Totals| match priced {
        true => totals.cost(),
        false => format!("{} tokens", terminal::tokens(totals.input + totals.output)),
    };
    let within = |from: u64, to: u64| {
        Totals::of((responses.iter().copied()).filter(move |r| from <= r.at && r.at < to))
    };
    let today = local_day(now).unwrap_or(0);
    let today_totals = Totals::of((responses.iter().copied()).filter(|r| same_day(r.at, now)));
    let week = Totals::of(
        (responses.iter().copied()).filter(|r| local_day(r.at).is_some_and(|d| d >= monday(today))),
    );
    let month_from = now.saturating_sub(30 * DAY);
    let month = within(month_from, u64::MAX);
    let before = within(now.saturating_sub(60 * DAY), month_from);
    let size = |totals: &Totals| match priced {
        true => totals.cost,
        false => (totals.input + totals.output) as f64,
    };
    let change = match (size(&month), size(&before)) {
        (_, before) if before <= 0.0 => String::new(),
        (month, before) => {
            let change = ((month - before) / before * 100.0).round();
            let said = match change {
                0.0 => "the same as the 30 before".to_owned(),
                up if up > 0.0 => format!("up {up:.0}% on the 30 before"),
                down => format!("down {:.0}% on the 30 before", -down),
            };
            style::mark(style::MUTED, &format!("   ({said})"))
        }
    };
    let spent = [
        ("Today", &today_totals),
        ("This week", &week),
        ("Last 30 days", &month),
    ]
    .map(|(label, totals)| format!("{} {}", style::mark(style::MUTED, label), amount(totals)));
    text += &format!("  {}{change}\n\n", spent.join("   "));

    let label = |label: &str| style::mark(style::MUTED, &format!("{label:<21}"));
    let recent: Vec<&Response> = (responses.iter().copied())
        .filter(|r| r.at >= month_from)
        .collect();
    let all: f64 = recent.iter().map(|r| measure(r)).sum();
    let mut models: Vec<(&str, f64, Totals)> = Vec::new();
    for response in &recent {
        let name = bare(&response.model);
        match models.iter_mut().find(|(model, ..)| *model == name) {
            Some((_, size, totals)) => {
                *size += measure(response);
                totals.add(response);
            }
            None => models.push((name, measure(response), Totals::of([*response].into_iter()))),
        }
    }
    models.sort_by(|a, b| b.1.total_cmp(&a.1));
    let shares: Vec<String> = (models.iter())
        .filter(|(_, size, _)| all > 0.0 && size / all >= 0.01)
        .take(TOP_MODELS)
        .map(|(name, size, totals)| {
            let share = style::mark(style::MUTED, &format!("({:.0}%)", size / all * 100.0));
            format!("{name}  {} {share}", amount(totals))
        })
        .collect();
    if !shares.is_empty() {
        let went = match models.len() {
            1 => "All of it went to",
            _ => "Most of it went to",
        };
        text += &format!("  {}{}\n", label(went), shares.join("   "));
    }

    let this = history.session(current);
    let here = this
        .and_then(|this| history.summaries.get(this))
        .and_then(|summary| summary.root.as_deref());
    let mut sessions: Vec<(usize, f64, Totals, u64)> = Vec::new();
    for response in &recent {
        match sessions
            .iter_mut()
            .find(|(session, ..)| *session == response.session)
        {
            Some((_, size, totals, started)) => {
                *size += measure(response);
                totals.add(response);
                *started = (*started).min(response.at);
            }
            None => sessions.push((
                response.session,
                measure(response),
                Totals::of([*response].into_iter()),
                response.at,
            )),
        }
    }
    let costliest = (sessions.iter()).max_by(|a, b| a.1.total_cmp(&b.1).then(b.3.cmp(&a.3)));
    if let Some((index, _, totals, started)) = costliest.filter(|(_, size, ..)| *size > 0.0) {
        let summary = history.summaries.get(*index);
        let name = match summary.and_then(|summary| summary.title.as_deref()) {
            Some(title) => format!("\"{}\"", super::text::clip(title, TITLE_WIDTH)),
            None => style::mark(style::MUTED, "(no prompt)"),
        };
        let root = summary.and_then(|summary| summary.root.as_deref());
        let place = match root
            .filter(|&root| Some(root) != here)
            .and_then(Path::file_name)
        {
            Some(project) => format!(" in {}", project.to_string_lossy()),
            None => String::new(),
        };
        let when = match Some(*index) == this {
            true => "this session".to_owned(),
            false => day_said(*started, now),
        };
        text += &format!(
            "  {}{name}{place} {when}, {}\n",
            label("Costliest session"),
            amount(totals),
        );
    }
    text
}

/// The details behind the summary, `current` being this session's file and `model` its model,
/// at `now` (milliseconds since 1970): of `only`'s responses, or of every provider's.
pub(super) fn details(
    history: &History,
    sessions: &Path,
    current: &Path,
    model: &str,
    now: u64,
    only: Option<&str>,
) -> String {
    let heading = |text: &str| format!("{}\n", style::mark(style::INFO_HEADING, text));
    let responses: Vec<&Response> = (history.responses.iter())
        .filter(|r| only.is_none_or(|only| provider(&r.model) == only))
        .collect();
    let title = format!("Usage · {} · details", only.unwrap_or("All"));
    let mut text = heading(&title) + "\n";
    if responses.is_empty() {
        text += &style::mark(style::MUTED, "No responses saved yet.");
        text.push('\n');
        return text;
    }
    let row = |label: &str, totals: &Totals| {
        format!(
            "{}{:>10}{:>9}{:>9}{:>8}{:>11}\n",
            style::mark(style::MUTED, &format!("{label:<14}")),
            grouped(totals.requests),
            terminal::tokens(totals.input),
            terminal::tokens(totals.output),
            totals.cached(),
            totals.cost(),
        )
    };
    text += &style::mark(
        style::MUTED,
        &format!(
            "{:<14}{:>10}{:>9}{:>9}{:>8}{:>11}",
            "", "Requests", "Input", "Output", "Cached", "Cost"
        ),
    );
    text.push('\n');
    let session = history.session(current);
    let this = Totals::of(
        responses
            .iter()
            .copied()
            .filter(|r| Some(r.session) == session),
    );
    text += &row("This session", &this);
    for (hours, label) in [(1, "Last hour"), (5, "Last 5 hours")] {
        let since = now.saturating_sub(hours * HOUR);
        let totals = Totals::of(responses.iter().copied().filter(|r| r.at >= since));
        text += &row(label, &totals);
    }
    let today = Totals::of(responses.iter().copied().filter(|r| same_day(r.at, now)));
    text += &row("Today", &today);
    for days in [7, 30] {
        let since = now.saturating_sub(days * DAY);
        let totals = Totals::of(responses.iter().copied().filter(|r| r.at >= since));
        text += &row(&format!("Last {days} days"), &totals);
    }
    text += &row("All", &Totals::of(responses.iter().copied()));

    text += &charts(&responses, now, only.is_none());

    let since = now.saturating_sub(30 * DAY);
    let recent = || responses.iter().copied().filter(|r| r.at >= since);
    let table = |label: &str, rows: &[(String, Totals)]| {
        let width = (rows.iter().map(|(name, _)| name.chars().count()))
            .chain([label.len()])
            .max()
            .unwrap_or(0)
            .min(MODEL_WIDTH);
        let mut text = style::mark(
            style::MUTED,
            &format!(
                "{label:<width$}{:>10}{:>9}{:>9}{:>8}{:>11}",
                "Requests", "Input", "Output", "Cached", "Cost"
            ),
        );
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
            text += &style::mark(style::MUTED, "No responses.");
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
        text += &style::mark(
            style::MUTED,
            &format!(
                "{:<width$}{:>8} {:>12}{:>8} {:>12}{:>12}",
                "Model", "Timed", "First output", "p90", "Rate", "p10"
            ),
        );
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
    if !costliest.is_empty() {
        text += "\n";
        text += &heading("Sessions by tokens, last 30 days");
        text += &sizes(&costliest);
    }
    costliest.truncate(SESSIONS);
    if !costliest.is_empty() {
        text += "\n";
        text += &heading("Costliest sessions, last 30 days");
        text += &style::mark(
            style::MUTED,
            &format!(
                "{:<8}{:>10}{:>9}{:>11}  {:<PROJECT_WIDTH$}  Session",
                "Started", "Requests", "Tokens", "Cost", "Project"
            ),
        );
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
                None => style::mark(style::MUTED, "(no prompt)"),
            };
            let mark = match Some(index) == session {
                true => style::mark(style::MUTED, " (this one)"),
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
    text += &style::mark(style::MUTED, &said);
    text.push('\n');
    text
}

/// `/usage` on the full screen: a summary for each provider, the current one shown first, and
/// one of them all, shown in the conversation's place one at a time, each with its details a key
/// away: the current provider (`model`'s) and then the others that have responses, the costliest
/// first; `details` shows the details first.
pub(super) fn view(
    history: &History,
    sessions: &Path,
    current: &Path,
    model: &str,
    now: u64,
    details: bool,
) -> ReportView {
    let this = provider(model);
    let mut names = vec![None, Some(this.to_owned())];
    let others = grouped_by(history.responses.iter(), |r| provider(&r.model).to_owned());
    names.extend(
        (others.into_iter().map(|(name, _)| Some(name)))
            .filter(|name| name.as_deref() != Some(this)),
    );
    let tabs = (names.into_iter())
        .map(|name| {
            let only = name.as_deref();
            Tab {
                summary: summary(history, current, model, now, only),
                details: self::details(history, sessions, current, model, now, only),
                name: name.unwrap_or_else(|| "All".to_owned()),
            }
        })
        .collect();
    ReportView::new("Usage", "summary", tabs, 1, details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::keys::{Code, Key, Press};
    use crate::agent_cli::report::Took;
    use scramjet_rs::agent::usage::Summary;

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

    fn plain(text: &str) -> String {
        super::super::text::plain(&style::themed(text, false))
    }

    #[test]
    fn the_details_add_up_sessions_days_and_models() {
        let (now, history) = (NOW, history());
        let report = details(
            &history,
            Path::new("/s"),
            Path::new("/s/p/2.jsonl"),
            "b/new",
            now,
            None,
        );
        let report = plain(&report);
        let lines: Vec<&str> = report.lines().map(str::trim_end).collect();
        let day = |back: u64| date(now - back * DAY, now);
        assert_eq!(
            lines[..11],
            [
                "Usage · All · details",
                "",
                "                Requests    Input   Output  Cached       Cost",
                "This session           2      800      100     75%    $2.00+?",
                "Last hour              2      800      100     75%    $2.00+?",
                "Last 5 hours           2      800      100     75%    $2.00+?",
                "Today                  2      800      100     75%    $2.00+?",
                "Last 7 days            3     1.2k      150     75%    $3.00+?",
                "Last 30 days           3     1.2k      150     75%    $3.00+?",
                "All                    4     1.6k      200     75%   $12.00+?",
                "",
            ]
        );
        assert_eq!(lines[11], "Last 14 days");
        assert_eq!(
            lines[12],
            format!("{:<8}      $1.00         1 {}", day(3), "█".repeat(10))
        );
        assert_eq!(
            lines[13],
            format!("{:<8}    $2.00+?         2 {}", day(0), "█".repeat(20))
        );
        let providers = (lines.iter())
            .position(|line| *line == "Providers, last 30 days")
            .unwrap();
        assert_eq!(lines[15], "Last 12 weeks, from Mondays");
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
                "Sessions by tokens, last 30 days",
                "Tokens          Sessions       Cost",
                &format!("Under 10k              2    $3.00+? {}", "█".repeat(20)),
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
    fn sessions_are_counted_by_their_tokens_from_the_smallest_size_to_the_largest() {
        let session = |tokens: u64, cost: Option<f64>| {
            let response = Response {
                input: tokens,
                output: 0,
                cache_read: 0,
                ..response(NOW, "a/m", cost, 0)
            };
            Totals::of([&response].into_iter())
        };
        let sessions: Vec<(String, Totals)> = [
            (20_000, Some(0.5)),
            (99_999, Some(1.0)),
            (100_000, None),
            (5_000_000, Some(4.0)),
            (40_000, Some(0.25)),
            (60_000, Some(0.25)),
            (70_000, Some(0.25)),
            (80_000, Some(0.25)),
            (90_000, Some(0.25)),
            (95_000, Some(0.25)),
            (96_000, Some(0.25)),
            (97_000, Some(0.25)),
            (98_000, Some(0.25)),
            (98_500, Some(0.25)),
            (98_600, Some(0.25)),
            (98_700, Some(0.25)),
            (98_800, Some(0.25)),
            (98_900, Some(0.25)),
            (98_950, Some(0.25)),
            (98_990, Some(0.25)),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (tokens, cost))| (index.to_string(), session(tokens, cost)))
        .collect();
        let text = plain(&sizes(&sessions));
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        assert_eq!(
            lines,
            [
                "Tokens          Sessions       Cost",
                &format!("10k to 100k           18      $5.50 {}", "█".repeat(20)),
                "100k to 1M             1         $? █",
                "1M to 10M              1      $4.00 █",
            ]
        );

        let one = [("0".to_owned(), session(10_000_000, Some(1.0)))];
        let text = plain(&sizes(&one));
        assert!(
            text.ends_with("10M and more           1      $1.00 ████████████████████\n"),
            "{text}"
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
        let today = local_day(NOW).unwrap();
        let week_ago = NOW - 7 * DAY;
        let mut responses: Vec<Response> = (1..=5)
            .map(|cost| response(NOW, &format!("a/m{cost}"), Some(cost as f64), 0))
            .collect();
        responses.push(response(week_ago, "a/m5", Some(15.0), 0));
        let all: Vec<&Response> = responses.iter().collect();
        let text = plain(&charts(&all, NOW, false));
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
        let text = plain(&charts(&all, NOW, false));
        let half = "█".repeat(15) + &"▓".repeat(5);
        assert!(text.contains(&format!("$?         2 {half}\n")), "{text}");
        assert!(
            text.ends_with("Bars: tokens by model: █ c/big  ▓ c/small\n"),
            "{text}"
        );
    }

    #[test]
    fn a_providers_details_count_only_its_responses() {
        let (now, history) = (NOW, history());
        let current = Path::new("/s/p/2.jsonl");
        let report = details(&history, Path::new("/s"), current, "a/one", now, Some("a"));
        let report = plain(&report);
        let lines: Vec<&str> = report.lines().map(str::trim_end).collect();
        assert_eq!(
            lines[..10],
            [
                "Usage · a · details",
                "",
                "                Requests    Input   Output  Cached       Cost",
                "This session           1      400       50     75%      $2.00",
                "Last hour              1      400       50     75%      $2.00",
                "Last 5 hours           1      400       50     75%      $2.00",
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

    fn lines(text: &str) -> Vec<String> {
        let text = plain(text);
        text.lines()
            .map(|line| line.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn the_summary_says_what_was_spent_and_what_drove_it() {
        let (now, mut history) = (NOW, history());
        let current = Path::new("/s/p/2.jsonl");
        let three_days = now - 3 * DAY;
        let this_week = local_day(three_days).unwrap() >= monday(local_day(now).unwrap());
        let week = |with: &str, without: &str| match this_week {
            true => with.to_owned(),
            false => without.to_owned(),
        };
        assert_eq!(
            lines(&summary(&history, current, "b/new", now, None)),
            [
                "Usage · All".to_owned(),
                String::new(),
                format!(
                    "  Today $2.00+?   This week {}   Last 30 days $3.00+?   (down 67% on the 30 before)",
                    week("$3.00+?", "$2.00+?")
                ),
                String::new(),
                "  Most of it went to   one  $3.00 (100%)".to_owned(),
                "  Costliest session    (no prompt) this session, $2.00+?".to_owned(),
            ]
        );
        // Another session costlier, from another project.
        history
            .responses
            .push(response(three_days, "a/one", Some(5.0), 0));
        let a = lines(&summary(&history, current, "a/one", now, Some("a")));
        assert_eq!(a[0], "Usage · a");
        assert_eq!(
            a[2],
            format!(
                "  Today $2.00   This week {}   Last 30 days $8.00   (down 11% on the 30 before)",
                week("$8.00", "$2.00")
            )
        );
        assert_eq!(a[4], "  All of it went to    one  $8.00 (100%)");
        assert_eq!(
            a[5],
            format!(
                "  Costliest session    \"/rs:loop\" in scramjet-rs {}, $6.00",
                day_said(three_days, now)
            )
        );
        // Nothing priced: tokens.
        assert_eq!(
            lines(&summary(&history, current, "b/new", now, Some("unknown")))[2..],
            [
                "  Today 450 tokens   This week 450 tokens   Last 30 days 450 tokens",
                "",
                "  All of it went to    sub  450 tokens (100%)",
                "  Costliest session    (no prompt) this session, 450 tokens",
            ]
        );
        assert_eq!(
            lines(&summary(&history, current, "b/new", now, Some("b"))),
            ["Usage · b", "", "  No responses saved yet."]
        );
        assert_eq!(day_said(now, now), "today");
        assert_eq!(day_said(now - DAY, now), "yesterday");
        assert!(day_said(three_days, now).starts_with("on "));
    }

    #[test]
    fn each_quota_window_is_a_bar_with_its_reset_and_pace() {
        let (now, mut history) = (NOW, history());
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
        let text = |history: &History, only| lines(&summary(history, current, "b/new", now, only));
        let bar = |filled: usize| "█".repeat(filled) + &"░".repeat(QUOTA_BAR - filled);
        let reset = clock((now / 1000 - 60) * 1000, now);
        let a = text(&history, Some("a"));
        assert_eq!(
            a[..5],
            [
                "Usage · a".to_owned(),
                String::new(),
                format!(
                    "  5-hour    {}   41%   resets in 2h · on pace to last until it resets",
                    bar(7)
                ),
                format!("  Weekly    {}    8%   reset since {reset}", bar(1)),
                String::new(),
            ]
        );
        // Every provider's, each named, the current one first.
        let all = text(&history, None).join("\n");
        let b = all.find(&format!("  b\n  Monthly   {}  100%\n", bar(18)));
        let a = all.find("  a\n  5-hour");
        assert!(b.is_some_and(|b| a.is_some_and(|a| b < a)), "{all}");
        assert!(!text(&history, Some("unknown")).join("\n").contains('█'));
        let c = text(&history, Some("c"));
        assert_eq!(
            c[2],
            format!("  Premium   {}  100% (300 of 300, 12 over)", bar(18))
        );
        // A model's own limit, named after the bar; credits; a quota seen a while ago says when.
        history.quotas = vec![(
            "c".into(),
            Quota {
                observed_at: now - 20 * 60_000,
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
        let seen = clock(now - 20 * 60_000, now);
        let rows = [
            format!("  5-hour    {}   41%", bar(7)),
            format!("  5-hour    {}   80%   gpt-sonic", bar(14)),
            "  Credits   42 credits".to_owned(),
        ];
        assert_eq!(
            text(&history, Some("c"))[2..6],
            [&[format!("  as of {seen}")], &rows[..]].concat()
        );
        // `/status` shows the current model's provider's, with the same rows.
        let status = status_quota(&history.quotas, "c/gpt-sonic", now).unwrap();
        assert_eq!(
            lines(&status),
            [&[format!("Quota, as of {seen}")], &rows[..]].concat()
        );
        assert_eq!(status_quota(&history.quotas, "b/new", now), None);
        assert_eq!(status_quota(&history.quotas, "unsaved", now), None);
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
    fn resets_are_said_in_hours_within_a_day_then_by_weekday_then_by_date() {
        let now = NOW;
        assert_eq!(until(now + 42 * 60_000, now), "in 42m");
        assert_eq!(until(now + 130 * 60_000 - 500, now), "in 2h 10m");
        assert_eq!(until(now + 5 * HOUR, now), "in 5h");
        let in_two_days = until(now + 2 * DAY, now);
        assert!(
            WEEKDAYS.iter().any(|day| in_two_days.starts_with(day)),
            "{in_two_days}"
        );
        assert_eq!(until(now + 20 * DAY, now), date(now + 20 * DAY, now));
        assert_eq!(window_label(Some(300)), "5-hour");
        assert_eq!(window_label(Some(10080)), "Weekly");
        assert_eq!(window_label(Some(90)), "90-minute");
        assert_eq!(span("Weekly"), "the week");
        assert_eq!(span("Premium"), "the month");
    }

    #[test]
    fn a_window_used_faster_than_it_lasts_says_when_it_runs_out() {
        let (now, mut history) = (NOW, history());
        let seconds = |ms: u64| Some((ms / 1000) as i64);
        // Three hours of five gone, 80% used: at that pace 100% after 3h45m.
        let fast = Window {
            used_percent: 80.0,
            window_minutes: Some(300),
            resets_at: seconds(now + 2 * 3_600_000),
            ..Window::default()
        };
        let out = now + 45 * 60_000;
        assert_eq!(pace(&fast, now), Some(Pace::RunsOut(out)));
        let slow = Window {
            used_percent: 50.0,
            ..fast.clone()
        };
        assert_eq!(pace(&slow, now), Some(Pace::Lasts));
        let unused = Window {
            used_percent: 0.0,
            ..fast.clone()
        };
        assert_eq!(pace(&unused, now), Some(Pace::Lasts));
        let early = Window {
            resets_at: seconds(now + 299 * 60_000),
            ..fast.clone()
        };
        assert_eq!(pace(&early, now), None, "a minute is too little");
        let unknown = Window {
            window_minutes: None,
            ..fast.clone()
        };
        assert_eq!(pace(&unknown, now), None);
        let full = Window {
            used_percent: 100.0,
            ..fast.clone()
        };
        assert_eq!(pace(&full, now), None);
        // Copilot's monthly requests start on the first day of the month before their reset.
        let monthly = |reset: (i32, i32, i32)| Window {
            name: Some("Premium".into()),
            resets_at: Some(civil_days(reset.0, reset.1, reset.2) * 86_400),
            ..Window::default()
        };
        let start = |reset| window_start(&monthly(reset)).map(|ms| civil_date(ms / 86_400_000));
        assert_eq!(start((2026, 12, 1)), Some((2026, 11, 1)));
        assert_eq!(start((2027, 1, 1)), Some((2026, 12, 1)));
        assert_eq!(start((2026, 3, 31)), Some((2026, 2, 28)));
        let weekly = Window {
            name: Some("Weekly".into()),
            resets_at: Some(700_000),
            ..Window::default()
        };
        assert_eq!(window_start(&weekly), Some((700_000 - 7 * 86_400) * 1000));
        // The row says it, unless that time has passed since the response that reported it.
        let quota = |observed_at| Quota {
            observed_at,
            windows: vec![fast.clone()],
            credits: None,
        };
        let current = Path::new("/s/p/2.jsonl");
        let row = |history: &History| {
            let text = summary(history, current, "b/new", now, Some("a"));
            lines(&text)
                .into_iter()
                .find(|line| line.contains("5-hour"))
                .unwrap_or_default()
        };
        history.quotas = vec![("a".into(), quota(now))];
        assert!(
            row(&history).ends_with("resets in 2h · runs out in 45m at this pace"),
            "{}",
            row(&history)
        );
        history.quotas = vec![("a".into(), quota(now - 50 * 60_000))];
        assert!(row(&history).ends_with("resets in 2h"), "{}", row(&history));
    }

    #[test]
    fn the_view_opens_on_the_current_provider_and_tabs_through_the_others() {
        let (now, history) = (NOW, history());
        let current = Path::new("/s/p/2.jsonl");
        let mut view = view(&history, Path::new("/s"), current, "b/new", now, false);
        let names: Vec<&str> = view.tabs.iter().map(|tab| tab.name.as_str()).collect();
        assert_eq!(names, ["All", "b", "a", "unknown"]);
        assert_eq!(
            lines(view.report()),
            ["Usage · b", "", "  No responses saved yet."]
        );
        let tabs = |view: &ReportView| plain(&view.lines(80)[1]);
        let keys = |view: &ReportView| plain(&view.lines(80)[0]);
        assert_eq!(tabs(&view), " All   b   a   unknown ");
        assert!(
            keys(&view).starts_with("Usage  d details · "),
            "{}",
            keys(&view)
        );
        let tab = Key::Press(Press::new(Code::Tab, 0));
        let left = Key::Press(Press::new(Code::Left, 0));
        let details = Key::Text("d".into());
        let shown = |view: &ReportView| lines(view.report())[0].clone();
        assert_eq!(view.key(&tab), Took::Moved);
        assert_eq!(shown(&view), "Usage · a");
        assert_eq!(view.key(&details), Took::Moved);
        assert_eq!(shown(&view), "Usage · a · details");
        assert!(
            keys(&view).starts_with("Usage  d summary · "),
            "{}",
            keys(&view)
        );
        assert_eq!(view.key(&left), Took::Moved);
        assert_eq!(view.key(&left), Took::Moved);
        assert_eq!(shown(&view), "Usage · All · details");
        assert_eq!(view.key(&left), Took::Moved);
        assert_eq!(shown(&view), "Usage · unknown · details");
        assert_eq!(view.key(&details), Took::Moved);
        assert_eq!(shown(&view), "Usage · unknown");
        assert_eq!(view.key(&Key::Text("x".into())), Took::Nothing);
        let escape = Key::Press(Press::new(Code::Escape, 0));
        assert_eq!(view.key(&escape), Took::Closed);
        let opened = super::view(&history, Path::new("/s"), current, "b/new", now, true);
        assert_eq!(shown(&opened), "Usage · b · details");
    }
}
