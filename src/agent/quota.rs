//! What is left of a subscription's quota, as ChatGPT's (Codex) responses report it in their
//! `x-codex-primary-*` and `x-codex-secondary-*` headers (as Codex reads them,
//! `codex-rs/codex-api/src/rate_limits.rs`), kept for each provider's latest response across
//! sessions in `$XDG_STATE_HOME/scramjet-rs/quota.json`, for `/usage`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

/// One limit's window: the share of it used, how long it is and when it starts again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    /// In seconds since 1970.
    pub resets_at: Option<i64>,
}

/// A provider's quota as one response reported it: ChatGPT's shorter window (five hours) is
/// its primary, its weekly one its secondary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    /// When the response came, in milliseconds since 1970.
    pub observed_at: u64,
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
}

/// The quota a ChatGPT response's headers report, `header` giving a header's value by its
/// lower-case name; none without either window.
pub(super) fn from_codex_headers<'a>(
    header: impl Fn(&str) -> Option<&'a str>,
    observed_at: u64,
) -> Option<Quota> {
    let window = |which: &str| {
        let value = |part: &str| header(&format!("x-codex-{which}-{part}")).map(str::trim);
        let used_percent =
            (value("used-percent")?.parse::<f64>().ok()).filter(|percent| percent.is_finite())?;
        let window_minutes = value("window-minutes").and_then(|text| text.parse().ok());
        let resets_at = value("reset-at").and_then(|text| text.parse().ok());
        // Codex takes all three empty for no window.
        let some =
            used_percent != 0.0 || window_minutes.is_some_and(|m| m != 0) || resets_at.is_some();
        some.then_some(Window {
            used_percent,
            window_minutes,
            resets_at,
        })
    };
    let (primary, secondary) = (window("primary"), window("secondary"));
    (primary.is_some() || secondary.is_some()).then_some(Quota {
        observed_at,
        primary,
        secondary,
    })
}

/// Where the quotas are kept; nowhere in unit tests, which must leave the user's alone.
pub(super) fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(super::session::state_directory().ok()?.join("quota.json"))
}

/// Keeps the quota `reply` from `model` (`provider/id`) reported, as its provider's.
pub(super) fn kept_from(model: &str, reply: &super::client::Reply) {
    let (Some(quota), Some(path)) = (&reply.quota, path()) else {
        return;
    };
    let provider = model
        .split_once('/')
        .map_or(model, |(provider, _)| provider);
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

    fn headers(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<&'static str> {
        let pairs = pairs.to_vec();
        move |name| pairs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }

    #[test]
    fn chatgpts_headers_give_both_windows() {
        let quota = from_codex_headers(
            headers(&[
                ("x-codex-primary-used-percent", "12.5"),
                ("x-codex-primary-window-minutes", "300"),
                ("x-codex-primary-reset-at", "1704069000"),
                ("x-codex-secondary-used-percent", " 40 "),
                ("x-codex-secondary-window-minutes", "10080"),
            ]),
            7,
        )
        .unwrap();
        assert_eq!(quota.observed_at, 7);
        let primary = quota.primary.unwrap();
        assert_eq!(
            (
                primary.used_percent,
                primary.window_minutes,
                primary.resets_at
            ),
            (12.5, Some(300), Some(1704069000))
        );
        let secondary = quota.secondary.unwrap();
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
    fn no_or_empty_windows_are_no_quota() {
        assert_eq!(from_codex_headers(headers(&[]), 1), None);
        let empty = headers(&[
            ("x-codex-primary-used-percent", "0"),
            ("x-codex-primary-window-minutes", "0"),
            ("x-codex-secondary-used-percent", "NaN"),
        ]);
        assert_eq!(from_codex_headers(empty, 1), None);
        let unused = headers(&[
            ("x-codex-primary-used-percent", "0"),
            ("x-codex-primary-window-minutes", "300"),
        ]);
        let quota = from_codex_headers(unused, 1).unwrap();
        assert_eq!(quota.primary.unwrap().used_percent, 0.0);
        assert_eq!(quota.secondary, None);
    }

    #[test]
    fn chatgpts_replies_carry_the_quota_their_headers_report_and_others_none() {
        use super::super::client::{Protocol, Sent, respond};
        use super::super::providers::Route;
        use crate::provider_fixture as fixture;
        let body = fixture::wire(&fixture::text_events("Hi", fixture::known_usage()), false);
        let mut response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
             x-codex-primary-used-percent: 41\r\nx-codex-primary-window-minutes: 300\r\n\
             x-codex-primary-reset-at: 1794030000\r\nx-codex-secondary-used-percent: 7.5\r\n\
             x-codex-secondary-window-minutes: 10080\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend(body);
        let (endpoint, server) = super::super::tests::serve(vec![response.clone(), response]);
        let route = Route {
            provider: "openai-codex".into(),
            tls: false,
            host: endpoint.ip().to_string(),
            port: endpoint.port(),
            authority: endpoint.to_string(),
            path: String::new(),
            headers: Vec::new(),
            login: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let compat = Default::default();
        let reply = |protocol| {
            let sent = Sent {
                route: &route,
                protocol,
                headers: &[],
                priced_by: None,
                compat: &compat,
            };
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let body = br#"{"model":"m"}"#.to_vec();
            let reply = runtime.block_on(respond(sent, body, &cancel, &mut |_| {}));
            reply.ok().unwrap()
        };
        let quota = reply(Protocol::Codex).quota.unwrap();
        assert_eq!(
            quota.primary,
            Some(Window {
                used_percent: 41.0,
                window_minutes: Some(300),
                resets_at: Some(1794030000),
            })
        );
        assert_eq!(quota.secondary.unwrap().used_percent, 7.5);
        assert!(quota.observed_at > 1_700_000_000_000);
        // Only ChatGPT's backend is read for them.
        assert_eq!(reply(Protocol::Responses).quota, None);
        server.join().unwrap();
    }

    #[test]
    fn each_provider_keeps_its_latest_quota() {
        let directory = std::env::temp_dir().join(format!("quota-test-{}", std::process::id()));
        let scratch = super::super::tests::ScratchDir::new(directory);
        let path = scratch.0.join("state/quota.json");
        let quota = |at, used| Quota {
            observed_at: at,
            primary: Some(Window {
                used_percent: used,
                window_minutes: Some(300),
                resets_at: None,
            }),
            secondary: None,
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
