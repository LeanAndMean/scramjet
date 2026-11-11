//! What is left of a subscription's quota, as its responses' headers report it, kept for each
//! provider's latest response across sessions in `$XDG_STATE_HOME/scramjet/quota.json`, for
//! `/usage`. ChatGPT's (Codex) backend sends `x-codex-primary-*` and `x-codex-secondary-*`, a
//! model's own limits under other names (`x-<id>-primary-*`) and `x-codex-credits-*`, read as
//! Codex reads them (`codex-rs/codex-api/src/rate_limits.rs`); GitHub Copilot's sends
//! `x-quota-snapshot-*` and `x-usage-ratelimit-*`, read as VS Code's Copilot extension (0.66.0)
//! reads them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

/// One limit's window: the share of it used, how long it is and when it starts again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// The limit it belongs to where a provider has several (Codex's for a model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// What the limit is called where its length does not say (Copilot's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    /// In seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// The requests it allows (Copilot's premium requests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed: Option<u64>,
    /// The requests made past them, billed apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<f64>,
}

/// A provider's quota as one response reported it, its shorter windows first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    /// When the response came, in milliseconds since 1970.
    pub observed_at: u64,
    pub windows: Vec<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
}

/// The credits an account has besides its windows (ChatGPT's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    pub unlimited: bool,
    /// As the provider wrote it; Codex shows it rounded, or `Available` without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
}

/// The quota a response's headers report, as `(name, value)` pairs with lower-case names; none
/// when they report none.
pub(super) fn from_headers(headers: &[(&str, &str)], observed_at: u64) -> Option<Quota> {
    let header =
        |name: &str| (headers.iter()).find_map(|(n, value)| (*n == name).then_some(*value));
    let credits = codex_credits(header);
    let mut windows = codex_windows(headers, header);
    if windows.is_empty() && credits.is_none() {
        windows = copilot_windows(header);
    }
    (!windows.is_empty() || credits.is_some()).then_some(Quota {
        observed_at,
        windows,
        credits,
    })
}

/// ChatGPT's shorter window (five hours) and its weekly one, then those of each other limit the
/// headers name (`x-<id>-primary-used-percent`), by id.
fn codex_windows<'a>(
    headers: &[(&str, &str)],
    header: impl Fn(&str) -> Option<&'a str>,
) -> Vec<Window> {
    let window = |prefix: &str, which: &str, limit: Option<&str>| {
        let value = |part: &str| header(&format!("{prefix}-{which}-{part}")).map(str::trim);
        let used_percent =
            (value("used-percent")?.parse::<f64>().ok()).filter(|percent| percent.is_finite())?;
        let window_minutes = value("window-minutes").and_then(|text| text.parse().ok());
        let resets_at = value("reset-at").and_then(|text| text.parse().ok());
        // Codex takes all three empty for no window.
        let some =
            used_percent != 0.0 || window_minutes.is_some_and(|m| m != 0) || resets_at.is_some();
        some.then(|| Window {
            limit: limit.map(str::to_owned),
            used_percent,
            window_minutes,
            resets_at,
            ..Window::default()
        })
    };
    let mut prefixes: Vec<&str> = (headers.iter())
        .filter_map(|(name, _)| name.strip_suffix("-primary-used-percent"))
        .filter(|prefix| prefix.starts_with("x-") && *prefix != "x-codex")
        .collect();
    prefixes.sort_unstable();
    prefixes.dedup();
    let mut windows: Vec<Window> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|which| window("x-codex", which, None))
        .collect();
    for prefix in prefixes {
        let name = (header(&format!("{prefix}-limit-name")).map(str::trim))
            .filter(|name| !name.is_empty())
            .unwrap_or(&prefix[2..]);
        windows.extend(
            ["primary", "secondary"]
                .map(|which| window(prefix, which, Some(name)))
                .into_iter()
                .flatten(),
        );
    }
    windows
}

/// ChatGPT's credits, when the account has some or no limit to them.
fn codex_credits<'a>(header: impl Fn(&str) -> Option<&'a str>) -> Option<Credits> {
    let flag = |name: &str| match header(name)?.trim() {
        "1" => Some(true),
        "0" => Some(false),
        text if text.eq_ignore_ascii_case("true") => Some(true),
        text if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    };
    let has = flag("x-codex-credits-has-credits")?;
    let unlimited = flag("x-codex-credits-unlimited")?;
    let balance = (header("x-codex-credits-balance").map(str::trim))
        .filter(|balance| !balance.is_empty())
        .map(str::to_owned);
    (has || unlimited).then_some(Credits { unlimited, balance })
}

/// Copilot's session and weekly rate limits, then its monthly premium requests (a free plan's
/// chat requests where it has none).
fn copilot_windows<'a>(header: impl Fn(&str) -> Option<&'a str>) -> Vec<Window> {
    let window = |name: &str, header_name: &str| copilot_window(name, header(header_name)?);
    let premium = (window("Premium", "x-quota-snapshot-premium_models"))
        .or_else(|| window("Premium", "x-quota-snapshot-premium_interactions"));
    let premium = match premium {
        Some(premium) if premium.allowed.is_some() => Some(premium),
        premium => window("Chat", "x-quota-snapshot-chat").or(premium),
    };
    [
        window("Session", "x-usage-ratelimit-session"),
        window("Weekly", "x-usage-ratelimit-weekly"),
        premium,
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// One of Copilot's quota headers, a query string: `ent` the requests allowed (`-1` for no
/// limit), `rem` the share left in percent, `ov` the requests past them and `rst` when it starts
/// again. None for no limit.
fn copilot_window(name: &str, value: &str) -> Option<Window> {
    let fields: Vec<(String, String)> = (value.trim().split('&'))
        .filter_map(|field| {
            let (key, value) = field.split_once('=').unwrap_or((field, ""));
            Some((decoded(key)?, decoded(value)?))
        })
        .collect();
    let field = |key: &str| (fields.iter()).find_map(|(k, v)| (k == key).then_some(v.as_str()));
    let allowed: i64 = field("ent").map_or(Some(0), |text| text.parse().ok())?;
    if allowed == -1 {
        return None;
    }
    let left = (field("rem")?.parse::<f64>().ok()).filter(|left| left.is_finite())?;
    let over = (field("ov").and_then(|text| text.parse::<f64>().ok()))
        .filter(|over| over.is_finite() && *over > 0.0);
    Some(Window {
        name: Some(name.to_owned()),
        used_percent: 100.0 - left,
        resets_at: field("rst").and_then(utc_seconds),
        allowed: u64::try_from(allowed).ok().filter(|allowed| *allowed > 0),
        over,
        ..Window::default()
    })
}

/// A query string's part, with its `%XX` escapes and `+` for a space; none if not UTF-8.
fn decoded(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        let hex = after
            .get(..2)
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match (byte, hex) {
            (b'%', Some(value)) => {
                bytes.push(value);
                rest = &after[2..];
                continue;
            }
            (b'+', _) => bytes.push(b' '),
            _ => bytes.push(byte),
        }
        rest = after;
    }
    String::from_utf8(bytes).ok()
}

/// Seconds since 1970 of an ISO 8601 date (`2026-12-01`, taken as UTC) or time
/// (`2026-12-01T00:00:00Z`, with fractions and an offset).
fn utc_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let dashes = text.get(4..5) == Some("-") && text.get(7..8) == Some("-");
    if !dashes || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = super::usage::civil_days(year as i32, month as i32, day as i32);
    let mut rest = &text[10..];
    let mut seconds = 0;
    if let Some(time) = rest.strip_prefix(['T', 't', ' ']) {
        let field = |range: std::ops::Range<usize>| -> Option<i64> {
            let part = time.get(range)?;
            part.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| part.parse().ok())?
        };
        let (hour, minute) = (field(0..2)?, field(3..5)?);
        if time.get(2..3) != Some(":") || hour > 23 || minute > 59 {
            return None;
        }
        let (second, mut after) = match time.get(5..6) {
            Some(":") => (field(6..8)?, &time[8..]),
            _ => (0, &time[5..]),
        };
        if let Some(fraction) = after.strip_prefix('.') {
            after = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
        }
        seconds = hour * 3600 + minute * 60 + second;
        rest = after;
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.get(..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let digits = rest[1..].replace(':', "");
            if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let (hours, minutes): (i64, i64) =
                (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
            sign * (hours * 3600 + minutes * 60)
        }
    };
    Some(days * 86_400 + seconds - offset)
}

/// Where the quotas are kept; nowhere in unit tests, which must leave the user's alone.
pub(super) fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(super::session::state_directory().ok()?.join("quota.json"))
}

/// Keeps the quota a response from `provider` reported, if any, as that provider's.
pub(super) fn keep(provider: &str, quota: Option<&Quota>) {
    let (Some(quota), Some(path)) = (quota, path()) else {
        return;
    };
    // The quota is only shown; a session goes on without it.
    let _ = record(&path, provider, quota);
}

/// The quotas a file keeps, by provider; none when it cannot be read.
fn kept(text: &[u8]) -> Map<String, Value> {
    match serde_json::from_slice::<Value>(text) {
        Ok(Value::Object(mut document)) => match document.remove("providers") {
            Some(Value::Object(providers)) => providers,
            _ => Map::new(),
        },
        _ => Map::new(),
    }
}

/// Keeps `quota` as `provider`'s in `path`, with the other providers' there, unless the file
/// keeps a later one.
pub(super) fn record(path: &Path, provider: &str, quota: &Quota) -> io::Result<()> {
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)?;
    // Another session's record between the read and the rename would be lost without it.
    let lock = File::create(path.with_extension("lock"))?;
    lock.lock()?;
    let mut providers = match std::fs::read(path) {
        Ok(text) => kept(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(error),
    };
    let later = (providers.get(provider))
        .and_then(|kept| serde_json::from_value::<Quota>(kept.clone()).ok())
        .is_some_and(|kept| kept.observed_at > quota.observed_at);
    if later {
        return Ok(());
    }
    providers.insert(provider.to_owned(), serde_json::to_value(quota)?);
    let temporary = directory.join(format!(".quota.{}.tmp", std::process::id()));
    let text = serde_json::to_vec_pretty(&json!({"version": 1, "providers": providers}))?;
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
}

/// The quotas kept in the state directory, by provider: none when there are none, or the file
/// cannot be read.
pub fn read() -> Vec<(String, Quota)> {
    let text = path().and_then(|path| std::fs::read(path).ok());
    read_from(text.as_deref().unwrap_or_default())
}

fn read_from(text: &[u8]) -> Vec<(String, Quota)> {
    (kept(text).into_iter())
        .filter_map(|(provider, quota)| Some((provider, serde_json::from_value(quota).ok()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatgpts_headers_give_both_windows() {
        let quota = from_headers(
            &[
                ("x-codex-primary-used-percent", "12.5"),
                ("x-codex-primary-window-minutes", "300"),
                ("x-codex-primary-reset-at", "1704069000"),
                ("x-codex-secondary-used-percent", " 40 "),
                ("x-codex-secondary-window-minutes", "10080"),
            ],
            7,
        )
        .unwrap();
        assert_eq!(quota.observed_at, 7);
        let [primary, secondary] = &quota.windows[..] else {
            panic!("{quota:?}")
        };
        assert_eq!(
            (
                primary.used_percent,
                primary.window_minutes,
                primary.resets_at
            ),
            (12.5, Some(300), Some(1704069000))
        );
        assert_eq!(
            (
                secondary.used_percent,
                secondary.window_minutes,
                secondary.resets_at
            ),
            (40.0, Some(10080), None)
        );
    }

    #[test]
    fn chatgpts_other_limits_follow_its_own_with_its_credits() {
        let quota = from_headers(
            &[
                ("x-codex-bengalfox-primary-used-percent", "80"),
                ("x-codex-bengalfox-primary-window-minutes", "300"),
                ("x-codex-bengalfox-limit-name", "gpt-5.2-codex-sonic"),
                ("x-codex-primary-used-percent", "12.5"),
                ("x-codex-other-primary-used-percent", "0"),
                ("x-codex-other-secondary-used-percent", "3"),
                ("x-codex-credits-has-credits", "true"),
                ("x-codex-credits-unlimited", "False"),
                ("x-codex-credits-balance", " 41.6 "),
            ],
            1,
        )
        .unwrap();
        let limits: Vec<_> = (quota.windows.iter())
            .map(|window| (window.limit.as_deref(), window.used_percent))
            .collect();
        assert_eq!(
            limits,
            [
                (None, 12.5),
                (Some("gpt-5.2-codex-sonic"), 80.0),
                (Some("codex-other"), 3.0),
            ]
        );
        assert_eq!(
            quota.credits,
            Some(Credits {
                unlimited: false,
                balance: Some("41.6".into()),
            })
        );
        // Credits alone are a quota; an account without any shows none.
        let unlimited = [
            ("x-codex-credits-has-credits", "0"),
            ("x-codex-credits-unlimited", "1"),
        ];
        let quota = from_headers(&unlimited, 1).unwrap();
        assert!(quota.windows.is_empty() && quota.credits.unwrap().unlimited);
        let none = [
            ("x-codex-credits-has-credits", "false"),
            ("x-codex-credits-unlimited", "false"),
        ];
        assert_eq!(from_headers(&none, 1), None);
    }

    #[test]
    fn no_or_empty_windows_are_no_quota() {
        assert_eq!(from_headers(&[], 1), None);
        let empty = &[
            ("x-codex-primary-used-percent", "0"),
            ("x-codex-primary-window-minutes", "0"),
            ("x-codex-secondary-used-percent", "NaN"),
        ];
        assert_eq!(from_headers(empty, 1), None);
        let unused = &[
            ("x-codex-primary-used-percent", "0"),
            ("x-codex-primary-window-minutes", "300"),
        ];
        let quota = from_headers(unused, 1).unwrap();
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].used_percent, 0.0);
    }

    #[test]
    fn replies_carry_the_quota_chatgpts_or_copilots_headers_report() {
        use super::super::client::{Protocol, Sent, respond};
        use super::super::providers::Route;
        use crate::provider_fixture as fixture;
        let body = fixture::wire(&fixture::text_events("Hi", fixture::known_usage()), false);
        let response = |headers: &str| {
            let mut response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n{headers}\
                 content-length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend(&body);
            response
        };
        let codex = response(
            "x-codex-primary-used-percent: 41\r\nx-codex-primary-window-minutes: 300\r\n\
             x-codex-primary-reset-at: 1794030000\r\nx-codex-secondary-used-percent: 7.5\r\n\
             x-codex-secondary-window-minutes: 10080\r\n",
        );
        let copilot = response(
            "x-quota-snapshot-premium_interactions: ent=300&ov=0.0&ovPerm=false&rem=80&\
             rst=2026-12-01T00%3A00%3A00Z\r\n",
        );
        let limit = br#"{"error":{"code":"usage_limit_reached","plan_type":"plus"}}"#;
        let mut refused = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\n\
             x-codex-primary-used-percent: 100\r\nx-codex-primary-window-minutes: 300\r\n\
             content-length: {}\r\n\r\n",
            limit.len()
        )
        .into_bytes();
        refused.extend(limit);
        let (endpoint, server) =
            super::super::tests::serve(vec![codex, copilot, response(""), refused]);
        let route = Route {
            provider: "openai-codex".into(),
            tls: false,
            host: endpoint.ip().to_string(),
            port: endpoint.port(),
            authority: endpoint.to_string(),
            path: String::new(),
            headers: Vec::new(),
            commanded: Vec::new(),
            login: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let compat = Default::default();
        let respond = |protocol| {
            let sent = Sent {
                route: &route,
                protocol,
                given: &[],
                headers: &[],
                priced_by: None,
                compat: &compat,
            };
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let body = br#"{"model":"m"}"#.to_vec();
            runtime.block_on(respond(sent, body, &cancel, &mut |_| {}))
        };
        let reply = |protocol| respond(protocol).ok().unwrap();
        let quota = reply(Protocol::Codex).quota.unwrap();
        assert_eq!(
            quota.windows,
            [
                Window {
                    used_percent: 41.0,
                    window_minutes: Some(300),
                    resets_at: Some(1794030000),
                    ..Window::default()
                },
                Window {
                    used_percent: 7.5,
                    window_minutes: Some(10080),
                    ..Window::default()
                },
            ]
        );
        assert!(quota.observed_at > 1_700_000_000_000);
        let quota = reply(Protocol::Responses).quota.unwrap();
        assert_eq!(
            quota.windows,
            [Window {
                name: Some("Premium".into()),
                used_percent: 20.0,
                resets_at: Some(1796083200),
                allowed: Some(300),
                ..Window::default()
            }]
        );
        assert_eq!(reply(Protocol::Responses).quota, None);
        let Err(super::super::client::ClientError::Failed(failure)) = respond(Protocol::Codex)
        else {
            panic!("a refused response is a failure");
        };
        assert!(
            failure.message.contains("usage limit"),
            "{}",
            failure.message
        );
        let windows = failure.quota.unwrap().windows;
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 100.0);
        server.join().unwrap();
    }

    #[test]
    fn copilots_headers_give_its_rate_limits_then_its_premium_requests() {
        let quota = from_headers(
            &[
                (
                    "x-quota-snapshot-premium_models",
                    "ent=300&ov=12.0&ovPerm=true&rem=-4&rst=2026-12-01",
                ),
                ("x-quota-snapshot-premium_interactions", "ent=1&rem=1"),
                (
                    "x-usage-ratelimit-session",
                    "rem=62.5&rst=2026-11-07T18%3A30%3A00.250%2B01%3A00",
                ),
                ("x-usage-ratelimit-weekly", "ent=-1&rem=100"),
            ],
            9,
        )
        .unwrap();
        assert_eq!(quota.observed_at, 9);
        assert_eq!(
            quota.windows,
            [
                Window {
                    name: Some("Session".into()),
                    used_percent: 37.5,
                    resets_at: Some(1794076200 - 3600),
                    ..Window::default()
                },
                Window {
                    name: Some("Premium".into()),
                    used_percent: 104.0,
                    resets_at: Some(1796083200),
                    allowed: Some(300),
                    over: Some(12.0),
                    ..Window::default()
                },
            ]
        );
        // A free plan's chat requests where it has no premium ones; no limit is no window.
        let free = from_headers(
            &[
                ("x-quota-snapshot-premium_interactions", "ent=0&rem=0"),
                ("x-quota-snapshot-chat", "ent=50&rem=90"),
            ],
            1,
        )
        .unwrap();
        assert_eq!(free.windows[0].name.as_deref(), Some("Chat"));
        assert_eq!(free.windows[0].allowed, Some(50));
        let unlimited = &[("x-quota-snapshot-premium_models", "ent=-1&rem=100")];
        assert_eq!(from_headers(unlimited, 1), None);
        assert_eq!(
            from_headers(&[("x-usage-ratelimit-weekly", "ent=5")], 1),
            None
        );
    }

    #[test]
    fn iso_dates_and_times_read_as_utc_seconds() {
        assert_eq!(utc_seconds("1970-01-01"), Some(0));
        assert_eq!(utc_seconds("2026-12-01T00:00:00Z"), Some(1796083200));
        assert_eq!(utc_seconds("2026-12-01 00:00"), Some(1796083200));
        assert_eq!(utc_seconds("2026-12-01T01:00:00.5+01:00"), Some(1796083200));
        assert_eq!(utc_seconds("2026-11-30T19:00:00-0500"), Some(1796083200));
        for wrong in [
            "",
            "1796083200",
            "2026-13-01",
            "2026-12-01T25:00",
            "2026-12-01X",
        ] {
            assert_eq!(utc_seconds(wrong), None, "{wrong}");
        }
        assert_eq!(decoded("a%3Ab+c%zz"), Some("a:b c%zz".into()));
    }

    #[test]
    fn each_provider_keeps_its_latest_quota() {
        let directory = std::env::temp_dir().join(format!("quota-test-{}", std::process::id()));
        let scratch = super::super::tests::ScratchDir::new(directory);
        let path = scratch.0.join("state/quota.json");
        let quota = |at, used| Quota {
            observed_at: at,
            windows: vec![Window {
                used_percent: used,
                window_minutes: Some(300),
                ..Window::default()
            }],
            credits: None,
        };
        record(&path, "openai-codex", &quota(2, 20.0)).unwrap();
        record(&path, "other", &quota(1, 5.0)).unwrap();
        // An earlier response that ends after a later one does not replace it.
        record(&path, "openai-codex", &quota(1, 10.0)).unwrap();
        let mut read = read_from(&std::fs::read(&path).unwrap());
        read.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            read,
            [
                ("openai-codex".to_owned(), quota(2, 20.0)),
                ("other".to_owned(), quota(1, 5.0)),
            ]
        );
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_from(&std::fs::read(&path).unwrap()), []);
        record(&path, "openai-codex", &quota(3, 30.0)).unwrap();
        assert_eq!(read_from(&std::fs::read(&path).unwrap()).len(), 1);
    }
}
