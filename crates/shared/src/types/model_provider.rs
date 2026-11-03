//! [`ModelProvider`] — closed vocabulary of the model vendors a project connects.

use serde::{Deserialize, Serialize};

/// Whose model a project's connection reaches, on the customer's own key.
/// `[PUBLIC-API]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ModelProvider {
    /// OpenAI's API, by an API key.
    Openai,
    /// Anthropic's Messages API, by an API key.
    Anthropic,
    /// Google AI Studio's Gemini API, by an API key.
    GoogleAiStudio,
    /// Google Cloud's Vertex AI, by a service account's JSON key.
    VertexAi,
    /// An Azure OpenAI resource, by its key, each model a deployment.
    AzureOpenai,
    /// Amazon Bedrock's Converse API, by an access key pair and a region.
    Bedrock,
    /// Any endpoint speaking OpenAI's Chat Completions at a base URL of the
    /// customer's: a gateway, another vendor, a model they serve.
    OpenaiCompatible,
    /// TypeSafe's Jev, by an API key: typed questions answered with their
    /// probabilities, never written text.
    Typesafe,
    /// Any endpoint speaking TypeSafe's System One at a base URL of the
    /// customer's: an open-weight model they serve, or a gateway.
    SystemOneCompatible,
}

crate::wire_enum!(ModelProvider {
    Openai => "openai",
    Anthropic => "anthropic",
    GoogleAiStudio => "google_ai_studio",
    VertexAi => "vertex_ai",
    AzureOpenai => "azure_openai",
    Bedrock => "bedrock",
    OpenaiCompatible => "openai_compatible",
    Typesafe => "typesafe",
    SystemOneCompatible => "system_one_compatible",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_the_wire_words() {
        crate::test_util::assert_wire_words::<ModelProvider>();
    }
}
