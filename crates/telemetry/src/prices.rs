//! What a model call costs when it reports no cost of its own: the price per
//! token, in US dollars, of each provider's model under every name it answers
//! with. The prices are LiteLLM's (`BerriAI/litellm`, MIT), converted at a
//! pinned commit into the price file, `model_prices.json`, by `make prices`,
//! and written over `telemetry.model_prices` by every `telmoni-server migrate`
//! ([`load`]), so the table holds the file the binary was built with.
//!
//! ⚠ **A name no row holds is unpriced, never guessed.** A call's provider and
//! model are matched whole, as they arrived: no case folded, no snapshot date
//! or routing prefix taken off, no nearest name tried, since a wrong price
//! passes for a right one where a missing one can be named.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::contract::{MODEL_MAX, PROVIDER_MAX};

/// The price file, embedded so `migrate` writes the prices the binary was
/// built with.
pub const FILE: &str = include_str!("../model_prices.json");

/// The price file: the prices, and the file they were converted from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceFile {
    /// Where the prices came from.
    pub source: Source,
    /// Every name's prices, by provider, model and the day each takes effect.
    pub prices: Vec<ModelPrice>,
}

/// LiteLLM's file at the pinned commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The repository.
    pub repository: String,
    /// The file, within it.
    pub path: String,
    /// The commit pinned.
    pub commit: String,
    /// The SHA-256 of the file's bytes at that commit, in hex.
    pub sha256: String,
    /// Its licence, a line an entry: MIT's, whose notice goes with every copy.
    pub license: Vec<String>,
}

/// One name's price, each of its four in US dollars a token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    /// The provider, as the GenAI conventions spell it (`gen_ai.provider.name`).
    pub provider: String,
    /// The model, as the provider answers with it.
    pub model: String,
    /// The day the price takes effect, from midnight UTC; none for always.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<NaiveDate>,
    /// An input token no cache served.
    pub input: f64,
    /// An output token, reasoning among them.
    pub output: f64,
    /// An input token read from the provider's cache.
    pub cache_read: f64,
    /// An input token written to the provider's cache.
    pub cache_write: f64,
}

/// A call's token counts, as `spans` keeps them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Every input token, the cache's reads and writes among them.
    pub input_tokens: u64,
    /// Every output token.
    pub output_tokens: u64,
    /// Input tokens read from the provider's cache.
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the provider's cache.
    pub cache_write_input_tokens: u64,
}

impl ModelPrice {
    /// What `usage` costs at this price, in US dollars: its input less the
    /// cache's at the input price, each cache count at its own, and its
    /// output at the output price.
    #[must_use]
    pub fn cost(&self, usage: Usage) -> f64 {
        let cached = usage
            .cache_read_input_tokens
            .saturating_add(usage.cache_write_input_tokens);
        // Input counted without the cache's, against the conventions, prices none.
        let uncached = usage.input_tokens.saturating_sub(cached);
        tokens(uncached) * self.input
            + tokens(usage.cache_read_input_tokens) * self.cache_read
            + tokens(usage.cache_write_input_tokens) * self.cache_write
            + tokens(usage.output_tokens) * self.output
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count past 2^53 tokens is no call's"
)]
fn tokens(count: u64) -> f64 {
    count as f64
}

impl PriceFile {
    /// The price file the binary was built with, held to its rules.
    pub fn embedded() -> anyhow::Result<Self> {
        Self::parse(FILE)
    }

    /// A price file, held to its rules.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let file: Self = serde_json::from_str(text).context("reading the price file")?;
        file.check()?;
        Ok(file)
    }

    /// Every row a name a span can carry, priced at numbers the table takes,
    /// and none of them twice, so a broken file stops `migrate` before it
    /// touches Postgres.
    fn check(&self) -> anyhow::Result<()> {
        ensure!(
            is_hex(&self.source.commit, 40) && is_hex(&self.source.sha256, 64),
            "the price file names no commit and digest it was converted at"
        );
        ensure!(!self.prices.is_empty(), "the price file holds no price");
        let mut seen = HashSet::new();
        for row in &self.prices {
            ensure!(
                is_name(&row.provider, PROVIDER_MAX) && is_name(&row.model, MODEL_MAX),
                "the price file names {:?} {:?}, which no span can carry",
                row.provider,
                row.model
            );
            for price in [row.input, row.output, row.cache_read, row.cache_write] {
                ensure!(
                    price.is_finite() && price >= 0.0,
                    "the price file prices {} {} at {price}",
                    row.provider,
                    row.model
                );
            }
            ensure!(
                seen.insert((&row.provider, &row.model, row.effective_from)),
                "the price file prices {} {} twice from the same day",
                row.provider,
                row.model
            );
        }
        Ok(())
    }
}

/// Lower-case hex of `len` digits.
fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Text a span keeps under a cap of `max` characters, as ingest reads it.
fn is_name(text: &str, max: usize) -> bool {
    !text.is_empty() && text.trim() == text && text.chars().count() <= max
}

/// Write `file`'s prices over `telemetry.model_prices`, as its owner, the
/// migrator: in one transaction, so a reader finds the old prices or the
/// new, never a part. Answers how many it wrote.
pub async fn load(pool: &PgPool, file: &PriceFile) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;
    // A second `migrate` waits here for the first, not colliding with its rows.
    sqlx::query("LOCK TABLE telemetry.model_prices IN EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .context("locking the model prices")?;
    sqlx::query("DELETE FROM telemetry.model_prices")
        .execute(&mut *tx)
        .await?;
    let rows = &file.prices;
    let written = sqlx::query(
        "INSERT INTO telemetry.model_prices
             (provider, model, effective_from, input, output, cache_read, cache_write)
         SELECT * FROM UNNEST($1::text[], $2::text[], $3::date[], $4::float8[], $5::float8[],
                              $6::float8[], $7::float8[])",
    )
    .bind(rows.iter().map(|r| r.provider.as_str()).collect::<Vec<_>>())
    .bind(rows.iter().map(|r| r.model.as_str()).collect::<Vec<_>>())
    .bind(rows.iter().map(|r| r.effective_from).collect::<Vec<_>>())
    .bind(rows.iter().map(|r| r.input).collect::<Vec<_>>())
    .bind(rows.iter().map(|r| r.output).collect::<Vec<_>>())
    .bind(rows.iter().map(|r| r.cache_read).collect::<Vec<_>>())
    .bind(rows.iter().map(|r| r.cache_write).collect::<Vec<_>>())
    .execute(&mut *tx)
    .await
    .context("writing the model prices")?
    .rows_affected();
    tx.commit().await?;
    Ok(usize::try_from(written)?)
}

/// The prices, by provider and model, each name's in the order they take
/// effect.
#[derive(Debug, Clone, Default)]
pub struct PriceBook {
    prices: HashMap<String, HashMap<String, Vec<ModelPrice>>>,
}

impl PriceBook {
    /// A book of `prices`.
    pub fn new(prices: impl IntoIterator<Item = ModelPrice>) -> Self {
        let mut book = Self::default();
        for row in prices {
            book.prices
                .entry(row.provider.clone())
                .or_default()
                .entry(row.model.clone())
                .or_default()
                .push(row);
        }
        for rows in book.prices.values_mut().flat_map(HashMap::values_mut) {
            rows.sort_by_key(|row| row.effective_from);
        }
        book
    }

    /// Every price `telemetry.model_prices` holds.
    pub async fn read(pool: &PgPool) -> sqlx::Result<Self> {
        let rows: Vec<ModelPrice> = sqlx::query_as(
            "SELECT provider, model, effective_from, input, output, cache_read, cache_write
               FROM telemetry.model_prices",
        )
        .fetch_all(pool)
        .await?;
        Ok(Self::new(rows))
    }

    /// The prices `telemetry.model_prices` holds for `names`, each a provider
    /// and a model, from every day; none for a name no row holds.
    pub async fn read_for(pool: &PgPool, names: &[(String, String)]) -> sqlx::Result<Self> {
        if names.is_empty() {
            return Ok(Self::default());
        }
        let rows: Vec<ModelPrice> = sqlx::query_as(
            "SELECT provider, model, effective_from, input, output, cache_read, cache_write
               FROM telemetry.model_prices
              WHERE (provider, model) IN (SELECT * FROM UNNEST($1::text[], $2::text[]))",
        )
        .bind(names.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>())
        .bind(names.iter().map(|(_, m)| m.as_str()).collect::<Vec<_>>())
        .fetch_all(pool)
        .await?;
        Ok(Self::new(rows))
    }

    /// Every row, each name's in the order they take effect.
    pub fn rows(&self) -> impl Iterator<Item = &ModelPrice> {
        self.prices.values().flat_map(HashMap::values).flatten()
    }

    /// The price in force for a call of `provider`'s `model` at `at`: the
    /// last to take effect by its day. None for a name no row holds, or a
    /// call before its first price.
    #[must_use]
    pub fn price(&self, provider: &str, model: &str, at: DateTime<Utc>) -> Option<&ModelPrice> {
        let day = at.date_naive();
        self.prices
            .get(provider)?
            .get(model)?
            .iter()
            .rev()
            .find(|row| row.effective_from.is_none_or(|from| from <= day))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(provider: &str, model: &str, effective_from: Option<&str>, input: f64) -> ModelPrice {
        ModelPrice {
            provider: provider.to_owned(),
            model: model.to_owned(),
            effective_from: effective_from.map(|day| day.parse().unwrap()),
            input,
            output: 4.0 * input,
            cache_read: input / 10.0,
            cache_write: 1.25 * input,
        }
    }

    fn at(instant: &str) -> DateTime<Utc> {
        instant.parse().unwrap()
    }

    fn file(prices: Vec<ModelPrice>) -> PriceFile {
        PriceFile {
            source: Source {
                repository: "https://github.com/BerriAI/litellm".to_owned(),
                path: "model_prices_and_context_window.json".to_owned(),
                commit: "a".repeat(40),
                sha256: "b".repeat(64),
                license: vec!["MIT License".to_owned()],
            },
            prices,
        }
    }

    /// The embedded file holds its rules, its rows in the converter's order.
    #[test]
    fn the_embedded_file_holds_its_rules_in_order() {
        let file = PriceFile::embedded().unwrap();
        let keys: Vec<_> = file
            .prices
            .iter()
            .map(|r| (&r.provider, &r.model, r.effective_from))
            .collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] < pair[1]),
            "the price file's rows are out of order"
        );
        assert_eq!(
            file.source.license.first().map(String::as_str),
            Some("MIT License")
        );
    }

    #[test]
    fn a_call_is_priced_as_its_input_less_the_cache_and_each_count_at_its_own() {
        let price = ModelPrice {
            input: 3e-6,
            output: 15e-6,
            cache_read: 0.3e-6,
            cache_write: 3.75e-6,
            ..row("anthropic", "claude-sonnet-4-5", None, 0.0)
        };
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 50,
            cache_read_input_tokens: 200,
            cache_write_input_tokens: 100,
        };
        let expected = 700.0 * 3e-6 + 200.0 * 0.3e-6 + 100.0 * 3.75e-6 + 50.0 * 15e-6;
        assert!(
            (price.cost(usage) - expected).abs() < 1e-15,
            "{}",
            price.cost(usage)
        );
        assert!(price.cost(Usage::default()).abs() < f64::EPSILON);

        let short = Usage {
            input_tokens: 100,
            cache_read_input_tokens: 300,
            ..Usage::default()
        };
        assert!(
            (price.cost(short) - 300.0 * 0.3e-6).abs() < 1e-15,
            "a cache past the input prices no input at the input price"
        );
    }

    #[test]
    fn the_price_in_force_is_the_last_to_take_effect_by_the_calls_day() {
        let book = PriceBook::new([
            row("openai", "gpt-4o", Some("2027-03-01"), 2e-6),
            row("openai", "gpt-4o", None, 2.5e-6),
            row("deepseek", "deepseek-chat", Some("2027-03-01"), 1e-6),
        ]);
        let input =
            |provider, model, instant| book.price(provider, model, at(instant)).map(|p| p.input);
        assert_eq!(
            input("openai", "gpt-4o", "2026-10-26T12:00:00Z"),
            Some(2.5e-6)
        );
        assert_eq!(
            input("openai", "gpt-4o", "2027-02-28T23:59:59Z"),
            Some(2.5e-6)
        );
        assert_eq!(
            input("openai", "gpt-4o", "2027-03-01T00:00:00Z"),
            Some(2e-6)
        );
        assert_eq!(
            input("deepseek", "deepseek-chat", "2027-02-28T23:59:59Z"),
            None,
            "a call before a name's first price is unpriced"
        );
        assert_eq!(
            input("deepseek", "deepseek-chat", "2027-03-01T00:00:00Z"),
            Some(1e-6)
        );
    }

    /// ⚠ A name is the provider's and the model's as they arrived, whole.
    #[test]
    fn a_name_no_row_holds_is_unpriced() {
        let book = PriceBook::new([row("openai", "gpt-4o-2024-08-06", None, 2.5e-6)]);
        let now = at("2026-10-26T12:00:00Z");
        assert!(book.price("openai", "gpt-4o-2024-08-06", now).is_some());
        for (provider, model) in [
            ("openai", "gpt-4o"),
            ("openai", "GPT-4o-2024-08-06"),
            ("openai", "openai/gpt-4o-2024-08-06"),
            ("openai", "gpt-4o-2024-08-06 "),
            ("OpenAI", "gpt-4o-2024-08-06"),
            ("azure.ai.openai", "gpt-4o-2024-08-06"),
        ] {
            assert!(
                book.price(provider, model, now).is_none(),
                "{provider} {model}"
            );
        }
    }

    #[test]
    fn a_file_that_breaks_a_rule_is_refused() {
        assert!(
            file(vec![row("openai", "gpt-4o", None, 2.5e-6)])
                .check()
                .is_ok()
        );
        assert!(file(vec![]).check().is_err(), "no price");

        let mut unpinned = file(vec![row("openai", "gpt-4o", None, 2.5e-6)]);
        unpinned.source.commit = "de5dc5b".to_owned();
        assert!(unpinned.check().is_err(), "a commit is its forty digits");

        for broken in [
            row("openai", "gpt-4o", None, -1e-6),
            row("openai", "gpt-4o", None, f64::INFINITY),
            row("openai", "gpt-4o", None, f64::NAN),
            row("openai", "", None, 1e-6),
            row("openai", " gpt-4o", None, 1e-6),
            row("openai", &"m".repeat(MODEL_MAX + 1), None, 1e-6),
            row(&"p".repeat(PROVIDER_MAX + 1), "gpt-4o", None, 1e-6),
        ] {
            assert!(file(vec![broken.clone()]).check().is_err(), "{broken:?}");
        }
        assert!(
            file(vec![row("openai", &"m".repeat(MODEL_MAX), None, 1e-6)])
                .check()
                .is_ok()
        );

        let twice = file(vec![
            row("openai", "gpt-4o", Some("2027-03-01"), 2e-6),
            row("openai", "gpt-4o", Some("2027-03-01"), 2.5e-6),
        ]);
        assert!(twice.check().is_err(), "one name priced twice from one day");
        let dated = file(vec![
            row("openai", "gpt-4o", None, 2.5e-6),
            row("openai", "gpt-4o", Some("2027-03-01"), 2e-6),
        ]);
        assert!(dated.check().is_ok(), "a later price beside the first");

        assert!(
            PriceFile::parse(r#"{"source":{},"prices":[],"extra":1}"#).is_err(),
            "a field the file does not have"
        );
    }
}
