//! DeepSeek client (OpenAI-compatible). Blocking reqwest — the batch enricher is a
//! CLI job and any live use runs on a worker thread, so no async runtime is needed.

pub mod enrich;
pub mod socratic;

use std::time::Duration;

use anyhow::{Context, Result, bail};
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

#[derive(Serialize)]
struct MultiChatRequest<'a> {
    model: &'a str,
    messages: Vec<OwnedMsg>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Serialize)]
struct OwnedMsg {
    role: String,
    content: String,
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

    /// A JSON-mode chat call. `system` should be byte-stable across calls so
    /// DeepSeek's prompt cache applies.
    pub fn chat_json(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, Usage)> {
        self.chat(model, system, user, max_tokens, true)
    }

    /// A plain-text chat call (for Socratic dialogue — no JSON).
    pub fn chat_text(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<(String, Usage)> {
        self.chat(model, system, user, max_tokens, false)
    }

    /// A multi-turn plain-text chat call. `messages` is a sequence of
    /// (role, content) pairs — the first should be ("system", system_prompt),
    /// followed by prior turns ("user" / "assistant") and the new user message.
    pub fn chat_multi(
        &self,
        model: &str,
        messages: Vec<(&str, String)>,
        max_tokens: u32,
    ) -> Result<String> {
        let req = MultiChatRequest {
            model,
            messages: messages
                .into_iter()
                .map(|(role, content)| OwnedMsg {
                    role: role.to_string(),
                    content,
                })
                .collect(),
            temperature: 0.4,
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
        let msg = &body["choices"][0]["message"];
        let content = msg["content"].as_str().unwrap_or("").trim().to_string();
        // Reasoning models put the chain-of-thought in `reasoning_content` and the
        // answer in `content`. When max_tokens runs out mid-reasoning, `content`
        // comes back EMPTY while the call still succeeds — surfaced as an error
        // here, because an empty bubble in the chat reads as the reply having been
        // swallowed. finish_reason tells the two cases apart.
        if content.is_empty() {
            let finish = body["choices"][0]["finish_reason"]
                .as_str()
                .unwrap_or("unknown");
            let reasoned = msg["reasoning_content"]
                .as_str()
                .map(|r| !r.trim().is_empty())
                .unwrap_or(false);
            if reasoned || finish == "length" {
                bail!("回答被截断：思维链耗尽了 max_tokens（finish_reason={finish}）");
            }
            bail!("模型返回了空内容（finish_reason={finish}）");
        }
        Ok(content)
    }

    fn chat(
        &self,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u32,
        json: bool,
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
            response_format: json.then_some(ResponseFormat {
                kind: "json_object",
            }),
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
        // Same failure shape as chat_multi: a reasoning model that exhausts
        // max_tokens mid-chain returns success with empty content.
        if content.trim().is_empty() {
            let finish = body["choices"][0]["finish_reason"]
                .as_str()
                .unwrap_or("unknown");
            bail!("模型返回了空内容（finish_reason={finish}，可能是思维链耗尽 max_tokens）");
        }
        let u = &body["usage"];
        let usage = Usage {
            prompt: u["prompt_tokens"].as_u64().unwrap_or(0),
            cached: u["prompt_cache_hit_tokens"].as_u64().unwrap_or(0),
            completion: u["completion_tokens"].as_u64().unwrap_or(0),
        };
        Ok((content, usage))
    }
}
