//! The price file: converted from LiteLLM's at its pinned commit by `make
//! prices` (`UPDATE_PRICES`, and `LITELLM_COMMIT` to move the pin), held to
//! the conversion's rules, and written over the table by `migrate`.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a panicking helper is a failing test"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use telmoni_shared::digest::hex_lower;
use telmoni_shared::test_util::{ServiceRole, service_pool};
use telmoni_telemetry::prices::{self, ModelPrice, PriceBook, PriceFile, Source};

/// LiteLLM's repository, and its file of prices there.
const REPOSITORY: &str = "https://github.com/BerriAI/litellm";
const PATH: &str = "model_prices_and_context_window.json";

/// Every provider the GenAI conventions name, at the commit the contract
/// pins (`gen_ai.provider.name`).
const CONVENTIONS: [&str; 16] = [
    "anthropic",
    "aws.bedrock",
    "azure.ai.inference",
    "azure.ai.openai",
    "cohere",
    "deepseek",
    "gcp.gemini",
    "gcp.gen_ai",
    "gcp.vertex_ai",
    "groq",
    "ibm.watsonx.ai",
    "mistral_ai",
    "moonshot_ai",
    "openai",
    "perplexity",
    "x_ai",
];

/// The modes whose calls are priced by the token.
const MODES: [&str; 4] = ["chat", "completion", "responses", "embedding"];

/// The conventions' name for a provider LiteLLM names, the routing prefix
/// LiteLLM's keys for it may carry, and whether a name it answers with holds
/// a `/` of its own; none for a provider the conventions do not name.
fn provider(litellm: &str) -> Option<(&'static str, &'static str, bool)> {
    Some(match litellm {
        "anthropic" => ("anthropic", "anthropic/", false),
        "azure" | "azure_text" => ("azure.ai.openai", "azure/", false),
        "azure_ai" => ("azure.ai.inference", "azure_ai/", false),
        "bedrock" | "bedrock_converse" => ("aws.bedrock", "bedrock/", false),
        "codestral" => ("mistral_ai", "codestral/", false),
        "cohere" | "cohere_chat" => ("cohere", "cohere/", false),
        "deepseek" => ("deepseek", "deepseek/", false),
        "gemini" => ("gcp.gemini", "gemini/", false),
        "groq" => ("groq", "groq/", true),
        "mistral" => ("mistral_ai", "mistral/", false),
        "moonshot" => ("moonshot_ai", "moonshot/", false),
        "openai" | "text-completion-openai" => ("openai", "openai/", false),
        "perplexity" => ("perplexity", "perplexity/", false),
        "text-completion-codestral" => ("mistral_ai", "text-completion-codestral/", false),
        "watsonx" => ("ibm.watsonx.ai", "watsonx/", true),
        "xai" => ("x_ai", "xai/", false),
        // Vertex AI's families: its own models and each partner's it serves.
        vertex if vertex == "vertex_ai" || vertex.starts_with("vertex_ai-") => {
            ("gcp.vertex_ai", "vertex_ai/", true)
        }
        _ => return None,
    })
}

/// One name's four prices as LiteLLM's entries give them, each absent where
/// none says.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Given {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

/// LiteLLM's entries as the price file's rows, sorted, by the rules
/// `ARCHITECTURE.md` gives (*Model prices*); and the names left unpriced
/// because two of its entries price them differently.
fn convert(litellm: &Value) -> (Vec<ModelPrice>, Vec<String>) {
    let entries = litellm.as_object().expect("LiteLLM's file is an object");
    let mut names: BTreeMap<(String, String), Vec<Given>> = BTreeMap::new();
    for (key, entry) in entries {
        let Some((to, prefix, slashes)) = entry["litellm_provider"].as_str().and_then(provider)
        else {
            continue;
        };
        let mode = entry["mode"].as_str().unwrap_or_default();
        if !MODES.contains(&mode) {
            continue;
        }
        let model = key.strip_prefix(prefix).unwrap_or(key);
        // A fine-tune's base, a region's or a data zone's price: no span names one.
        if model.starts_with("ft:") || (!slashes && model.contains('/')) {
            continue;
        }
        // Missing or null is a price not given; anything else not a price, no entry.
        let price = |field: &str| match &entry[field] {
            Value::Null => Ok(None),
            given => given
                .as_f64()
                .filter(|p| p.is_finite() && *p >= 0.0)
                .map(Some)
                .ok_or(()),
        };
        let (Ok(input), Ok(output), Ok(cache_read), Ok(cache_write)) = (
            price("input_cost_per_token"),
            price("output_cost_per_token"),
            price("cache_read_input_token_cost"),
            price("cache_creation_input_token_cost"),
        ) else {
            continue;
        };
        let output = output.or((mode == "embedding").then_some(0.0));
        let (Some(input), Some(output)) = (input, output) else {
            continue;
        };
        // LiteLLM's 0 for a price it does not know.
        if input == 0.0 && output == 0.0 {
            continue;
        }
        names
            .entry((to.to_owned(), model.to_owned()))
            .or_default()
            .push(Given {
                input,
                output,
                cache_read,
                cache_write,
            });
    }

    let mut rows = Vec::new();
    let mut unpriced = Vec::new();
    for ((provider, model), given) in names {
        let Some(price) = agreed(&given) else {
            unpriced.push(format!("{provider} {model}"));
            continue;
        };
        rows.push(ModelPrice {
            provider,
            model,
            effective_from: None,
            input: price.input,
            output: price.output,
            cache_read: price.cache_read.unwrap_or(price.input),
            cache_write: price.cache_write.unwrap_or(price.input),
        });
    }
    (rows, unpriced)
}

/// The price `given` agrees on, each cache price taken from any entry that
/// gives one; none where two entries give one price differently.
fn agreed(given: &[Given]) -> Option<Given> {
    let mut agreed = *given.first()?;
    for entry in given {
        if entry.input != agreed.input || entry.output != agreed.output {
            return None;
        }
        agreed.cache_read = merged(agreed.cache_read, entry.cache_read)?;
        agreed.cache_write = merged(agreed.cache_write, entry.cache_write)?;
    }
    Some(agreed)
}

/// Two entries' cache price as one: either's where only one gives it, none
/// at all where they differ.
fn merged(one: Option<f64>, other: Option<f64>) -> Option<Option<f64>> {
    match (one, other) {
        (Some(one), Some(other)) if one != other => None,
        (one, other) => Some(one.or(other)),
    }
}

/// MIT's notice from LiteLLM's licence, a line an entry: the file's prices
/// are taken under it alone, so a licence that is not MIT's has none.
fn mit_notice(license: &str) -> Option<Vec<String>> {
    let mut lines: Vec<String> = license
        .lines()
        .skip_while(|line| line.trim() != "MIT License")
        .map(|line| line.trim_end().to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
        .iter()
        .any(|line| line.starts_with("Copyright (c)"))
        .then_some(lines)
}

/// The price file as it is kept: its source laid out, and one price a line,
/// so a moved pin reads as a diff of the prices it moved.
fn render(file: &PriceFile) -> String {
    let source = serde_json::to_string_pretty(&file.source).expect("the source as JSON");
    let rows: Vec<String> = file
        .prices
        .iter()
        .map(|row| {
            format!(
                "    {}",
                serde_json::to_string(row).expect("a price as JSON")
            )
        })
        .collect();
    format!(
        "{{\n  \"source\": {},\n  \"prices\": [\n{}\n  ]\n}}\n",
        source.replace('\n', "\n  "),
        rows.join(",\n")
    )
}

fn price_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("model_prices.json")
}

/// Each SDK's copy of the price file, which its release carries for its guard.
fn sdk_copies() -> [PathBuf; 4] {
    let sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk");
    [
        sdk.join("python/src/telmoni/model_prices.json"),
        sdk.join("typescript/src/model_prices.json"),
        sdk.join("go/model_prices.json"),
        sdk.join("rust/src/model_prices.json"),
    ]
}

/// `path` in LiteLLM's repository at `commit`.
async fn fetch(client: &reqwest::Client, commit: &str, path: &str) -> Vec<u8> {
    let url = format!("https://raw.githubusercontent.com/BerriAI/litellm/{commit}/{path}");
    let answer = client
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .unwrap_or_else(|e| panic!("fetching {url}: {e}"));
    answer
        .bytes()
        .await
        .unwrap_or_else(|e| panic!("reading {url}: {e}"))
        .to_vec()
}

/// `make prices`: LiteLLM's file converted at the pin, its digest unmoved, or at `LITELLM_COMMIT`.
#[tokio::test]
async fn the_price_file_is_litellms_at_its_pin() {
    if std::env::var_os("UPDATE_PRICES").is_none() {
        return;
    }
    let path = price_file_path();
    let current: Option<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let pinned = |field: &str| {
        current
            .as_ref()
            .and_then(|file| file["source"][field].as_str())
            .map(str::to_owned)
    };
    let commit = std::env::var("LITELLM_COMMIT")
        .ok()
        .filter(|commit| !commit.is_empty())
        .or_else(|| pinned("commit"))
        .expect("the price file names no commit: LITELLM_COMMIT=<sha>");
    assert!(
        commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "LITELLM_COMMIT is a commit's forty hex digits, not {commit:?}"
    );
    let commit = commit.to_ascii_lowercase();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("an HTTP client");
    let bytes = fetch(&client, &commit, PATH).await;
    let sha256 = hex_lower(&Sha256::digest(&bytes));
    if pinned("commit").as_deref() == Some(commit.as_str()) {
        assert_eq!(
            pinned("sha256").as_deref(),
            Some(sha256.as_str()),
            "LiteLLM's file at {commit} is not the one the price file was converted from"
        );
    }
    let license = mit_notice(&String::from_utf8_lossy(
        &fetch(&client, &commit, "LICENSE").await,
    ))
    .expect("LiteLLM's licence no longer reads as MIT's: read it before moving the pin");
    let litellm: Value = serde_json::from_slice(&bytes).expect("LiteLLM's file is JSON");
    let (prices, unpriced) = convert(&litellm);
    for name in &unpriced {
        eprintln!("unpriced: LiteLLM prices {name} twice, differently");
    }
    let file = PriceFile {
        source: Source {
            repository: REPOSITORY.to_owned(),
            path: PATH.to_owned(),
            commit,
            sha256,
            license,
        },
        prices,
    };
    let text = render(&file);
    assert_eq!(
        PriceFile::parse(&text).expect("the converted file holds its rules"),
        file,
        "the file reads back as it was written"
    );
    std::fs::write(&path, &text).expect("write the price file");
    for copy in sdk_copies() {
        std::fs::write(&copy, &text).unwrap_or_else(|e| panic!("writing {}: {e}", copy.display()));
    }
    eprintln!("{} prices at {}", file.prices.len(), file.source.commit);
}

/// The file is as `make prices` writes it, each provider one a span names.
#[test]
fn the_price_file_is_as_make_prices_writes_it() {
    let text = std::fs::read_to_string(price_file_path()).expect("the price file");
    let file = PriceFile::parse(&text).expect("the price file holds its rules");
    assert_eq!(render(&file), text, "the price file was edited by hand");
    for copy in sdk_copies() {
        assert!(
            std::fs::read_to_string(&copy).is_ok_and(|carried| carried == text),
            "{} is not the price file: make prices writes it",
            copy.display()
        );
    }
    assert_eq!(file.source.repository, REPOSITORY);
    assert_eq!(file.source.path, PATH);
    for row in &file.prices {
        assert!(CONVENTIONS.contains(&row.provider.as_str()), "{row:?}");
        assert!(
            row.effective_from.is_none(),
            "LiteLLM dates no price, nor does the SDKs' guard read a date: {row:?}"
        );
    }
}

#[test]
fn every_provider_converted_is_one_the_conventions_name() {
    for litellm in [
        "anthropic",
        "azure",
        "azure_ai",
        "azure_text",
        "bedrock",
        "bedrock_converse",
        "codestral",
        "cohere",
        "cohere_chat",
        "deepseek",
        "gemini",
        "groq",
        "mistral",
        "moonshot",
        "openai",
        "perplexity",
        "text-completion-codestral",
        "text-completion-openai",
        "vertex_ai",
        "vertex_ai-anthropic_models",
        "watsonx",
        "xai",
    ] {
        let (to, prefix, _) = provider(litellm).expect(litellm);
        assert!(CONVENTIONS.contains(&to), "{litellm} -> {to}");
        assert!(prefix.ends_with('/'), "{litellm}'s prefix {prefix:?}");
    }
    for unnamed in [
        "fireworks_ai",
        "openrouter",
        "together_ai",
        "vertex",
        "ollama",
    ] {
        assert!(provider(unnamed).is_none(), "{unnamed}");
    }
}

/// What LiteLLM's entries look like, one of each case the conversion decides.
fn litellm() -> Value {
    json!({
        "sample_spec": {
            "litellm_provider": "one of https://docs.litellm.ai/docs/providers",
            "mode": "one of: chat, embedding, completion",
            "input_cost_per_token": 0.0,
            "output_cost_per_token": 0.0
        },
        "gpt-4o": {
            "litellm_provider": "openai", "mode": "chat",
            "input_cost_per_token": 2.5e-6, "output_cost_per_token": 1e-5,
            "cache_read_input_token_cost": 1.25e-6,
            "input_cost_per_token_batches": 1.25e-6
        },
        "o1-pro": {
            "litellm_provider": "openai", "mode": "responses",
            "input_cost_per_token": 1.5e-4, "output_cost_per_token": 6e-4
        },
        "text-embedding-3-small": {
            "litellm_provider": "openai", "mode": "embedding",
            "input_cost_per_token": 2e-8, "output_cost_per_token": 0.0
        },
        "ft:gpt-4o-2024-08-06": {
            "litellm_provider": "openai", "mode": "chat",
            "input_cost_per_token": 3.75e-6, "output_cost_per_token": 1.5e-5
        },
        "low/1024-x-1024/gpt-image-1.5": {
            "litellm_provider": "openai", "mode": "chat",
            "input_cost_per_token": 5e-6, "output_cost_per_token": 4e-5
        },
        "gpt-image-1": {
            "litellm_provider": "openai", "mode": "image_generation",
            "input_cost_per_token": 5e-6, "output_cost_per_token": 4e-5
        },
        "claude-sonnet-4-5": {
            "litellm_provider": "anthropic", "mode": "chat",
            "input_cost_per_token": 3e-6, "output_cost_per_token": 1.5e-5,
            "cache_read_input_token_cost": 3e-7, "cache_creation_input_token_cost": 3.75e-6,
            "cache_creation_input_token_cost_above_1hr": 6e-6
        },
        "gemini/gemini-2.5-flash": {
            "litellm_provider": "gemini", "mode": "chat",
            "input_cost_per_token": 3e-7, "output_cost_per_token": 2.5e-6,
            "cache_read_input_token_cost": 3e-8
        },
        "gemini/gemini-exp-1206": {
            "litellm_provider": "gemini", "mode": "chat",
            "input_cost_per_token": 0, "output_cost_per_token": 0
        },
        "gemini-exp-1206": {
            "litellm_provider": "gemini", "mode": "chat",
            "input_cost_per_token": 3e-7, "output_cost_per_token": 2.5e-6
        },
        "gemini/gemini-2.0-flash-exp": {
            "litellm_provider": "gemini", "mode": "chat",
            "input_cost_per_token": 0, "output_cost_per_token": 0
        },
        "gemini-2.5-flash": {
            "litellm_provider": "vertex_ai-language-models", "mode": "chat",
            "input_cost_per_token": 3e-7, "output_cost_per_token": 2.5e-6
        },
        "vertex_ai/claude-opus-4-5@20251101": {
            "litellm_provider": "vertex_ai-anthropic_models", "mode": "chat",
            "input_cost_per_token": 5e-6, "output_cost_per_token": 2.5e-5
        },
        "vertex_ai/meta/llama-3.3-70b-instruct-maas": {
            "litellm_provider": "vertex_ai-llama_models", "mode": "chat",
            "input_cost_per_token": 7.2e-7, "output_cost_per_token": 7.2e-7
        },
        "medlm-large": {
            "litellm_provider": "vertex_ai-language-models", "mode": "chat",
            "input_cost_per_character": 5e-6, "output_cost_per_character": 1.5e-5
        },
        "anthropic.claude-v2:1": {
            "litellm_provider": "bedrock", "mode": "chat",
            "input_cost_per_token": 8e-6, "output_cost_per_token": 2.4e-5
        },
        "bedrock/eu-central-1/anthropic.claude-v2:1": {
            "litellm_provider": "bedrock", "mode": "chat",
            "input_cost_per_token": 8.5e-6, "output_cost_per_token": 2.55e-5
        },
        "azure/gpt-4o": {
            "litellm_provider": "azure", "mode": "chat",
            "input_cost_per_token": 2.5e-6, "output_cost_per_token": 1e-5
        },
        "azure/eu/gpt-4o": {
            "litellm_provider": "azure", "mode": "chat",
            "input_cost_per_token": 2.75e-6, "output_cost_per_token": 1.1e-5
        },
        "groq/openai/gpt-oss-120b": {
            "litellm_provider": "groq", "mode": "chat",
            "input_cost_per_token": 1.5e-7, "output_cost_per_token": 6e-7
        },
        "mistral/mistral-embed": {
            "litellm_provider": "mistral", "mode": "embedding",
            "input_cost_per_token": 1e-7
        },
        "mistral/codestral-latest": {
            "litellm_provider": "mistral", "mode": "chat",
            "input_cost_per_token": 3e-7, "output_cost_per_token": 9e-7
        },
        "codestral/codestral-latest": {
            "litellm_provider": "codestral", "mode": "chat",
            "input_cost_per_token": 1e-6, "output_cost_per_token": 3e-6
        },
        "deepseek-chat": {
            "litellm_provider": "deepseek", "mode": "chat",
            "input_cost_per_token": 2.8e-7, "output_cost_per_token": 4.2e-7,
            "cache_read_input_token_cost": 2.8e-8
        },
        "deepseek/deepseek-chat": {
            "litellm_provider": "deepseek", "mode": "chat",
            "input_cost_per_token": 2.8e-7, "output_cost_per_token": 4.2e-7,
            "cache_read_input_token_cost": 2.8e-8, "cache_creation_input_token_cost": 0.0
        },
        "deepseek-reasoner": {
            "litellm_provider": "deepseek", "mode": "chat",
            "input_cost_per_token": 5.5e-7, "output_cost_per_token": 2.19e-6,
            "cache_read_input_token_cost": null
        },
        "xai/grok-4.3": {
            "litellm_provider": "xai", "mode": "chat",
            "input_cost_per_token": "3e-6", "output_cost_per_token": 1.5e-5
        },
        "claude-haiku-4-5": {
            "litellm_provider": "anthropic", "mode": "chat",
            "input_cost_per_token": 1e-6, "output_cost_per_token": 5e-6,
            "cache_read_input_token_cost": -1e-7
        },
        "cohere/embed-v4.0": {
            "litellm_provider": "cohere", "mode": "embedding",
            "input_cost_per_token": 1.2e-7, "output_cost_per_token": "n/a"
        },
        "accounts/fireworks/models/llama-v3p1-8b-instruct": {
            "litellm_provider": "fireworks_ai", "mode": "chat",
            "input_cost_per_token": 2e-7, "output_cost_per_token": 2e-7
        }
    })
}

/// ⚠ Each case the conversion decides, decided by its rules.
#[test]
fn litellms_entries_convert_by_the_rules() {
    let (rows, unpriced) = convert(&litellm());
    let got: Vec<(&str, &str, [f64; 4])> = rows
        .iter()
        .map(|r| {
            assert!(r.effective_from.is_none());
            (
                r.provider.as_str(),
                r.model.as_str(),
                [r.input, r.output, r.cache_read, r.cache_write],
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "anthropic",
                "claude-sonnet-4-5",
                [3e-6, 1.5e-5, 3e-7, 3.75e-6]
            ),
            (
                "aws.bedrock",
                "anthropic.claude-v2:1",
                [8e-6, 2.4e-5, 8e-6, 8e-6]
            ),
            ("azure.ai.openai", "gpt-4o", [2.5e-6, 1e-5, 2.5e-6, 2.5e-6]),
            ("deepseek", "deepseek-chat", [2.8e-7, 4.2e-7, 2.8e-8, 0.0]),
            (
                "deepseek",
                "deepseek-reasoner",
                [5.5e-7, 2.19e-6, 5.5e-7, 5.5e-7]
            ),
            ("gcp.gemini", "gemini-2.5-flash", [3e-7, 2.5e-6, 3e-8, 3e-7]),
            ("gcp.gemini", "gemini-exp-1206", [3e-7, 2.5e-6, 3e-7, 3e-7]),
            (
                "gcp.vertex_ai",
                "claude-opus-4-5@20251101",
                [5e-6, 2.5e-5, 5e-6, 5e-6]
            ),
            (
                "gcp.vertex_ai",
                "gemini-2.5-flash",
                [3e-7, 2.5e-6, 3e-7, 3e-7]
            ),
            (
                "gcp.vertex_ai",
                "meta/llama-3.3-70b-instruct-maas",
                [7.2e-7, 7.2e-7, 7.2e-7, 7.2e-7]
            ),
            (
                "groq",
                "openai/gpt-oss-120b",
                [1.5e-7, 6e-7, 1.5e-7, 1.5e-7]
            ),
            ("mistral_ai", "mistral-embed", [1e-7, 0.0, 1e-7, 1e-7]),
            ("openai", "gpt-4o", [2.5e-6, 1e-5, 1.25e-6, 2.5e-6]),
            ("openai", "o1-pro", [1.5e-4, 6e-4, 1.5e-4, 1.5e-4]),
            ("openai", "text-embedding-3-small", [2e-8, 0.0, 2e-8, 2e-8]),
        ]
    );
    assert_eq!(unpriced, ["mistral_ai codestral-latest"]);
}

/// The licence's MIT notice is kept, its preamble aside; one not MIT's stops the conversion.
#[test]
fn the_notice_kept_is_mits() {
    let license = "Portions of this software are licensed as follows:\n\n* enterprise/ is not\n---\n\
                   MIT License\n\nCopyright (c) 2023 Berri AI\n\nPermission is hereby granted.  \n\n";
    assert_eq!(
        mit_notice(license).expect("MIT's notice"),
        [
            "MIT License",
            "",
            "Copyright (c) 2023 Berri AI",
            "",
            "Permission is hereby granted."
        ]
    );
    assert!(mit_notice("Business Source License 1.1\n\nCopyright (c) 2023 Berri AI\n").is_none());
    assert!(mit_notice("MIT License\n\nPermission is hereby granted.\n").is_none());
}

/// ⚠ What every SDK's guard counts of a call (`sdk/fixtures/guard.json`) is
/// what the server's price book makes of it, given the same prices.
#[test]
fn the_sdks_guard_counts_a_call_as_the_server_prices_it() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/fixtures/guard.json");
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the guard's fixture"))
            .expect("the guard's fixture is JSON");
    let handed: Vec<ModelPrice> = fixture["prices"]
        .as_array()
        .expect("prices")
        .iter()
        .map(|price| ModelPrice {
            provider: price["provider"].as_str().expect("a provider").to_owned(),
            model: price["model"].as_str().expect("a model").to_owned(),
            effective_from: None,
            input: price["input"].as_f64().expect("an input price"),
            output: price["output"].as_f64().expect("an output price"),
            cache_read: price["cache_read"]
                .as_f64()
                .or_else(|| price["input"].as_f64())
                .expect("a cache read price"),
            cache_write: price["cache_write"]
                .as_f64()
                .or_else(|| price["input"].as_f64())
                .expect("a cache write price"),
        })
        .collect();
    let book = PriceBook::new(handed);
    for call in fixture["calls"].as_array().expect("calls") {
        // As its span holds it once answered: each attribute as last sent,
        // a cost below 0 never sent.
        let answers: Vec<&Value> = call["answers"]
            .as_array()
            .map_or_else(|| vec![call], |answers| answers.iter().collect());
        let mut usage = serde_json::Map::new();
        let mut model = call["model"].as_str().unwrap_or_default();
        for answer in answers {
            if let Some(sent) = answer["usage"].as_object() {
                usage.extend(
                    sent.iter()
                        .filter(|(key, value)| {
                            *key != "cost" || value.as_f64().is_some_and(|cost| cost >= 0.0)
                        })
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            if let Some(answered_as) = answer["response_model"].as_str() {
                model = answered_as;
            }
        }
        let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let reported = usage.get("cost").and_then(Value::as_f64);
        let priced = book
            .price(
                call["provider"].as_str().unwrap_or_default(),
                model,
                Utc::now(),
            )
            .map(|price| {
                price.cost(prices::Usage {
                    input_tokens: count("input_tokens"),
                    output_tokens: count("output_tokens"),
                    cache_read_input_tokens: count("cache_read_input_tokens"),
                    cache_write_input_tokens: count("cache_write_input_tokens"),
                })
            });
        let server = reported.or(priced).unwrap_or(0.0);
        let counted = call["counted"].as_f64().expect("counted");
        assert!(
            (server - counted).abs() <= 1e-12,
            "{}: the price book prices it at {server}, the fixture counts {counted}",
            call["name"]
        );
    }
}

/// ⚠ `migrate` writes the file over the table, and a row it no longer holds goes.
#[sqlx::test]
async fn migrate_writes_the_price_file_over_the_table(pool: PgPool) {
    sqlx::query(
        "INSERT INTO telemetry.model_prices (provider, model, input, output, cache_read, cache_write)
         VALUES ('openai', 'gpt-retired', 1, 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .expect("a price the file no longer holds");
    let file = PriceFile::embedded().expect("the price file");
    assert_eq!(
        prices::load(&pool, &file).await.expect("the load"),
        file.prices.len()
    );

    let book = PriceBook::read(&service_pool(&pool, ServiceRole::Telemetry))
        .await
        .expect("the module reads the prices");
    let now = Utc::now();
    for row in &file.prices {
        assert_eq!(book.price(&row.provider, &row.model, now), Some(row));
    }
    assert!(book.price("openai", "gpt-retired", now).is_none());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.model_prices")
        .fetch_one(&pool)
        .await
        .expect("count the prices");
    assert_eq!(usize::try_from(rows).ok(), Some(file.prices.len()));

    assert_eq!(
        prices::load(&pool, &file).await.expect("a second load"),
        file.prices.len(),
        "a run over loaded prices writes them again"
    );
}

/// Only `migrate` writes a price: the module's every write is refused.
#[sqlx::test]
async fn the_module_reads_the_prices_and_writes_none(pool: PgPool) {
    let file = PriceFile::embedded().expect("the price file");
    prices::load(&pool, &file).await.expect("the load");
    let module = service_pool(&pool, ServiceRole::Telemetry);
    for statement in [
        "INSERT INTO telemetry.model_prices (provider, model, input, output, cache_read, cache_write)
         VALUES ('openai', 'gpt-mine', 0, 0, 0, 0)",
        "UPDATE telemetry.model_prices SET input = 0",
        "DELETE FROM telemetry.model_prices",
    ] {
        let error = sqlx::query(statement)
            .execute(&module)
            .await
            .expect_err(statement);
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501"),
            "{statement}: {error}"
        );
    }
    let first = file.prices.first().expect("a price");
    assert_eq!(
        PriceBook::read(&module)
            .await
            .expect("the module reads them")
            .price(&first.provider, &first.model, Utc::now()),
        Some(first)
    );
}

/// ⚠ A second `migrate` mid-load waits for the first's commit, then writes over its rows.
#[sqlx::test]
async fn a_load_waits_for_one_in_flight_then_writes_over_it(pool: PgPool) {
    let file = PriceFile::embedded().expect("the price file");
    let first = file.prices.first().expect("a price").clone();
    let mut in_flight = pool.begin().await.expect("a load in flight");
    sqlx::query(
        "INSERT INTO telemetry.model_prices (provider, model, input, output, cache_read, cache_write)
         VALUES ($1, $2, 1, 1, 1, 1)",
    )
    .bind(&first.provider)
    .bind(&first.model)
    .execute(&mut *in_flight)
    .await
    .expect("the load in flight writes a row");

    let second = tokio::spawn({
        let (pool, file) = (pool.clone(), file.clone());
        async move { prices::load(&pool, &file).await }
    });
    // Committed once the second waits on it, so the two meet mid-load.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                                 WHERE datname = current_database() AND wait_event_type = 'Lock')",
            )
            .fetch_one(&pool)
            .await
            .expect("read who waits");
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the second load waits on the first");
    in_flight.commit().await.expect("the load in flight lands");

    assert_eq!(
        second
            .await
            .expect("the second load's task")
            .expect("the second load"),
        file.prices.len()
    );
    let book = PriceBook::read(&pool).await.expect("read the prices");
    assert_eq!(
        book.price(&first.provider, &first.model, Utc::now()),
        Some(&first),
        "the second load's price stands"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM telemetry.model_prices")
        .fetch_one(&pool)
        .await
        .expect("count the prices");
    assert_eq!(usize::try_from(rows).ok(), Some(file.prices.len()));
}
