//! Amazon Bedrock's Converse API, signed with SigV4, the answer asked for as
//! the one tool the model must use, whose input is the schema.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::sigv4::{Signer, authorization, encode_segment};
use super::{
    Ask, Connection, Credentials, Judged, Prepared, Refused, Role, SCHEMA_NAME, Usage, tokens,
};

/// The request `ask` makes of `connection`, signed at `at`.
///
/// # Errors
///
/// A connection with no access key pair or no region.
pub fn request(
    connection: &Connection,
    ask: &Ask<'_>,
    at: DateTime<Utc>,
) -> Result<Prepared, Refused> {
    let Credentials::AccessKeys {
        access_key_id,
        secret_access_key,
    } = &connection.credentials
    else {
        return Err(Refused::Unreadable(
            "a Bedrock connection holds an access key pair".into(),
        ));
    };
    let region = connection
        .region
        .as_ref()
        .map(super::Region::as_str)
        .ok_or_else(|| Refused::Unreadable("a Bedrock connection names its region".into()))?;
    let with_media = super::last_user(ask.messages).filter(|_| !ask.media.is_empty());
    let messages: Vec<Value> = ask
        .messages
        .iter()
        .enumerate()
        .map(|(at, (role, text))| {
            let mut content = vec![json!({ "text": text })];
            if with_media == Some(at) {
                content.extend(ask.media.iter().enumerate().map(|(n, m)| block(n, m)));
            }
            json!({ "role": match role { Role::User => "user", Role::Assistant => "assistant" }, "content": content })
        })
        .collect();
    let mut inference = json!({ "maxTokens": ask.max_tokens });
    if let Some(temperature) = ask.temperature {
        super::set(&mut inference, "temperature", json!(temperature));
    }
    let body = serde_json::to_vec(&json!({
        "system": [{ "text": ask.system }],
        "messages": messages,
        "inferenceConfig": inference,
        "toolConfig": {
            "tools": [{ "toolSpec": { "name": SCHEMA_NAME, "description": "Give the score.", "inputSchema": { "json": ask.schema } } }],
            "toolChoice": { "tool": { "name": SCHEMA_NAME } },
        },
    }))
    .unwrap_or_default();
    let host = format!("bedrock-runtime.{region}.amazonaws.com");
    let path = format!("/model/{}/converse", encode_segment(ask.model));
    let stamp = at.format("%Y%m%dT%H%M%SZ").to_string();
    let signed = authorization(
        &Signer {
            access_key_id,
            secret_access_key: secret_access_key.expose(),
            region,
            service: "bedrock",
            at,
        },
        "POST",
        &path,
        "",
        &[
            ("content-type", "application/json"),
            ("host", &host),
            ("x-amz-date", &stamp),
        ],
        &body,
    );
    Ok(Prepared {
        url: format!("https://{host}{path}"),
        headers: vec![
            (
                "content-type".to_owned(),
                "application/json".to_owned().into(),
            ),
            ("x-amz-date".to_owned(), stamp.into()),
            ("authorization".to_owned(), signed.into()),
        ],
        body,
    })
}

/// A file as Converse carries it: an image by its format, or a PDF as a
/// document, each document named once in the request as Converse asks.
fn block(n: usize, media: &super::Media) -> Value {
    let bytes = media.base64();
    match media.mime_type.as_str() {
        "application/pdf" => json!({
            "document": { "format": "pdf", "name": format!("document-{}", n + 1), "source": { "bytes": bytes } },
        }),
        mime => json!({
            "image": { "format": mime.trim_start_matches("image/"), "source": { "bytes": bytes } },
        }),
    }
}
/// The judgement in a 2xx's body: the input of the tool it used.
///
/// # Errors
///
/// A body whose model used no tool.
pub fn answer(body: &Value) -> Result<Judged, Refused> {
    let input = body
        .pointer("/output/message/content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find_map(|b| b.get("toolUse").filter(|t| t["name"] == SCHEMA_NAME))
        })
        .and_then(|tool| tool.get("input"))
        .cloned()
        .ok_or_else(|| Refused::Unreadable("the model gave no score".into()))?;
    Ok(Judged {
        answer: input,
        usage: Usage {
            input_tokens: tokens(body, "/usage/inputTokens"),
            output_tokens: tokens(body, "/usage/outputTokens"),
        },
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::ModelProvider;

    /// The request goes to the region's runtime, its model's colon encoded,
    /// signed for Bedrock; the answer is the used tool's input.
    #[test]
    fn a_request_is_signed_for_its_region_and_the_answer_is_the_tools_input() {
        let schema = json!({ "type": "object" });
        let connection = Connection {
            provider: ModelProvider::Bedrock,
            base_url: None,
            region: Some(crate::model::Region::try_new("us-east-1").unwrap()),
            credentials: Credentials::AccessKeys {
                access_key_id: "AKID".into(),
                secret_access_key: "secret".to_owned().into(),
            },
        };
        let ask = Ask {
            model: "anthropic.claude-x:0",
            system: "s",
            messages: &[],
            schema: &schema,
            temperature: None,
            max_tokens: 10,
            media: &[],
        };
        let at = Utc
            .with_ymd_and_hms(2026, 11, 2, 12, 0, 0)
            .single()
            .expect("a time");
        let prepared = request(&connection, &ask, at).expect("built");
        assert_eq!(
            prepared.url,
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-x%3A0/converse"
        );
        let auth = prepared
            .headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .expect("signed");
        assert!(auth.1.expose().starts_with(
            "AWS4-HMAC-SHA256 Credential=AKID/20261102/us-east-1/bedrock/aws4_request"
        ));
        let body: Value = serde_json::from_slice(&prepared.body).expect("json");
        assert_eq!(
            body["toolConfig"]["toolChoice"],
            json!({ "tool": { "name": "score" } })
        );

        let judged = answer(&json!({
            "output": { "message": { "content": [{ "toolUse": { "name": "score", "input": { "value": 3 } } }] } },
            "usage": { "inputTokens": 5, "outputTokens": 2 },
        }))
        .expect("read");
        assert_eq!(judged.answer, json!({ "value": 3 }));
        let no_region = Connection {
            region: None,
            ..connection
        };
        assert!(request(&no_region, &ask, at).is_err());
    }
}
