//! DeepSeek client (OpenAI-compatible). Blocking reqwest — the batch enricher is a
//! CLI job and any live use runs on a worker thread, so no async runtime is needed.

pub mod enrich;

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Serialize;

pub struct DeepSeek {
    client: reqwest::blocking::Client,
    base_url: String,
    api_key: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Msg<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Serialize)]
struct Msg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

/// Token usage from one call (for cost reporting).
#[derive(Debug, Default, Clone, Copy)]
pub struct Usage {
    pub prompt: u64,
    pub cached: u64,
    pub completion: u64,
}

impl DeepSeek {
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("build reqwest client");
        Self {
            client,
            base_url,
            api_key,
        }
    }

    /// A JSON-mode chat call. Returns the assistant's message content (a JSON
    /// string) plus token usage. `system` should be byte-stable across calls so
    /// DeepSeek's prompt cache applies.
    pub fn chat_json(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, Usage)> {
        let req = ChatRequest {
            model,
            messages: vec![
                Msg {
                    role: "system",
                    content: system,
                },
                Msg {
                    role: "user",
                    content: user,
                },
            ],
            response_format: Some(ResponseFormat { kind: "json_object" }),
            temperature: 0.3,
            max_tokens,
        };
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .context("DeepSeek request failed (network?)")?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().context("DeepSeek response was not JSON")?;
        if !status.is_success() {
            bail!("DeepSeek error {status}: {body}");
        }
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .context("DeepSeek response had no message content")?
            .to_string();
        let u = &body["usage"];
        let usage = Usage {
            prompt: u["prompt_tokens"].as_u64().unwrap_or(0),
            cached: u["prompt_cache_hit_tokens"].as_u64().unwrap_or(0),
            completion: u["completion_tokens"].as_u64().unwrap_or(0),
        };
        Ok((content, usage))
    }
}
