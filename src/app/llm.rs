//! Shared LLM call helper for rig-core backends.
//!
//! Reads provider and model from environment variables:
//! - `ANTHROPIC_API_KEY` + `ANTHROPIC_MODEL` (default: `claude-sonnet-4-6`)
//! - `OPENAI_API_KEY` + `OPENAI_MODEL` (default: `gpt-5.4-mini`)

use rig::{
    client::{CompletionClient, ProviderClient},
    completion::Prompt,
};

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4-mini";

/// Call an LLM with a system preamble and user prompt.
///
/// Selects the provider from environment variables. Returns the
/// model's text response.
///
/// # Errors
///
/// Returns an error string if no API key is set or the call fails.
pub async fn call(preamble: &str, prompt: &str) -> Result<String, String> {
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        let model = std::env::var("ANTHROPIC_MODEL")
            .unwrap_or_else(|_| DEFAULT_ANTHROPIC_MODEL.to_string());
        let client = rig::providers::anthropic::Client::from_env();
        let agent = client.agent(&model).preamble(preamble).build();
        agent.prompt(prompt).await.map_err(|e| e.to_string())
    } else if std::env::var("OPENAI_API_KEY").is_ok() {
        let model = std::env::var("OPENAI_MODEL")
            .unwrap_or_else(|_| DEFAULT_OPENAI_MODEL.to_string());
        let client = rig::providers::openai::Client::from_env();
        let agent = client.agent(&model).preamble(preamble).build();
        agent.prompt(prompt).await.map_err(|e| e.to_string())
    } else {
        Err("No ANTHROPIC_API_KEY or OPENAI_API_KEY set. \
             Lobster requires an LLM API key."
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_requires_api_key() {
        if std::env::var("ANTHROPIC_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            let result = call("system", "hello").await;
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("API_KEY"));
        }
    }
}
