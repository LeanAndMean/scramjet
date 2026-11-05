//! What a response cost, from its model's `cost` entry: Scramjet's dollars per million tokens
//! (`input`, `output`, `cacheRead`, `cacheWrite`), with two of scramjet-rs's own. `longContext`
//! gives the rates of a response whose input is over its `above` tokens. `header` names a
//! response header, such as the backend that served it, and `byHeader` the rates for each of its
//! values. A response with a value `byHeader` lacks has no known cost.

use crate::http::responses::ProviderUsage;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Prices {
    rates: Tiered,
    header: Option<String>,
    by_header: Vec<(String, Tiered)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Tiered {
    rates: Rates,
    /// The rates above a number of input tokens.
    long: Option<(u64, Rates)>,
}

/// Dollars per million tokens; `None` where the entry gives no price.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rates {
    input: Option<f64>,
    cache_write: Option<f64>,
    cache_read: Option<f64>,
    output: Option<f64>,
}

impl Rates {
    fn of(entry: &Value) -> Self {
        Self {
            input: entry["input"].as_f64(),
            cache_write: entry["cacheWrite"].as_f64(),
            cache_read: entry["cacheRead"].as_f64(),
            output: entry["output"].as_f64(),
        }
    }
}

impl Tiered {
    fn of(entry: &Value) -> Self {
        let long = &entry["longContext"];
        Self {
            rates: Rates::of(entry),
            long: long["above"].as_u64().map(|above| (above, Rates::of(long))),
        }
    }
}

impl Prices {
    /// From a model's `cost` entry; without one, every cost is unknown.
    pub(super) fn of(cost: &Value) -> Self {
        let by_header = (cost["byHeader"].as_object().into_iter().flatten())
            .map(|(value, rates)| (value.to_ascii_lowercase(), Tiered::of(rates)))
            .collect();
        Self {
            rates: Tiered::of(cost),
            header: cost["header"].as_str().map(str::to_ascii_lowercase),
            by_header,
        }
    }

    /// The response header whose value chooses the rates.
    pub(super) fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    /// What a response with `usage` cost in dollars, `served_by` the value of its `header`, in
    /// lower case; `None` when no rates apply or they have no price for tokens it used.
    pub(super) fn cost(&self, usage: &ProviderUsage, served_by: Option<&str>) -> Option<f64> {
        let tiered = match served_by {
            Some(value) if self.header.is_some() => {
                &(self.by_header.iter())
                    .find(|(listed, _)| listed == value)?
                    .1
            }
            _ => &self.rates,
        };
        let input = usage.input_tokens?;
        let read = usage.cached_input_tokens.unwrap_or(0);
        let written = usage.cache_write_tokens.unwrap_or(0);
        let rates = match tiered.long {
            Some((above, rates)) if input > above => rates,
            _ => tiered.rates,
        };
        let part = |tokens: u64, rate: Option<f64>| match tokens {
            0 => Some(0.0),
            _ => Some(tokens as f64 * rate? / 1e6),
        };
        Some(
            part(input.saturating_sub(read + written), rates.input)?
                + part(written, rates.cache_write)?
                + part(read, rates.cache_read)?
                + part(usage.output_tokens?, rates.output)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(input: u64, read: u64, written: u64, output: u64) -> ProviderUsage {
        ProviderUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            total_tokens: Some(input + output),
            cached_input_tokens: Some(read),
            cache_write_tokens: Some(written),
            reasoning_tokens: None,
        }
    }

    fn close(cost: Option<f64>, expected: f64) {
        let cost = cost.unwrap();
        assert!((cost - expected).abs() < 1e-12, "{cost} != {expected}");
    }

    #[test]
    fn the_header_value_chooses_the_rates() {
        let prices = Prices::of(&json!({
            "input": 5, "cacheWrite": 6.25, "cacheRead": 0.5, "output": 25,
            "header": "X-Served-By",
            "byHeader": {
                "direct": {"input": 5, "cacheWrite": 6.25, "cacheRead": 0.5, "output": 25},
                "Bedrock": {"input": 5.5, "cacheWrite": 6.875, "cacheRead": 0.55, "output": 27.5},
            },
        }));
        assert_eq!(prices.header(), Some("x-served-by"));
        // 1M input tokens: 600k not cached, 300k read from the cache and 100k written to it.
        let used = usage(1_000_000, 300_000, 100_000, 10_000);
        close(prices.cost(&used, None), 3.0 + 0.625 + 0.15 + 0.25);
        close(
            prices.cost(&used, Some("direct")),
            3.0 + 0.625 + 0.15 + 0.25,
        );
        close(
            prices.cost(&used, Some("bedrock")),
            3.3 + 0.6875 + 0.165 + 0.275,
        );
        // A value without rates of its own, or no entry at all: the cost is not known.
        assert_eq!(prices.cost(&used, Some("elsewhere")), None);
        assert_eq!(Prices::of(&Value::Null).cost(&used, None), None);
        // Without a header to read, its value does not matter.
        let flat = Prices::of(&json!({"input": 1, "cacheWrite": 1, "cacheRead": 1, "output": 1}));
        close(flat.cost(&used, Some("bedrock")), 1.01);
    }

    #[test]
    fn long_context_rates_apply_above_their_threshold_and_a_missing_price_is_unknown() {
        let prices = Prices::of(&json!({
            "input": 2, "cacheRead": 0.2, "output": 10,
            "longContext": {"above": 272_000, "input": 4, "cacheRead": 0.4, "output": 15},
            "header": "x-served-by",
            "byHeader": {"azure": {"input": 1, "cacheWrite": 1, "cacheRead": 1, "output": 1,
                "longContext": {"above": 100_000, "input": 2, "cacheWrite": 2, "cacheRead": 2,
                    "output": 2}}},
        }));
        close(
            prices.cost(&usage(272_000, 72_000, 0, 1_000), None),
            0.4 + 0.0144 + 0.01,
        );
        close(
            prices.cost(&usage(272_001, 72_000, 0, 1_000), None),
            200_001.0 * 4e-6 + 0.0288 + 0.015,
        );
        close(
            prices.cost(&usage(100_001, 0, 0, 0), Some("azure")),
            100_001.0 * 2e-6,
        );
        // Cache writes without a price make the cost unknown; none written cost nothing.
        assert_eq!(prices.cost(&usage(1_000, 0, 10, 1), None), None);
        let unknown_output = ProviderUsage {
            output_tokens: None,
            ..usage(1_000, 0, 0, 0)
        };
        assert_eq!(prices.cost(&unknown_output, None), None);
    }
}
