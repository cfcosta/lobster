//! Shared LLM call helper for rig-core backends.
//!
//! Reads provider and model from environment variables:
//! - `ANTHROPIC_API_KEY` + `ANTHROPIC_MODEL` (default: `claude-sonnet-4-6`)
//! - `OPENAI_API_KEY` + `OPENAI_MODEL` (default: `gpt-5.4-mini`)

use rig::client::{CompletionClient, ProviderClient};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4-mini";

const NO_API_KEY_MSG: &str = "No ANTHROPIC_API_KEY or OPENAI_API_KEY set. \
                               Lobster requires an LLM API key.";

/// Extract structured data from a prompt using rig's tool-call
/// based extractor. The model is forced to return a JSON object
/// matching `T`'s schema.
///
/// # Errors
///
/// Returns an error string if no API key is set or the call fails.
pub async fn extract<T>(preamble: &str, prompt: &str) -> Result<T, String>
where
    T: JsonSchema + for<'a> Deserialize<'a> + Serialize + Send + Sync + 'static,
{
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        let model = std::env::var("ANTHROPIC_MODEL")
            .unwrap_or_else(|_| DEFAULT_ANTHROPIC_MODEL.to_string());
        let client = rig::providers::anthropic::Client::from_env();
        let extractor =
            client.extractor::<T>(&model).preamble(preamble).build();
        extractor.extract(prompt).await.map_err(|e| e.to_string())
    } else if std::env::var("OPENAI_API_KEY").is_ok() {
        let model = std::env::var("OPENAI_MODEL")
            .unwrap_or_else(|_| DEFAULT_OPENAI_MODEL.to_string());
        let client = rig::providers::openai::Client::from_env();
        let extractor =
            client.extractor::<T>(&model).preamble(preamble).build();
        extractor.extract(prompt).await.map_err(|e| e.to_string())
    } else {
        Err(NO_API_KEY_MSG.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Serialize, JsonSchema)]
    struct Dummy {
        value: String,
    }

    #[tokio::test]
    async fn test_requires_api_key() {
        if std::env::var("ANTHROPIC_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            let result = extract::<Dummy>("system", "hello").await;
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("API_KEY"));
        }
    }
}
