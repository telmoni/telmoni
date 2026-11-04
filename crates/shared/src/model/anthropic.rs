//! Anthropic's Messages API, the answer asked for as the one tool the model
//! must call, whose input is the schema.

use serde_json::{Value, json};

use super::{
    Ask, Connection, Credentials, Judged, Prepared, Refused, Role, SCHEMA_NAME, Usage, tokens,
};

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// The API version every request names, the agent's and the evaluators'.
pub const VERSION: &str = "2023-06-01";

/// The request `ask` makes of `connection`.
///
/// # Errors
///
/// A connection holding no API key.
pub fn request(connection: &Connection, ask: &Ask<'_>) -> Result<Prepared, Refused> {
    let Credentials::ApiKey(key) = &connection.credentials else {
        return Err(Refused::Unreadable(
            "this connection holds no API key".into(),
        ));
    };
    let with_media = super::last_user(ask.messages).filter(|_| !ask.media.is_empty());
    let messages: Vec<Value> = ask
        .messages
        .iter()
        .enumerate()
        .map(|(at, (role, text))| {
            let role = match role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            if with_media != Some(at) {
                return json!({ "role": role, "content": text });
            }
            let mut content: Vec<Value> = ask.media.iter().map(block).collect();
            content.push(json!({ "type": "text", "text": text }));
            json!({ "role": role, "content": content })
        })
        .collect();
    let mut body = json!({
        "model": ask.model,
        "max_tokens": ask.max_tokens,
        "system": ask.system,
        "messages": messages,
        "tools": [{ "name": SCHEMA_NAME, "description": "Give the score.", "input_schema": ask.schema }],
        "tool_choice": { "type": "tool", "name": SCHEMA_NAME },
    });
    if let Some(temperature) = ask.temperature {
        super::set(&mut body, "temperature", json!(temperature));
    }
    Ok(Prepared {
        url: MESSAGES_URL.to_owned(),
        headers: vec![
            ("x-api-key".to_owned(), key.clone()),
            ("anthropic-version".to_owned(), VERSION.to_owned().into()),
            (
                "content-type".to_owned(),
                "application/json".to_owned().into(),
            ),
        ],
        body: serde_json::to_vec(&body).unwrap_or_default(),
    })
}

/// A file as Messages carries it, before the text as Anthropic advises: an
/// image, or a PDF as a document.
fn block(media: &super::Media) -> Value {
    let kind = if media.mime_type == "application/pdf" {
        "document"
    } else {
        "image"
    };
    json!({
        "type": kind,
        "source": { "type": "base64", "media_type": media.mime_type, "data": media.base64() },
    })
}
/// The judgement in a 2xx's body: the input of the tool it called.
///
/// # Errors
///
/// A body whose model called no tool.
pub fn answer(body: &Value) -> Result<Judged, Refused> {
    let input = body
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b["type"] == "tool_use" && b["name"] == SCHEMA_NAME)
        })
        .and_then(|block| block.get("input"))
        .cloned()
        .ok_or_else(|| Refused::Unreadable("the model gave no score".into()))?;
    Ok(Judged {
        answer: input,
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

    /// The request makes the score a tool the model must call, the key its own header.
    #[test]
    fn a_request_makes_the_score_a_tool_the_model_must_call() {
        let schema = json!({ "type": "object" });
        let connection = Connection {
            provider: ModelProvider::Anthropic,
            base_url: None,
            region: None,
            credentials: Credentials::ApiKey("sk-ant".to_owned().into()),
            sign_in: None,
        };
        let messages = [(Role::User, "judge this".to_owned())];
        let ask = Ask {
            model: "claude-x",
            system: "s",
            messages: &messages,
            schema: &schema,
            temperature: None,
            max_tokens: 100,
            media: &[],
        };
        let prepared = request(&connection, &ask).expect("built");
        let body: Value = serde_json::from_slice(&prepared.body).expect("json");
        assert_eq!(
            body["tool_choice"],
            json!({ "type": "tool", "name": "score" })
        );
        assert_eq!(body["tools"][0]["input_schema"], schema);
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("temperature").is_none());
        assert!(
            prepared
                .headers
                .iter()
                .any(|(n, v)| n == "x-api-key" && v.expose() == "sk-ant")
        );
    }

    /// The answer is the called tool's input.
    #[test]
    fn an_answer_is_the_tools_input() {
        let judged = answer(&json!({
            "content": [{ "type": "text", "text": "thinking" }, { "type": "tool_use", "name": "score", "input": { "value": true } }],
            "usage": { "input_tokens": 40, "output_tokens": 9 },
        }))
        .expect("read");
        assert_eq!(judged.answer, json!({ "value": true }));
        assert_eq!(judged.usage.input_tokens, 40);
        assert!(answer(&json!({ "content": [{ "type": "text", "text": "no" }] })).is_err());
    }
}
