//! TypeSafe's System One, as Jev and the open-weight models serving its
//! shape take it: a state and typed questions in, each answer with its
//! probabilities out, and no text written.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Connection, Credentials, Prepared, Refused, Usage, tokens};

/// TypeSafe's own endpoint.
const TYPESAFE_BASE: &str = "https://api.typesafe.ai";

/// What a question asks, and what each answer it may give means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Criteria {
    /// Yes or no, each optionally described.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    Noul {
        #[serde(default)]
        yes_means: String,
        #[serde(default)]
        no_means: String,
    },
    /// One of ordered levels, the lowest first.
    #[expect(missing_docs, reason = "the variant says what its field is")]
    Score { levels: Vec<String> },
    /// One of named options.
    #[expect(missing_docs, reason = "the variant says what its field is")]
    Choice { options: Vec<Choice> },
}

/// One option of a choice, and what it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// What the answer names.
    pub name: String,
    /// What it means; its name where none.
    #[serde(default)]
    pub means: String,
}

/// One typed question, under the name its answer's score goes by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// Its answer's name, as its score's.
    pub name: String,
    /// How to judge the state.
    pub instructions: String,
    /// What it asks.
    #[serde(flatten)]
    pub criteria: Criteria,
}

/// One decision asked for: the model, the state it reads, its questions.
#[derive(Debug, Clone)]
pub struct Decide<'a> {
    /// The model's name at the endpoint, such as `jev-latest`.
    pub model: &'a str,
    /// What is judged, and nothing of how to judge it.
    pub state: &'a str,
    /// The questions, each answered on its own.
    pub questions: &'a [Question],
}

/// One question's answer as the model gave it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    /// The probability of yes.
    #[expect(missing_docs, reason = "the variant says what its field is")]
    Noul { noul: f64 },
    /// The levels' probability-weighted value, between the lowest's 0 and the
    /// highest's.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    Score {
        score: f64,
        confidence: Option<f64>,
        probabilities: BTreeMap<String, f64>,
    },
    /// The likeliest option.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    Choice {
        choice: String,
        confidence: Option<f64>,
        probabilities: BTreeMap<String, f64>,
    },
}

/// A decision as the model gave it: the version that answered, each
/// question's answer by name, and what it used.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    /// The model's version that answered, such as `jev-1.14.0`.
    pub model: String,
    /// Each question's answer, by its name.
    pub answers: BTreeMap<String, Answer>,
    /// What it used.
    pub usage: Usage,
}

/// Where a connection's requests go: TypeSafe's, or the endpoint it names,
/// whether given with its `/v1` or without.
fn url_of(connection: &Connection) -> String {
    let base = connection.base_url.as_deref().unwrap_or(TYPESAFE_BASE);
    if base.ends_with("/v1/systemone") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/systemone")
    } else {
        format!("{base}/v1/systemone")
    }
}

/// The request `decide` makes of `connection`.
///
/// # Errors
///
/// A connection holding no API key.
pub fn request(connection: &Connection, decide: &Decide<'_>) -> Result<Prepared, Refused> {
    let Credentials::ApiKey(key) = &connection.credentials else {
        return Err(Refused::Unreadable(
            "this connection holds no API key".into(),
        ));
    };
    let mut questions = Map::new();
    for question in decide.questions {
        let criteria = match &question.criteria {
            Criteria::Noul {
                yes_means,
                no_means,
            } if !yes_means.is_empty() && !no_means.is_empty() => {
                Some(json!({ "true": yes_means, "false": no_means }))
            }
            Criteria::Noul { .. } => None,
            Criteria::Score { levels } => Some(json!(levels)),
            Criteria::Choice { options } => Some(Value::Object(
                options
                    .iter()
                    .map(|o| {
                        let means = if o.means.is_empty() {
                            &o.name
                        } else {
                            &o.means
                        };
                        (o.name.clone(), json!(means))
                    })
                    .collect(),
            )),
        };
        let kind = match question.criteria {
            Criteria::Noul { .. } => "noul",
            Criteria::Score { .. } => "score",
            Criteria::Choice { .. } => "choice",
        };
        let mut asked = json!({ "type": kind, "instructions": question.instructions });
        if let Some(criteria) = criteria {
            super::set(&mut asked, "criteria", criteria);
        }
        questions.insert(question.name.clone(), asked);
    }
    let body = json!({
        "model": decide.model,
        "state": decide.state,
        "questions": questions,
    });
    let mut headers = vec![(
        "content-type".to_owned(),
        "application/json".to_owned().into(),
    )];
    if !key.expose().is_empty() {
        headers.push((
            "authorization".to_owned(),
            format!("Bearer {}", key.expose()).into(),
        ));
    }
    Ok(Prepared {
        url: url_of(connection),
        headers,
        body: serde_json::to_vec(&body).unwrap_or_default(),
    })
}

/// A probability, read: a finite number from 0 to 1.
fn probability(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
}

/// Each answer's probabilities by level or option, as finite numbers.
fn probabilities(answer: &Value) -> BTreeMap<String, f64> {
    answer
        .get("probabilities")
        .and_then(Value::as_object)
        .map(|all| {
            all.iter()
                .filter_map(|(k, v)| probability(Some(v)).map(|p| (k.clone(), p)))
                .collect()
        })
        .unwrap_or_default()
}

/// The decision in a 2xx's body, `asked` the model named where it names none.
///
/// # Errors
///
/// A body holding no answers, or an answer of no type it knows.
pub fn answer(body: &Value, asked: &str) -> Result<Decided, Refused> {
    let unreadable = |why: &str| Refused::Unreadable(why.to_owned());
    let given = body
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| unreadable("it gave no answers"))?;
    let mut answers = BTreeMap::new();
    for (name, answer) in given {
        let read = match answer.get("type").and_then(Value::as_str) {
            Some("noul") => Answer::Noul {
                noul: probability(answer.get("noul"))
                    .ok_or_else(|| unreadable("a yes-or-no answer holds no probability"))?,
            },
            Some("score") => Answer::Score {
                score: answer
                    .get("score")
                    .and_then(Value::as_f64)
                    .filter(|s| s.is_finite())
                    .ok_or_else(|| unreadable("a score's answer holds no number"))?,
                confidence: probability(answer.get("confidence")),
                probabilities: probabilities(answer),
            },
            Some("choice") => Answer::Choice {
                choice: answer
                    .get("choice")
                    .and_then(Value::as_str)
                    .ok_or_else(|| unreadable("a choice's answer names no option"))?
                    .to_owned(),
                confidence: probability(answer.get("confidence")),
                probabilities: probabilities(answer),
            },
            _ => return Err(unreadable("an answer of no type a question asks")),
        };
        answers.insert(name.clone(), read);
    }
    Ok(Decided {
        model: body
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .unwrap_or(asked)
            .to_owned(),
        answers,
        usage: Usage {
            input_tokens: tokens(body, "/usage/input_tokens"),
            output_tokens: tokens(body, "/usage/output_tokens"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelProvider;

    fn connection(base_url: Option<&str>, key: &str) -> Connection {
        Connection {
            provider: if base_url.is_some() {
                ModelProvider::SystemOneCompatible
            } else {
                ModelProvider::Typesafe
            },
            base_url: base_url.map(str::to_owned),
            region: None,
            credentials: Credentials::ApiKey(key.to_owned().into()),
        }
    }

    fn questions() -> Vec<Question> {
        vec![
            Question {
                name: "grounded".into(),
                instructions: "Does the answer rest on the context?".into(),
                criteria: Criteria::Noul {
                    yes_means: "every claim is in it".into(),
                    no_means: String::new(),
                },
            },
            Question {
                name: "complete".into(),
                instructions: "How far was the task done?".into(),
                criteria: Criteria::Score {
                    levels: vec!["not at all".into(), "partly".into(), "fully".into()],
                },
            },
            Question {
                name: "topic".into(),
                instructions: "What is it about?".into(),
                criteria: Criteria::Choice {
                    options: vec![
                        Choice {
                            name: "billing".into(),
                            means: String::new(),
                        },
                        Choice {
                            name: "other".into(),
                            means: "anything else".into(),
                        },
                    ],
                },
            },
        ]
    }

    /// The request is System One's: each question by its name and type, a
    /// yes-or-no's criteria only when both are described, an option with no
    /// description described by its name, the key a bearer.
    #[test]
    fn a_request_is_system_ones_shape() {
        let questions = questions();
        let decide = Decide {
            model: "jev-latest",
            state: "the run",
            questions: &questions,
        };
        let prepared = request(&connection(None, "ts-key"), &decide).expect("built");
        assert_eq!(prepared.url, "https://api.typesafe.ai/v1/systemone");
        let body: Value = serde_json::from_slice(&prepared.body).expect("json");
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], "the run");
        assert_eq!(body["questions"]["grounded"]["type"], "noul");
        assert!(body["questions"]["grounded"].get("criteria").is_none());
        assert_eq!(
            body["questions"]["complete"]["criteria"],
            json!(["not at all", "partly", "fully"])
        );
        assert_eq!(
            body["questions"]["topic"]["criteria"],
            json!({ "billing": "billing", "other": "anything else" })
        );
        assert!(
            prepared
                .headers
                .iter()
                .any(|(n, v)| n == "authorization" && v.expose() == "Bearer ts-key")
        );
    }

    /// A compatible endpoint is reached at its base, given with `/v1` or
    /// without, and one that asks for no key is sent none.
    #[test]
    fn a_compatible_endpoint_is_reached_at_its_base() {
        let questions = questions();
        let decide = Decide {
            model: "Cloudflare/clef",
            state: "s",
            questions: &questions,
        };
        for base in [
            "http://clef.internal:30000",
            "http://clef.internal:30000/v1",
            "http://clef.internal:30000/v1/systemone",
        ] {
            let prepared = request(&connection(Some(base), ""), &decide).expect("built");
            assert_eq!(prepared.url, "http://clef.internal:30000/v1/systemone");
            assert!(prepared.headers.iter().all(|(n, _)| n != "authorization"));
        }
    }

    /// Each answer is read by its type, the version that answered kept, and
    /// one of no known type, or a probability past one, is unreadable.
    #[test]
    fn answers_are_read_by_their_type() {
        let decided = answer(
            &json!({
                "model": "jev-1.14.0",
                "answers": {
                    "grounded": { "type": "noul", "noul": 0.82 },
                    "complete": { "type": "score", "score": 1.6, "confidence": 0.7,
                                  "legend": { "0": "not at all", "1": "partly", "2": "fully" },
                                  "probabilities": { "0": 0.1, "1": 0.2, "2": 0.7 } },
                    "topic": { "type": "choice", "choice": "billing", "confidence": 0.9,
                               "probabilities": { "billing": 0.9, "other": 0.1 } },
                },
                "usage": { "input_tokens": 812, "output_tokens": 0 },
            }),
            "jev-latest",
        )
        .expect("read");
        assert_eq!(decided.model, "jev-1.14.0");
        assert_eq!(decided.answers["grounded"], Answer::Noul { noul: 0.82 });
        assert!(matches!(
            &decided.answers["complete"],
            Answer::Score { score, confidence: Some(c), probabilities } if (*score - 1.6).abs() < 1e-9 && (*c - 0.7).abs() < 1e-9 && probabilities.len() == 3
        ));
        assert!(matches!(
            &decided.answers["topic"],
            Answer::Choice { choice, .. } if choice == "billing"
        ));
        assert_eq!(decided.usage.input_tokens, 812);
        assert_eq!(
            answer(&json!({ "answers": {} }), "m").expect("read").model,
            "m"
        );
        assert!(answer(&json!({ "answers": { "x": { "type": "box" } } }), "m").is_err());
        assert!(
            answer(
                &json!({ "answers": { "x": { "type": "noul", "noul": 1.5 } } }),
                "m"
            )
            .is_err()
        );
        assert!(answer(&json!({ "choices": [] }), "m").is_err());
    }
}
