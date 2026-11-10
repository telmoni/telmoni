//! Fault classes from codes and exception classes, never their messages.

use opentelemetry_proto::tonic::common::v1::any_value::Value;
use telmoni_shared::{SpanFault, SpanKind, SpanStatus};

/// A failed span's code or class; an unknown failure is still counted.
#[must_use]
pub fn error(given: &str) -> SpanFault {
    use SpanFault as F;
    // Python and Go qualify exception classes with their package.
    let name = given.rsplit('.').next().unwrap_or(given);
    match name {
        "429"
        | "rate_limit"
        | "rate_limit_error"
        | "rate_limit_exceeded"
        | "RateLimitError"
        | "ResourceExhausted"
        | "RESOURCE_EXHAUSTED"
        | "ThrottlingException"
        | "TooManyRequests"
        | "TooManyRequestsException" => F::RateLimited,
        "503"
        | "529"
        | "overloaded_error"
        | "OverloadedError"
        | "ServiceUnavailable"
        | "ServiceUnavailableError"
        | "ServiceUnavailableException" => F::Overloaded,
        "408"
        | "504"
        | "timeout"
        | "request_timeout"
        | "TimeoutError"
        | "APITimeoutError"
        | "RequestTimeoutError"
        | "ReadTimeout"
        | "ConnectTimeout"
        | "DeadlineExceeded"
        | "DEADLINE_EXCEEDED"
        | "ModelTimeoutException" => F::Timeout,
        "context_length_exceeded" | "ContextWindowExceededError" => F::ContextLength,
        "401"
        | "403"
        | "authentication_error"
        | "permission_error"
        | "invalid_api_key"
        | "AuthenticationError"
        | "PermissionDeniedError"
        | "Unauthenticated"
        | "UNAUTHENTICATED"
        | "PermissionDenied"
        | "PERMISSION_DENIED"
        | "AccessDeniedException" => F::Unauthorized,
        "APIResponseValidationError"
        | "JSONParseError"
        | "AI_JSONParseError"
        | "JSONSchemaValidationError"
        | "OutputParserException"
        | "ModelBehaviorError"
        | "AI_NoObjectGeneratedError"
        | "AI_TypeValidationError" => F::InvalidOutput,
        "content_policy_violation"
        | "ContentPolicyViolationError"
        | "ContentFilterFinishReasonError" => F::Refused,
        "LengthFinishReasonError" => F::Truncated,
        _ => F::Other,
    }
}

/// A model's finish reasons, refusal taking precedence across choices.
#[must_use]
pub fn finish(value: &Value) -> Option<SpanFault> {
    fn one(value: &Value) -> Option<SpanFault> {
        let Value::StringValue(reason) = value else {
            return None;
        };
        match reason.as_str() {
            "content_filter" | "refusal" | "refused" | "SAFETY" | "RECITATION" | "BLOCKLIST"
            | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => Some(SpanFault::Refused),
            "length" | "max_tokens" | "MAX_TOKENS" => Some(SpanFault::Truncated),
            _ => None,
        }
    }
    match value {
        Value::ArrayValue(array) => array
            .values
            .iter()
            .filter_map(|v| v.value.as_ref().and_then(one))
            .reduce(|left, right| {
                if right == SpanFault::Refused {
                    right
                } else {
                    left
                }
            }),
        _ => one(value),
    }
}

/// Classification never changes the sender's status.
#[must_use]
pub fn of(
    status: SpanStatus,
    kind: SpanKind,
    error_type: &str,
    finish: Option<SpanFault>,
) -> Option<SpanFault> {
    match status {
        SpanStatus::Cancelled => None,
        SpanStatus::Error => Some(error(error_type)),
        SpanStatus::Ok if kind == SpanKind::ModelCall => finish,
        SpanStatus::Ok => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_error_spellings_have_recorded_sources() {
        #[derive(serde::Deserialize)]
        struct Case {
            source: String,
            error_type: String,
            fault: SpanFault,
        }
        #[derive(serde::Deserialize)]
        struct Fixture {
            sources: std::collections::HashMap<String, String>,
            errors: Vec<Case>,
        }
        let fixture: Fixture =
            serde_json::from_str(include_str!("../tests/fixtures/faults.json")).expect("fixture");
        for case in fixture.errors {
            assert!(fixture.sources.contains_key(&case.source));
            assert_eq!(error(&case.error_type), case.fault, "{}", case.error_type);
        }
    }

    #[test]
    fn finish_reasons_are_bounded_classes_and_refusal_wins() {
        use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue};
        let reasons = |values: &[&str]| {
            Value::ArrayValue(ArrayValue {
                values: values
                    .iter()
                    .map(|value| AnyValue {
                        value: Some(Value::StringValue((*value).into())),
                    })
                    .collect(),
            })
        };
        for values in [["length", "content_filter"], ["SAFETY", "MAX_TOKENS"]] {
            assert_eq!(finish(&reasons(&values)), Some(SpanFault::Refused));
        }
        assert_eq!(finish(&reasons(&["stop"])), None);
        assert_eq!(
            finish(&reasons(&["max_tokens"])),
            Some(SpanFault::Truncated)
        );
        assert_eq!(finish(&Value::IntValue(429)), None);
        assert_eq!(
            of(
                SpanStatus::Error,
                SpanKind::ModelCall,
                "429",
                Some(SpanFault::Refused)
            ),
            Some(SpanFault::RateLimited)
        );
        assert_eq!(
            of(
                SpanStatus::Ok,
                SpanKind::ToolCall,
                "",
                Some(SpanFault::Truncated)
            ),
            None
        );
    }

    #[test]
    fn messages_and_substrings_are_not_codes() {
        for value in [
            "",
            "NotRateLimitError",
            "RateLimitError: private message",
            "some_limit",
            "BadRequestError",
        ] {
            assert_eq!(error(value), SpanFault::Other);
        }
        assert_eq!(error("openai.RateLimitError"), SpanFault::RateLimited);
        assert_eq!(of(SpanStatus::Ok, SpanKind::ModelCall, "429", None), None);
        assert_eq!(
            of(
                SpanStatus::Cancelled,
                SpanKind::ModelCall,
                "429",
                Some(SpanFault::Refused)
            ),
            None
        );
    }
}
