//! Chat Completions, as OpenAI, an Azure OpenAI resource and every
//! compatible endpoint take it, the answer asked for in a strict JSON schema.

use serde_json::{Value, json};

use super::{
    Ask, Connection, Credentials, Judged, Prepared, Refused, Role, SCHEMA_NAME, Usage, json_in,
    tokens,
};
use crate::ModelProvider;

/// OpenAI's own endpoint.
const OPENAI_BASE: &str = "https://api.openai.com/v1";

/// The field a request's token limit goes by: OpenAI's own API takes
/// `max_completion_tokens` alone, its reasoning models refusing the other;
/// every compatible endpoint `max_tokens`.
#[must_use]
pub const fn limit_field(official: bool) -> &'static str {
    if official {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

/// The Azure OpenAI API version structured outputs need, generally available.
pub const AZURE_API_VERSION: &str = "2024-10-21";

/// The request `ask` makes of `connection`.
///
/// # Errors
///
/// A connection with no endpoint where its provider needs one.
pub fn request(connection: &Connection, ask: &Ask<'_>) -> Result<Prepared, Refused> {
    let Credentials::ApiKey(key) = &connection.credentials else {
        return Err(Refused::Unreadable(
            "this connection holds no API key".into(),
        ));
    };
    let with_media = super::last_user(ask.messages).filter(|_| !ask.media.is_empty());
    let mut messages = vec![json!({ "role": "system", "content": ask.system })];
    messages.extend(ask.messages.iter().enumerate().map(|(at, (role, text))| {
        let role = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        if with_media != Some(at) {
            return json!({ "role": role, "content": text });
        }
        let mut content = vec![json!({ "type": "text", "text": text })];
        content.extend(ask.media.iter().map(part));
        json!({ "role": role, "content": content })
    }));
    let mut body = json!({
        "model": ask.model,
        "messages": messages,
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": SCHEMA_NAME, "schema": ask.schema, "strict": true },
        },
    });
    let limit = limit_field(connection.provider == ModelProvider::Openai);
    super::set(&mut body, limit, json!(ask.max_tokens));
    if let Some(temperature) = ask.temperature {
        super::set(&mut body, "temperature", json!(temperature));
    }
    let (url, headers) = match connection.provider {
        ModelProvider::AzureOpenai => {
            let base = connection.base_url.as_deref().ok_or_else(|| {
                Refused::Unreadable(
                    "an Azure OpenAI connection names its resource's endpoint".into(),
                )
            })?;
            (
                format!(
                    "{base}/openai/deployments/{}/chat/completions?api-version={AZURE_API_VERSION}",
                    super::sigv4::encode_segment(ask.model)
                ),
                vec![("api-key".to_owned(), key.clone())],
            )
        }
        _ => {
            let base = connection.base_url.as_deref().unwrap_or(OPENAI_BASE);
            let headers = if key.expose().is_empty() {
                Vec::new()
            } else {
                vec![(
                    "authorization".to_owned(),
                    format!("Bearer {}", key.expose()).into(),
                )]
            };
            (format!("{base}/chat/completions"), headers)
        }
    };
    let mut headers = headers;
    headers.push((
        "content-type".to_owned(),
        "application/json".to_owned().into(),
    ));
    Ok(Prepared {
        url,
        headers,
        body: serde_json::to_vec(&body).unwrap_or_default(),
    })
}

/// A file as Chat Completions carries it: an image by its data URL, a PDF as
/// a file.
fn part(media: &super::Media) -> Value {
    let data = media.base64();
    match media.mime_type.as_str() {
        "application/pdf" => json!({
            "type": "file",
            "file": { "filename": "document.pdf", "file_data": format!("data:application/pdf;base64,{data}") },
        }),
        mime => {
            json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } })
        }
    }
}

/// The judgement in a 2xx's body: the first choice's message, as JSON.
///
/// # Errors
///
/// A body with no message, a refusal, or one that is not JSON.
pub fn answer(body: &Value) -> Result<Judged, Refused> {
    let message = body
        .pointer("/choices/0/message")
        .ok_or_else(|| Refused::Unreadable("it answered no choice".into()))?;
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str) {
        return Err(Refused::Unreadable(format!(
            "the model declined: {}",
            refusal.chars().take(200).collect::<String>()
        )));
    }
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| Refused::Unreadable("its message holds no text".into()))?;
    Ok(Judged {
        answer: json_in(text)?,
        usage: Usage {
            input_tokens: tokens(body, "/usage/prompt_tokens"),
            output_tokens: tokens(body, "/usage/completion_tokens"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(provider: ModelProvider, base: Option<&str>) -> Connection {
        Connection {
            provider,
            base_url: base.map(str::to_owned),
            region: None,
            credentials: Credentials::ApiKey("sk-test".to_owned().into()),
            sign_in: None,
        }
    }

    fn ask(schema: &Value) -> Ask<'_> {
        Ask {
            model: "gpt-x",
            system: "judge",
            messages: &[],
            schema,
            temperature: Some(0.0),
            max_tokens: 64,
            media: &[],
        }
    }

    /// OpenAI's request asks for the schema strictly, its limit by its own
    /// name, the key as a bearer; Azure's names the deployment and its key.
    #[test]
    fn a_request_asks_for_the_schema_where_its_provider_answers() {
        let schema = json!({ "type": "object" });
        let openai =
            request(&connection(ModelProvider::Openai, None), &ask(&schema)).expect("built");
        assert_eq!(openai.url, "https://api.openai.com/v1/chat/completions");
        let body: Value = serde_json::from_slice(&openai.body).expect("json");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["max_completion_tokens"], 64);
        assert_eq!(
            body["messages"][0],
            json!({ "role": "system", "content": "judge" })
        );
        assert!(
            openai
                .headers
                .iter()
                .any(|(n, v)| n == "authorization" && v.expose() == "Bearer sk-test")
        );

        let azure = request(
            &connection(
                ModelProvider::AzureOpenai,
                Some("https://r.openai.azure.com"),
            ),
            &ask(&schema),
        )
        .expect("built");
        assert_eq!(
            azure.url,
            "https://r.openai.azure.com/openai/deployments/gpt-x/chat/completions?api-version=2024-10-21"
        );
        assert!(azure.headers.iter().any(|(n, _)| n == "api-key"));
        assert!(request(&connection(ModelProvider::AzureOpenai, None), &ask(&schema)).is_err());

        let gateway = request(
            &connection(
                ModelProvider::OpenaiCompatible,
                Some("https://gw.example/v1"),
            ),
            &ask(&schema),
        )
        .expect("built");
        assert_eq!(gateway.url, "https://gw.example/v1/chat/completions");
        let body: Value = serde_json::from_slice(&gateway.body).expect("json");
        assert_eq!(body["max_tokens"], 64);
    }

    /// The answer is the first choice's text as JSON, a refusal none.
    #[test]
    fn an_answer_is_the_first_choices_json() {
        let judged = answer(&json!({
            "choices": [{ "message": { "content": "{\"value\": 1}" } }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 3 },
        }))
        .expect("read");
        assert_eq!(judged.answer, json!({ "value": 1 }));
        assert_eq!(
            judged.usage,
            Usage {
                input_tokens: 12,
                output_tokens: 3
            }
        );
        assert!(answer(&json!({ "choices": [{ "message": { "refusal": "no" } }] })).is_err());
        assert!(answer(&json!({})).is_err());
    }
}
