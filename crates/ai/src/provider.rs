//! Where drafts come from: settings types and provider dispatch.

use serde::Deserialize;
use serde::Serialize;

use crate::assistant::{ChatTurn, Draft};

/// Which provider turns a request into a command draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiMode {
    /// Bundled deterministic rule engine. No network, no API key.
    #[default]
    Offline,
    /// OpenAI-compatible chat completions endpoint (`openai` module, native builds only).
    OpenAi,
}

/// Endpoint configuration for the OpenAI-compatible provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OpenAiSettings {
    /// Examples: `https://api.deepseek.com/v1`,
    /// `https://dashscope.aliyuncs.com/compatible-mode/v1`,
    /// `https://api.openai.com/v1`, `http://127.0.0.1:11434/v1` (Ollama).
    pub base_url: String,
    /// Sent as `Authorization: Bearer <key>`; may be empty for local endpoints.
    pub api_key: String,
    /// Model name shown to the endpoint, e.g. `deepseek-chat`.
    pub model: String,
    /// Sampling temperature, `0.0` for deterministic drafting.
    pub temperature: f64,
}

impl Default for OpenAiSettings {
    fn default() -> Self {
        Self { base_url: String::from("https://api.openai.com/v1"), api_key: String::new(), model: String::new(), temperature: 0.2 }
    }
}

/// Everything the AI window needs to produce a draft.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    pub mode: AiMode,
    pub openai: OpenAiSettings,
}

impl AiSettings {
    /// Offline mode works everywhere; OpenAI needs the native build and a key.
    pub fn usable(&self) -> Result<(), String> {
        match self.mode {
            AiMode::Offline => Ok(()),
            AiMode::OpenAi => {
                if cfg!(target_arch = "wasm32") {
                    return Err(String::from("The OpenAI provider is native-only; switch to the offline rule engine for the web build."));
                }
                if self.openai.base_url.trim().is_empty() {
                    Err(String::from("OpenAI mode is missing its Base URL (设置里还没有 Base URL)"))
                } else if self.openai.model.trim().is_empty() {
                    Err(String::from("OpenAI mode is missing its model name (设置里还没有模型名)"))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Produce a draft for `user` given conversation history and a drawing context block.
pub fn generate(settings: &AiSettings, context: &str, history: &[ChatTurn], user: &str) -> Result<Draft, String> {
    // The wasm build has no HTTP provider, so the history is only read natively.
    #[cfg(target_arch = "wasm32")]
    let _ = history;
    settings.usable()?;
    match settings.mode {
        AiMode::Offline => crate::rules::offline_draft(user, context),
        AiMode::OpenAi => {
            #[cfg(target_arch = "wasm32")]
            {
                Err(String::from("OpenAI provider is unavailable on wasm; the offline rule engine still works"))
            }
            #[cfg(not(target_arch = "wasm32"))]
            crate::openai::chat(&settings.openai, context, history, user)
        }
    }
}
