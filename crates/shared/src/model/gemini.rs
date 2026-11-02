//! Gemini's `generateContent`, as Google AI Studio takes it by a key and
//! Vertex AI by a service account's token, the answer asked for as JSON in
//! the schema.

use serde_json::{Value, json};

use super::{
    Ask, Connection, Credentials, Judged, Prepared, Refused, Role, Usage, json_in,
    sigv4::encode_segment, tokens,
};
use crate::Redacted;

const AI_STUDIO_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// The request `ask` makes of `connection`: AI Studio's with its key, or
/// Vertex AI's with `token`, at its project and location.
///
/// # Errors
///
/// A connection with neither a key nor a token to send.
pub fn request(
    connection: &Connection,
    ask: &Ask<'_>,
    token: Option<&Redacted>,
) -> Result<Prepared, Refused> {
    let contents: Vec<Value> = ask
        .messages
        .iter()
        .map(|(role, text)| json!({ "role": match role { Role::User => "user", Role::Assistant => "model" }, "parts": [{ "text": text }] }))
        .collect();
    let mut config = json!({
        "maxOutputTokens": ask.max_tokens,
        "responseMimeType": "application/json",
        "responseJsonSchema": ask.schema,
    });
    if let Some(temperature) = ask.temperature {
        super::set(&mut config, "temperature", json!(temperature));
    }
    let body = json!({
        "systemInstruction": { "parts": [{ "text": ask.system }] },
        "contents": contents,
        "generationConfig": config,
    });
    let model = encode_segment(ask.model);
    let (url, auth) = match (&connection.credentials, token) {
        (Credentials::ServiceAccount(account), Some(token)) => {
            let location = connection.region.as_deref().unwrap_or("global");
            let host = if location == "global" {
                "aiplatform.googleapis.com".to_owned()
            } else {
                format!("{location}-aiplatform.googleapis.com")
            };
            (
                format!(
                    "https://{host}/v1/projects/{}/locations/{location}/publishers/google/models/{model}:generateContent",
                    encode_segment(&account.project_id)
                ),
                (
                    "authorization".to_owned(),
                    format!("Bearer {}", token.expose()).into(),
                ),
            )
        }
        (Credentials::ApiKey(key), None) => (
            format!("{AI_STUDIO_BASE}/{model}:generateContent"),
            ("x-goog-api-key".to_owned(), key.clone()),
        ),
        _ => {
            return Err(Refused::Unreadable(
                "this connection holds no key for Gemini".into(),
            ));
        }
    };
    Ok(Prepared {
        url,
        headers: vec![
            auth,
            (
                "content-type".to_owned(),
                "application/json".to_owned().into(),
            ),
        ],
        body: serde_json::to_vec(&body).unwrap_or_default(),
    })
}

/// The judgement in a 2xx's body: the first candidate's text, as JSON.
///
/// # Errors
///
/// A body with no candidate, one blocked, or text that is not JSON.
pub fn answer(body: &Value) -> Result<Judged, Refused> {
    if let Some(reason) = body
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
    {
        return Err(Refused::Unreadable(format!(
            "the prompt was blocked: {reason}"
        )));
    }
    let text: String = body
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| Refused::Unreadable("it answered no candidate".into()))?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    Ok(Judged {
        answer: json_in(&text)?,
        usage: Usage {
            input_tokens: tokens(body, "/usageMetadata/promptTokenCount"),
            output_tokens: tokens(body, "/usageMetadata/candidatesTokenCount"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelProvider;

    /// AI Studio's request carries its key and the schema; the answer is its text as JSON.
    #[test]
    fn ai_studio_asks_for_json_in_the_schema() {
        let schema = json!({ "type": "object" });
        let connection = Connection {
            provider: ModelProvider::GoogleAiStudio,
            base_url: None,
            region: None,
            credentials: Credentials::ApiKey("AIza".to_owned().into()),
        };
        let messages = [(Role::Assistant, "an answer".to_owned())];
        let ask = Ask {
            model: "gemini-x",
            system: "s",
            messages: &messages,
            schema: &schema,
            temperature: Some(0.2),
            max_tokens: 50,
        };
        let prepared = request(&connection, &ask, None).expect("built");
        assert_eq!(
            prepared.url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-x:generateContent"
        );
        let body: Value = serde_json::from_slice(&prepared.body).expect("json");
        assert_eq!(body["generationConfig"]["responseJsonSchema"], schema);
        assert_eq!(body["contents"][0]["role"], "model");
        assert!(prepared.headers.iter().any(|(n, _)| n == "x-goog-api-key"));

        let judged = answer(&json!({
            "candidates": [{ "content": { "parts": [{ "text": "{\"value\":" }, { "text": " 2}" }] } }],
            "usageMetadata": { "promptTokenCount": 8, "candidatesTokenCount": 4 },
        }))
        .expect("read");
        assert_eq!(judged.answer, json!({ "value": 2 }));
        assert!(answer(&json!({ "promptFeedback": { "blockReason": "SAFETY" } })).is_err());
    }
}
