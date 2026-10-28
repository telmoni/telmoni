//! The guard: a run's opt-in caps on its steps and its spend, counted in this
//! process with no call to Telmoni, refused at the step, model call or tool
//! call the run would open past one.
//!
//! A call's cost is counted as the server counts it: as reported, else its
//! tokens at its price in the table this release carries (`model_prices.json`,
//! LiteLLM's, whose notice it holds), else nothing; so a call with neither
//! stops nothing. A price handed to the client comes before the table's, and
//! the server never sees it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::config::Price;
use crate::error::{Cap, Error};
use crate::lock;
use crate::warnings::Warner;

/// The price table this release carries.
const PRICE_FILE: &str = include_str!("model_prices.json");

/// One name's price, each in US dollars a token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rate {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

/// A call's counts as sent, one left off counting 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Tokens {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
}

impl Rate {
    /// What `tokens` cost: the input less the cache's at the input price,
    /// each cache count at its own, and the output at the output price.
    fn cost(self, tokens: Tokens) -> f64 {
        let cached = tokens.cache_read.saturating_add(tokens.cache_write);
        let uncached = tokens.input.saturating_sub(cached);
        count(uncached) * self.input
            + count(tokens.cache_read) * self.cache_read
            + count(tokens.cache_write) * self.cache_write
            + count(tokens.output) * self.output
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count past 2^53 tokens is no call's"
)]
fn count(n: u64) -> f64 {
    n as f64
}

/// Prices by provider, then model: a project's own, or the table's. Nested,
/// so a lookup borrows the names a call carries rather than owning a key.
pub(crate) type OwnRates = HashMap<String, HashMap<String, Rate>>;

/// The rate `rates` holds for `model` of `provider`.
fn rate<'a>(rates: &'a OwnRates, provider: &str, model: &str) -> Option<&'a Rate> {
    rates.get(provider)?.get(model)
}

#[derive(Default, Deserialize)]
struct File {
    prices: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    provider: String,
    model: String,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

/// The price table this release carries, read once per process.
fn table() -> &'static OwnRates {
    static TABLE: OnceLock<OwnRates> = OnceLock::new();
    TABLE.get_or_init(|| {
        // A test holds the file to the core's, so this never fails; were it
        // to, nothing would be priced.
        let file: File = serde_json::from_str(PRICE_FILE).unwrap_or_default();
        let mut rates = OwnRates::new();
        for row in file.prices {
            let rate = Rate {
                input: row.input,
                output: row.output,
                cache_read: row.cache_read,
                cache_write: row.cache_write,
            };
            rates
                .entry(row.provider)
                .or_default()
                .insert(row.model, rate);
        }
        rates
    })
}

/// A project's own prices by name, a name given twice its last; one that is
/// no price is left out with one warning, since a client cannot fail.
pub(crate) fn own_rates(prices: &[Price], warner: &Warner) -> OwnRates {
    let mut rates = OwnRates::new();
    for price in prices {
        let cache_read = price.cache_read.unwrap_or(price.input);
        let cache_write = price.cache_write.unwrap_or(price.input);
        let amounts = [price.input, price.output, cache_read, cache_write];
        if price.provider.is_empty()
            || price.model.is_empty()
            || !amounts
                .iter()
                .all(|amount| amount.is_finite() && *amount >= 0.0)
        {
            warner.once(
                "price".to_owned(),
                "telmoni: a price names its provider and its model, each a finite number of US dollars a token from 0; one that does not is left out",
            );
            continue;
        }
        rates.entry(price.provider.clone()).or_default().insert(
            price.model.clone(),
            Rate {
                input: price.input,
                output: price.output,
                cache_read,
                cache_write,
            },
        );
    }
    rates
}

/// What a call that reported no cost cost: at its own price, else at the
/// table's, by its provider and model as sent, whole; `None` for a name
/// neither holds.
pub(crate) fn priced(own: &OwnRates, provider: &str, model: &str, tokens: Tokens) -> Option<f64> {
    rate(own, provider, model)
        .or_else(|| rate(table(), provider, model))
        .map(|rate| rate.cost(tokens))
}

/// What opens under a run: a step, which its step cap counts, or a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opening {
    Step,
    Call,
}

/// One run's caps, and what it has used of them.
#[derive(Debug, Default)]
pub(crate) struct Guard {
    max_steps: Option<u64>,
    max_spend: Option<f64>,
    used: Mutex<Used>,
}

#[derive(Debug, Default)]
struct Used {
    steps: u64,
    spent: f64,
}

impl Guard {
    pub(crate) fn new(max_steps: Option<u64>, max_spend: Option<f64>) -> Self {
        Self {
            max_steps,
            max_spend,
            used: Mutex::new(Used::default()),
        }
    }

    /// What the run's calls have cost so far, in US dollars.
    pub(crate) fn spent(&self) -> f64 {
        lock(&self.used).spent
    }

    /// Whether the run has a spend cap, which a call priced at nothing never
    /// reaches.
    pub(crate) fn capped(&self) -> bool {
        self.max_spend.is_some()
    }

    /// Counts an opening, or refuses where the run reached a cap: its spend
    /// for whatever opens, its steps for a step.
    pub(crate) fn opens(&self, opening: Opening) -> Result<(), Error> {
        let mut used = lock(&self.used);
        if let Some(limit) = self.max_spend
            && used.spent >= limit
        {
            return Err(Error::CapExceeded {
                cap: Cap::Spend,
                limit,
                used: used.spent,
            });
        }
        if opening == Opening::Step {
            if let Some(limit) = self.max_steps
                && used.steps >= limit
            {
                return Err(Error::CapExceeded {
                    cap: Cap::Steps,
                    limit: count(limit),
                    used: count(used.steps),
                });
            }
            used.steps += 1;
        }
        Ok(())
    }

    /// A call's cost counted again: `now` in place of what `counted` held.
    pub(crate) fn count(&self, counted: &mut f64, now: f64) {
        let mut used = lock(&self.used);
        used.spent += now - *counted;
        *counted = now;
    }
}
