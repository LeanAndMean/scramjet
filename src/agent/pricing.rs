//! What a response cost, as Proxy prices it (`src/cost/provider-pricing.ts` there): the price card
//! of the model's pricing key for the backend that served the response, the latest in effect when
//! it ran, at its long-context rates when the response's input was over their threshold.

use crate::http::responses::ProviderUsage;
use serde_json::Value;

/// Proxy's threshold for long-context rates when a card gives none.
const LONG_CONTEXT_TOKENS: u64 = 272_000;
/// The agentic backend whose usage Proxy prices with the provider's own cards.
const PROVIDER_PRICED_BACKEND: &str = "palantir";

/// The price cards of one model's pricing key, from Proxy's model card document.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Prices(Vec<Card>);

#[derive(Clone, Debug, PartialEq)]
struct Card {
    /// The agentic backend (`x-llm-backend`) it prices; `None` for the provider's own card.
    backend: Option<String>,
    /// When it took effect, in seconds since the Unix epoch.
    from: i64,
    rates: Rates,
    /// The rates above a number of input tokens.
    long: Option<(u64, Rates)>,
}

/// Dollars per million tokens; `None` where the card has no price.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rates {
    input: Option<f64>,
    cache_write: Option<f64>,
    cache_read: Option<f64>,
    output: Option<f64>,
}

impl Rates {
    fn of(card: &Value) -> Self {
        Self {
            input: card["inputPerMTok"].as_f64(),
            cache_write: card["cacheWritePerMTok"].as_f64(),
            cache_read: card["cacheReadPerMTok"].as_f64(),
            output: card["outputPerMTok"].as_f64(),
        }
    }
}

impl Prices {
    /// The cards in `cards` (the document's `priceCards`) for `key`; a card of another API or
    /// without a date it took effect is left out.
    pub(super) fn of(cards: &[Value], key: Option<&str>) -> Self {
        let Some(key) = key else {
            return Self::default();
        };
        let card = |card: &Value| {
            let backend = match card["api"].as_str()? {
                "foundry" => None,
                "agentic" => Some(card["upstreamProvider"].as_str()?.to_owned()),
                _ => return None,
            };
            let long = card.get("longContext").map(|rates| {
                let above = card["longContextThresholdTokens"].as_u64();
                (above.unwrap_or(LONG_CONTEXT_TOKENS), Rates::of(rates))
            });
            Some(Card {
                backend,
                from: seconds(card["effectiveFrom"].as_str()?)?,
                rates: Rates::of(card),
                long,
            })
        };
        Self(
            (cards.iter())
                .filter(|card| card["pricingKey"] == key)
                .filter_map(card)
                .collect(),
        )
    }

    /// What a response with `usage` cost in dollars, served by `backend` at `now` (seconds since
    /// the Unix epoch); `None` when no card applies or it has no price for tokens the response
    /// used.
    pub(super) fn cost(
        &self,
        usage: &ProviderUsage,
        backend: Option<&str>,
        now: i64,
    ) -> Option<f64> {
        let backend = backend.filter(|backend| *backend != PROVIDER_PRICED_BACKEND);
        let card = (self.0.iter())
            .filter(|card| card.backend.as_deref() == backend && card.from <= now)
            .max_by_key(|card| card.from)?;
        let input = usage.input_tokens?;
        let read = usage.cached_input_tokens.unwrap_or(0);
        let written = usage.cache_write_tokens.unwrap_or(0);
        let rates = match card.long {
            Some((above, rates)) if input > above => rates,
            _ => card.rates,
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

/// An ISO 8601 UTC time as Proxy writes it (`2026-10-07T00:00:00.000Z`), in seconds since the Unix
/// epoch.
fn seconds(time: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| time.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || time.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    // Days from the civil date, as `instructions::today` turns them back.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Now, in seconds since the Unix epoch.
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
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
    fn times_are_read_as_proxy_writes_them() {
        assert_eq!(seconds("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(seconds("2026-10-07T00:00:00.000Z"), Some(1_791_331_200));
        assert_eq!(seconds("2000-02-29T12:34:56Z"), Some(951_827_696));
        assert_eq!(seconds("2026-13-01T00:00:00Z"), None);
        assert_eq!(seconds("2026-10-07"), None);
    }

    #[test]
    fn the_backend_chooses_the_card_and_the_latest_in_effect_applies() {
        let cards = [
            json!({"api":"foundry","pricingKey":"opus","effectiveFrom":"1970-01-01T00:00:00.000Z",
                "inputPerMTok":5,"cacheWritePerMTok":5,"cacheReadPerMTok":0,"outputPerMTok":25}),
            json!({"api":"foundry","pricingKey":"opus","effectiveFrom":"2026-06-06T00:00:00.000Z",
                "inputPerMTok":5,"cacheWritePerMTok":6.25,"cacheReadPerMTok":0.5,"outputPerMTok":25}),
            json!({"api":"agentic","pricingKey":"opus","upstreamProvider":"bedrock",
                "effectiveFrom":"2026-09-25T00:00:00.000Z",
                "inputPerMTok":5.5,"cacheWritePerMTok":6.875,"cacheReadPerMTok":0.55,"outputPerMTok":27.5}),
            json!({"api":"foundry","pricingKey":"other","effectiveFrom":"1970-01-01T00:00:00.000Z",
                "inputPerMTok":1,"cacheWritePerMTok":1,"cacheReadPerMTok":1,"outputPerMTok":1}),
        ];
        let prices = Prices::of(&cards, Some("opus"));
        // 1M input tokens: 600k not cached, 300k read from the cache and 100k written to it.
        let used = usage(1_000_000, 300_000, 100_000, 10_000);
        let june = seconds("2026-07-01T00:00:00Z").unwrap();
        close(prices.cost(&used, None, june), 3.0 + 0.625 + 0.15 + 0.25);
        close(
            prices.cost(&used, Some("palantir"), june),
            3.0 + 0.625 + 0.15 + 0.25,
        );
        close(prices.cost(&used, None, 0), 3.0 + 0.5 + 0.0 + 0.25);
        let october = seconds("2026-10-01T00:00:00Z").unwrap();
        close(
            prices.cost(&used, Some("bedrock"), october),
            3.3 + 0.6875 + 0.165 + 0.275,
        );
        // No card for the backend yet, or for a backend Proxy does not price.
        assert_eq!(prices.cost(&used, Some("bedrock"), june), None);
        assert_eq!(prices.cost(&used, Some("onehpc"), october), None);
        assert_eq!(Prices::of(&cards, None).cost(&used, None, june), None);
        assert_eq!(
            Prices::of(&cards, Some("gone")).cost(&used, None, june),
            None
        );
    }

    #[test]
    fn long_context_rates_apply_above_their_threshold_and_a_missing_price_is_unknown() {
        let cards = [
            json!({"api":"foundry","pricingKey":"gpt","effectiveFrom":"2026-09-22T00:00:00.000Z",
                "inputPerMTok":2,"cacheWritePerMTok":null,"cacheReadPerMTok":0.2,"outputPerMTok":10,
                "longContext":{"inputPerMTok":4,"cacheWritePerMTok":null,"cacheReadPerMTok":0.4,
                    "outputPerMTok":15}}),
            json!({"api":"agentic","pricingKey":"gpt","upstreamProvider":"azure_openai",
                "effectiveFrom":"2026-09-22T00:00:00.000Z",
                "inputPerMTok":1,"cacheWritePerMTok":1,"cacheReadPerMTok":1,"outputPerMTok":1,
                "longContext":{"inputPerMTok":2,"cacheWritePerMTok":2,"cacheReadPerMTok":2,
                    "outputPerMTok":2},"longContextThresholdTokens":100_000}),
        ];
        let prices = Prices::of(&cards, Some("gpt"));
        let now = seconds("2026-11-04T00:00:00Z").unwrap();
        close(
            prices.cost(&usage(272_000, 72_000, 0, 1_000), None, now),
            0.4 + 0.0144 + 0.01,
        );
        close(
            prices.cost(&usage(272_001, 72_000, 0, 1_000), None, now),
            200_001.0 * 4e-6 + 0.0288 + 0.015,
        );
        close(
            prices.cost(&usage(100_001, 0, 0, 0), Some("azure_openai"), now),
            100_001.0 * 2e-6,
        );
        // Cache writes without a price make the cost unknown; none written cost nothing.
        assert_eq!(prices.cost(&usage(1_000, 0, 10, 1), None, now), None);
        let unknown_output = ProviderUsage {
            output_tokens: None,
            ..usage(1_000, 0, 0, 0)
        };
        assert_eq!(prices.cost(&unknown_output, None, now), None);
    }
}
