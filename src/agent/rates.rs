//! How fast models write, as Scramjet's output rates: a reply's output tokens over the time from
//! its request to its end, kept for each model's last replies across sessions in
//! `$XDG_STATE_HOME/scramjet-rs/output-rates.json`, and shown as their median.

use super::client::Reply;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Scramjet's bounds: the replies kept for each model, and in all.
const PER_MODEL: usize = 20;
const KEPT: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    model: String,
    output_tokens: u64,
    /// From the request to the reply's end.
    duration_ms: f64,
    /// When the reply ended, in milliseconds since 1970.
    observed_at: u64,
}

impl Sample {
    fn rate(&self) -> f64 {
        self.output_tokens as f64 * 1000.0 / self.duration_ms
    }
}

/// Where the rates are kept; nowhere in unit tests, which must leave the user's alone.
pub(super) fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let state = super::session::state_directory().ok()?;
    Some(state.join("output-rates.json"))
}

/// The replies a file keeps, oldest first.
#[derive(Debug, Default)]
pub(super) struct Rates(Vec<Sample>);

impl Rates {
    /// What `path` keeps; nothing when there is no file or it cannot be read.
    pub(super) fn read(path: Option<&Path>) -> Self {
        let text = path.and_then(|path| std::fs::read(path).ok());
        Self(text.map(|text| samples(&text)).unwrap_or_default())
    }

    /// The median of `model`'s kept rates, in tokens a second, as Scramjet's.
    pub(super) fn median(&self, model: &str) -> Option<f64> {
        let mut rates: Vec<f64> = (self.0.iter())
            .filter(|sample| sample.model == model)
            .map(Sample::rate)
            .collect();
        rates.sort_by(f64::total_cmp);
        let middle = rates.len() / 2;
        match rates.len() {
            0 => None,
            n if n % 2 == 0 => Some((rates[middle - 1] + rates[middle]) / 2.0),
            _ => Some(rates[middle]),
        }
    }
}

/// The valid samples of a file's text; a file that is not one keeps none, and the next record
/// replaces it, as the rates are only shown.
fn samples(text: &[u8]) -> Vec<Sample> {
    let Ok(mut document) = serde_json::from_slice::<Value>(text) else {
        return Vec::new();
    };
    let Value::Array(samples) = document["samples"].take() else {
        return Vec::new();
    };
    (samples.into_iter())
        .filter_map(|sample| serde_json::from_value::<Sample>(sample).ok())
        .filter(|sample| {
            !sample.model.is_empty()
                && sample.output_tokens > 0
                && sample.duration_ms.is_finite()
                && sample.duration_ms > 0.0
        })
        .collect()
}

/// Keeps the rate `reply` from `model` was written at in `path`, with what other sessions kept
/// there: none for a reply without output tokens or time.
pub(super) fn record(path: &Path, model: &str, reply: &Reply) -> io::Result<()> {
    let output = reply.usage.as_ref().and_then(|usage| usage.output_tokens);
    let duration_ms = reply.elapsed.as_secs_f64() * 1000.0;
    let (Some(output_tokens @ 1..), true) = (output, duration_ms > 0.0) else {
        return Ok(());
    };
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    let sample = Sample {
        model: model.to_owned(),
        output_tokens,
        duration_ms,
        observed_at,
    };
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)?;
    // Another session's record between the read and the rename would be lost without it.
    let lock = File::create(path.with_extension("lock"))?;
    lock.lock()?;
    let mut samples = match std::fs::read(path) {
        Ok(text) => samples(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    samples.push(sample);
    let samples = kept(samples);
    let temporary = directory.join(format!(".output-rates.{}.tmp", std::process::id()));
    let text = serde_json::to_vec_pretty(&json!({"version": 1, "samples": samples}))?;
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
}

/// Each model's last `PER_MODEL` samples, at most `KEPT` in all, oldest first.
fn kept(mut samples: Vec<Sample>) -> Vec<Sample> {
    samples.sort_by_key(|sample| sample.observed_at);
    let mut later = std::collections::HashMap::<String, usize>::new();
    let mut kept: Vec<Sample> = (samples.into_iter().rev())
        .filter(|sample| {
            let count = later.entry(sample.model.clone()).or_default();
            *count += 1;
            *count <= PER_MODEL
        })
        .take(KEPT)
        .collect();
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;
    use crate::http::responses::ProviderUsage;
    use std::time::Duration;

    fn reply(output: Option<u64>, millis: u64) -> Reply {
        Reply {
            usage: Some(ProviderUsage {
                input_tokens: None,
                output_tokens: output,
                total_tokens: None,
                cached_input_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
            elapsed: Duration::from_millis(millis),
            ..Reply::default()
        }
    }

    #[test]
    fn each_models_median_rate_is_kept_across_sessions() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-rates-{}", std::process::id())),
        );
        let path = scratch.0.join("state/output-rates.json");
        assert_eq!(Rates::read(Some(&path)).median("a"), None);
        for (tokens, millis) in [(100, 1000), (300, 1000), (200, 1000)] {
            record(&path, "a", &reply(Some(tokens), millis)).unwrap();
        }
        record(&path, "b", &reply(Some(50), 2000)).unwrap();
        // Replies without output tokens or without known usage say nothing about the rate.
        record(&path, "b", &reply(Some(0), 2000)).unwrap();
        record(&path, "b", &reply(None, 2000)).unwrap();
        let rates = Rates::read(Some(&path));
        assert_eq!(rates.median("a"), Some(200.0));
        assert_eq!(rates.median("b"), Some(25.0));
        assert_eq!(rates.median("c"), None);
        record(&path, "a", &reply(Some(400), 1000)).unwrap();
        // An even count's median is the mean of the middle two.
        assert_eq!(Rates::read(Some(&path)).median("a"), Some(250.0));
        // No temporary file stays behind.
        let names: Vec<_> = std::fs::read_dir(scratch.0.join("state"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        // A file that is not one is replaced.
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Rates::read(Some(&path)).median("a"), None);
        record(&path, "a", &reply(Some(30), 1000)).unwrap();
        assert_eq!(Rates::read(Some(&path)).median("a"), Some(30.0));
    }

    #[test]
    fn each_model_keeps_its_last_twenty_replies() {
        let sample = |model: &str, at| Sample {
            model: model.into(),
            output_tokens: at + 1,
            duration_ms: 1000.0,
            observed_at: at,
        };
        let mut samples: Vec<_> = (0..30).map(|at| sample("a", at)).collect();
        samples.push(sample("b", 5));
        samples.reverse();
        let kept = kept(samples);
        assert_eq!(kept.len(), PER_MODEL + 1);
        assert_eq!(kept[0], sample("b", 5));
        assert_eq!(kept[1], sample("a", 10));
        assert_eq!(kept.last(), Some(&sample("a", 29)));
    }
}
