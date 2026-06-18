//! Local LLM client — auto-detects Ollama (11434) or LM Studio (1234).
//!
//! Both expose an OpenAI-compatible /v1/chat/completions endpoint,
//! so a single implementation covers both.

use std::time::Duration;

const OLLAMA_URL: &str = "http://localhost:11434";
const LMSTUDIO_URL: &str = "http://localhost:1234";
const DEFAULT_MODEL_OLLAMA: &str = "llama3.2";
const DEFAULT_MODEL_LMSTUDIO: &str = "local-model";
const DETECT_TIMEOUT_MS: u64 = 800;
const EXPLAIN_TIMEOUT_MS: u64 = 30_000;

pub struct LlmClient {
    base_url: String,
    model: String,
}

impl LlmClient {
    /// Auto-detect a running local LLM. Checks Ollama first, then LM Studio.
    /// Returns `None` if neither is reachable.
    pub fn detect(url_override: Option<&str>, model_override: Option<&str>) -> Option<Self> {
        // Env vars as fallback
        let url_env = std::env::var("TURBOLOG_LLM_URL").ok();
        let model_env = std::env::var("TURBOLOG_LLM_MODEL").ok();

        let url = url_override.or(url_env.as_deref());
        let model = model_override.or(model_env.as_deref());

        if let Some(url) = url {
            let url = url.trim_end_matches('/').to_string();
            let m = model.unwrap_or(DEFAULT_MODEL_OLLAMA).to_string();
            return Some(Self {
                base_url: url,
                model: m,
            });
        }

        // Auto-detect order: Ollama → LM Studio
        for (base, default_model) in [
            (OLLAMA_URL, DEFAULT_MODEL_OLLAMA),
            (LMSTUDIO_URL, DEFAULT_MODEL_LMSTUDIO),
        ] {
            if Self::is_reachable(base) {
                let m = model.unwrap_or(default_model).to_string();
                return Some(Self {
                    base_url: base.to_string(),
                    model: m,
                });
            }
        }

        None
    }

    /// Explains an anomalous log line. Returns `None` on error or timeout.
    /// `context` is an optional one-line history hint (e.g. "seen 3× in last 7 days").
    pub fn explain(&self, log_line: &str, score: f32, context: Option<&str>) -> Option<String> {
        let user_content = match context {
            Some(ctx) => format!(
                "Context: {ctx}\n\nAnomalous log line (score: {score:.2}, higher = more unusual):\n\n{log_line}"
            ),
            None => format!(
                "Anomalous log line (score: {score:.2}, higher = more unusual):\n\n{log_line}"
            ),
        };
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a concise log analysis assistant for developers. \
                                Explain anomalous log lines briefly: what likely went wrong \
                                and what to check. Answer in 1-2 sentences max. \
                                No preamble, no markdown."
                },
                {
                    "role": "user",
                    "content": user_content
                }
            ],
            "max_tokens": 120,
            "temperature": 0.3,
            "stream": false
        });

        let resp = ureq::post(&format!("{}/v1/chat/completions", self.base_url))
            .timeout(Duration::from_millis(EXPLAIN_TIMEOUT_MS))
            .set("Content-Type", "application/json")
            .send_json(&body)
            .ok()?;

        let json: serde_json::Value = resp.into_json().ok()?;
        let text = json["choices"][0]["message"]["content"]
            .as_str()?
            .trim()
            .to_string();

        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// A server is "reachable" if we get any HTTP response (even 4xx).
    /// Only network errors mean it's not running.
    fn is_reachable(base_url: &str) -> bool {
        match ureq::get(&format!("{}/v1/models", base_url))
            .timeout(Duration::from_millis(DETECT_TIMEOUT_MS))
            .call()
        {
            Ok(_) => true,
            Err(ureq::Error::Status(_, _)) => true,
            Err(_) => false,
        }
    }
}
