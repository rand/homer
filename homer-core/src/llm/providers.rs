// LLM provider implementations: Anthropic, OpenAI, and custom HTTP endpoints.
#![allow(clippy::cast_precision_loss)]

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{HomerError, LlmError};

use super::{LlmProvider, TokenUsage};

// ── Anthropic Provider ──────────────────────────────────────────────

#[derive(Debug)]
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    temperature: f64,
    messages: Vec<AnthropicMessage>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: String,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[async_trait::async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    async fn call(
        &self,
        prompt: &str,
        temperature: f64,
    ) -> crate::error::Result<(String, TokenUsage)> {
        let url = format!("{}/v1/messages", self.base_url);

        let body = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: 1024,
            temperature,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
        };

        debug!(model = %self.model, "Calling Anthropic API");

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| HomerError::Llm(LlmError::Network(e.to_string())))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(HomerError::Llm(LlmError::ApiError { status, body: text }));
        }

        let result: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| HomerError::Llm(LlmError::Parse(e.to_string())))?;

        let text = result
            .content
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();

        Ok((
            text,
            TokenUsage {
                input_tokens: result.usage.input_tokens,
                output_tokens: result.usage.output_tokens,
            },
        ))
    }

    fn cost_per_1k_input(&self) -> f64 {
        // Claude Sonnet 4 pricing (approximate)
        if self.model.contains("opus") {
            0.015
        } else if self.model.contains("sonnet") {
            0.003
        } else if self.model.contains("haiku") {
            0.00025
        } else {
            0.003
        }
    }

    fn cost_per_1k_output(&self) -> f64 {
        if self.model.contains("opus") {
            0.075
        } else if self.model.contains("sonnet") {
            0.015
        } else if self.model.contains("haiku") {
            0.00125
        } else {
            0.015
        }
    }
}

// ── OpenAI Provider ─────────────────────────────────────────────────

#[derive(Debug)]
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
            base_url: "https://api.openai.com".to_string(),
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    max_tokens: u32,
    temperature: f64,
    messages: Vec<OpenAiMessage>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: OpenAiUsage,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiChoiceMessage,
}

#[derive(Deserialize)]
struct OpenAiChoiceMessage {
    content: String,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[async_trait::async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    async fn call(
        &self,
        prompt: &str,
        temperature: f64,
    ) -> crate::error::Result<(String, TokenUsage)> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let body = OpenAiRequest {
            model: self.model.clone(),
            max_tokens: 1024,
            temperature,
            messages: vec![OpenAiMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
        };

        debug!(model = %self.model, "Calling OpenAI API");

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| HomerError::Llm(LlmError::Network(e.to_string())))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(HomerError::Llm(LlmError::ApiError { status, body: text }));
        }

        let result: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| HomerError::Llm(LlmError::Parse(e.to_string())))?;

        let text = result
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok((
            text,
            TokenUsage {
                input_tokens: result.usage.prompt_tokens,
                output_tokens: result.usage.completion_tokens,
            },
        ))
    }

    fn cost_per_1k_input(&self) -> f64 {
        if self.model.contains("gpt-4o") {
            0.0025
        } else if self.model.contains("gpt-4") {
            0.03
        } else {
            0.0015
        }
    }

    fn cost_per_1k_output(&self) -> f64 {
        if self.model.contains("gpt-4o") {
            0.01
        } else if self.model.contains("gpt-4") {
            0.06
        } else {
            0.002
        }
    }
}

// ── Provider Factory ────────────────────────────────────────────────

/// Create an LLM provider from configuration.
pub fn create_provider(
    provider: &str,
    model: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> crate::error::Result<Box<dyn LlmProvider>> {
    match provider {
        "anthropic" => {
            let mut p = AnthropicProvider::new(api_key.to_string(), model.to_string());
            if let Some(url) = base_url {
                p = p.with_base_url(url.to_string());
            }
            Ok(Box::new(p))
        }
        "openai" | "custom" => {
            let mut p = OpenAiProvider::new(api_key.to_string(), model.to_string());
            if let Some(url) = base_url {
                p = p.with_base_url(url.to_string());
            }
            Ok(Box::new(p))
        }
        other => Err(HomerError::Llm(LlmError::Config(format!(
            "Unknown provider: {other}. Use: anthropic, openai, custom"
        )))),
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_cost_tiers() {
        let opus = AnthropicProvider::new("key".into(), "claude-opus-4-20250514".into());
        assert!(opus.cost_per_1k_input() > 0.01);

        let sonnet = AnthropicProvider::new("key".into(), "claude-sonnet-4-20250514".into());
        assert!((sonnet.cost_per_1k_input() - 0.003).abs() < 0.001);

        let haiku = AnthropicProvider::new("key".into(), "claude-haiku-4-20250514".into());
        assert!(haiku.cost_per_1k_input() < 0.001);
    }

    #[test]
    fn openai_cost_tiers() {
        let gpt4o = OpenAiProvider::new("key".into(), "gpt-4o".into());
        assert!(gpt4o.cost_per_1k_input() < 0.01);

        let gpt4 = OpenAiProvider::new("key".into(), "gpt-4-turbo".into());
        assert!(gpt4.cost_per_1k_input() > 0.01);
    }

    #[test]
    fn create_provider_factory() {
        let p = create_provider("anthropic", "test-model", "key", None).unwrap();
        assert_eq!(p.name(), "anthropic");
        assert_eq!(p.model_id(), "test-model");

        let p = create_provider("openai", "gpt-4o", "key", None).unwrap();
        assert_eq!(p.name(), "openai");

        let p = create_provider(
            "custom",
            "local-model",
            "key",
            Some("http://localhost:8080"),
        );
        assert!(p.is_ok());

        let p = create_provider("invalid", "model", "key", None);
        assert!(p.is_err());
    }
}
