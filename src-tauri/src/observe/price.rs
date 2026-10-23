//! R9: the one price table. USD per million tokens, Anthropic first-party
//! API list prices (2026-09). An **estimate**: a subscription account pays
//! nothing per token, and prices change. Cache writes: 5-minute at 1.25x
//! input, 1-hour at 2x input.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

const fn price(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_write_5m: input * 1.25,
        cache_write_1h: input * 2.0,
        cache_read,
    }
}

/// (model id fragment, price, context window). First match wins, so the
/// more specific fragments come first.
const TABLE: &[(&str, Price, u64)] = &[
    ("fable-5-1", price(10.0, 50.0, 0.25), 1_000_000),
    ("fable", price(10.0, 50.0, 1.0), 1_000_000),
    ("mythos", price(10.0, 50.0, 1.0), 1_000_000),
    ("opus-5-5", price(4.0, 20.0, 0.20), 1_000_000),
    ("opus-5", price(5.0, 25.0, 0.50), 1_000_000),
    ("opus-4", price(5.0, 25.0, 0.50), 200_000),
    ("opus", price(5.0, 25.0, 0.50), 200_000),
    ("sonnet-5", price(2.0, 10.0, 0.20), 1_000_000),
    ("sonnet-4", price(3.0, 15.0, 0.30), 200_000),
    ("sonnet", price(3.0, 15.0, 0.30), 200_000),
    ("haiku", price(1.0, 5.0, 0.10), 200_000),
];

fn entry(model: &str) -> Option<&'static (&'static str, Price, u64)> {
    let model = model.to_ascii_lowercase();
    TABLE
        .iter()
        .find(|(fragment, _, _)| model.contains(fragment))
}

pub fn price_for(model: &str) -> Option<Price> {
    entry(model).map(|(_, p, _)| *p)
}

/// The model's context window as Claude Code uses it (A4 meter).
pub fn context_window(model: &str) -> u64 {
    let base = entry(model).map(|(_, _, w)| *w).unwrap_or(200_000);
    if model.contains("[1m]") {
        1_000_000
    } else {
        base
    }
}

/// Token counts of one or more API messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl Tokens {
    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.cache_read += other.cache_read;
    }

    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write_5m + self.cache_write_1h + self.cache_read
    }

    /// `message.usage`: the 5m/1h split when Claude Code reports it, else
    /// all cache writes as 5-minute ones.
    pub fn from_usage(usage: &serde_json::Value) -> Self {
        let n = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let writes = n(usage, "cache_creation_input_tokens");
        let (w5, w1) = match usage.get("cache_creation") {
            Some(split) if split.is_object() => (
                n(split, "ephemeral_5m_input_tokens"),
                n(split, "ephemeral_1h_input_tokens"),
            ),
            _ => (writes, 0),
        };
        Self {
            input: n(usage, "input_tokens"),
            output: n(usage, "output_tokens"),
            cache_write_5m: w5,
            cache_write_1h: w1,
            cache_read: n(usage, "cache_read_input_tokens"),
        }
    }

    /// USD estimate, None for a model not in the table.
    pub fn cost(&self, model: &str) -> Option<f64> {
        let p = price_for(model)?;
        let m = 1_000_000.0;
        Some(
            (self.input as f64 * p.input
                + self.output as f64 * p.output
                + self.cache_write_5m as f64 * p.cache_write_5m
                + self.cache_write_1h as f64 * p.cache_write_1h
                + self.cache_read as f64 * p.cache_read)
                / m,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn models_are_priced_by_family_and_unknown_ones_are_not_guessed() {
        assert_eq!(price_for("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(price_for("claude-opus-4-8").unwrap().output, 25.0);
        assert_eq!(price_for("claude-sonnet-5-5").unwrap().cache_read, 0.20);
        assert_eq!(
            price_for("claude-haiku-4-5-20251001")
                .unwrap()
                .cache_write_1h,
            2.0
        );
        assert!(price_for("gpt-x").is_none());
        assert_eq!(context_window("claude-haiku-4-5"), 200_000);
        assert_eq!(context_window("claude-opus-5-5"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-4-6[1m]"), 1_000_000);
    }

    #[test]
    fn cost_math_uses_each_bucket_once() {
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            cache_read: 1_000_000,
        };
        // haiku: 1 + 5 + 1.25 + 2 + 0.10
        assert!((t.cost("claude-haiku-4-5").unwrap() - 9.35).abs() < 1e-9);
        assert!(Tokens::default().cost("claude-haiku-4-5").unwrap().abs() < 1e-12);
        let split = Tokens::from_usage(&json!({
            "input_tokens": 9, "output_tokens": 348, "cache_read_input_tokens": 13803,
            "cache_creation_input_tokens": 13476,
            "cache_creation": {"ephemeral_1h_input_tokens": 13476, "ephemeral_5m_input_tokens": 0}
        }));
        assert_eq!((split.cache_write_5m, split.cache_write_1h), (0, 13476));
        let flat = Tokens::from_usage(&json!({"cache_creation_input_tokens": 50}));
        assert_eq!((flat.cache_write_5m, flat.cache_write_1h), (50, 0));
    }
}
