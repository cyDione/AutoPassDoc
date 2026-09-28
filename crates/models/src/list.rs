//! Model listing for each provider kind.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Client, endpoint_label};
use crate::error::{Error, Result};
use crate::profile::{Reasoning, ThinkingLevel};
use crate::provider::{ModelRole, Provider, ProviderKind, guess_role};

/// A model reported by a provider, with whatever metadata it exposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model id to send in requests.
    pub id: String,
    /// Human-friendly name, when the provider has one.
    pub display_name: Option<String>,
    /// Context window reported by the provider.
    pub context_window: Option<u32>,
    /// Output limit reported by the provider.
    pub max_output_tokens: Option<u32>,
    /// Reasoning control reported by the provider (`None`: unknown).
    pub reasoning: Option<Reasoning>,
    /// Likely role, from provider metadata or [`guess_role`].
    pub role_hint: ModelRole,
}

impl ModelInfo {
    fn bare(id: String) -> Self {
        Self {
            role_hint: guess_role(&id),
            id,
            display_name: None,
            context_window: None,
            max_output_tokens: None,
            reasoning: None,
        }
    }
}

impl Client {
    /// Lists the provider's models.
    ///
    /// - OpenAI-compatible / OpenRouter: `GET models`, reading the optional
    ///   `context_length` / `context_window` / `max_model_len` /
    ///   `max_context_length`, `top_provider.max_completion_tokens` and
    ///   `supported_parameters` fields.
    /// - Anthropic: `GET /v1/models`.
    /// - Ollama: `GET /api/tags`, then `POST /api/show` per model for the
    ///   context length (failures of `show` are ignored).
    pub async fn list_models(&self, p: &Provider) -> Result<Vec<ModelInfo>> {
        match p.kind {
            ProviderKind::OpenAiCompatible | ProviderKind::OpenRouter => {
                let url = p.openai_url("models")?;
                let v = self.get_json(p, &url).await?;
                let items = model_array(&v, &["data", "models"])
                    .ok_or_else(|| Error::decode(&endpoint_label("GET", &url), "缺少 data 数组"))?;
                Ok(items.iter().filter_map(parse_openai_model).collect())
            }
            ProviderKind::Anthropic => {
                let url = p.url("v1/models?limit=1000")?;
                let v = self.get_json(p, &url).await?;
                let items = model_array(&v, &["data"])
                    .ok_or_else(|| Error::decode(&endpoint_label("GET", &url), "缺少 data 数组"))?;
                Ok(items
                    .iter()
                    .filter_map(|m| {
                        let mut info = ModelInfo::bare(str_field(m, &["id"])?);
                        info.display_name = str_field(m, &["display_name"]);
                        Some(info)
                    })
                    .collect())
            }
            ProviderKind::Ollama => self.list_ollama(p).await,
        }
    }

    async fn list_ollama(&self, p: &Provider) -> Result<Vec<ModelInfo>> {
        let v = self.get_json(p, &p.url("api/tags")?).await?;
        let items = model_array(&v, &["models"])
            .ok_or_else(|| Error::decode("GET /api/tags", "缺少 models 数组"))?;
        let show_url = p.url("api/show")?;
        let mut out = Vec::with_capacity(items.len());
        for m in items {
            let Some(name) = str_field(m, &["name", "model"]) else {
                continue;
            };
            let mut info = ModelInfo::bare(name);
            if let Ok(show) = self
                .post_json(p, &show_url, &json!({ "model": info.id }))
                .await
            {
                apply_ollama_show(&mut info, &show);
            }
            out.push(info);
        }
        Ok(out)
    }
}

fn model_array<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Vec<Value>> {
    v.as_array()
        .or_else(|| keys.iter().find_map(|k| v.get(k)?.as_array()))
}

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        let s = v.get(k)?.as_str()?.trim();
        (!s.is_empty()).then(|| s.to_string())
    })
}

/// Reads a positive integer given as a number or numeric string.
fn u32_field(v: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|k| {
        let f = v.get(k)?;
        let n = f
            .as_u64()
            .or_else(|| f.as_f64().map(|x| x as u64))
            .or_else(|| f.as_str()?.trim().parse().ok())?;
        (n > 0).then(|| n.min(u32::MAX as u64) as u32)
    })
}

fn parse_openai_model(m: &Value) -> Option<ModelInfo> {
    let mut info = ModelInfo::bare(str_field(m, &["id"])?);
    info.display_name = str_field(m, &["display_name", "name"]).filter(|n| *n != info.id);
    let top = m.get("top_provider").unwrap_or(&Value::Null);
    info.context_window = u32_field(
        m,
        &[
            "context_length",
            "context_window",
            "max_model_len",
            "max_context_length",
        ],
    )
    .or_else(|| u32_field(top, &["context_length"]));
    info.max_output_tokens = u32_field(top, &["max_completion_tokens"])
        .or_else(|| u32_field(m, &["max_completion_tokens", "max_output_tokens"]));
    if let Some(params) = m.get("supported_parameters").and_then(Value::as_array) {
        let supports = |name: &str| params.iter().any(|p| p.as_str() == Some(name));
        // OpenRouter normalizes `reasoning: {effort}` / `{enabled: false}` across vendors.
        info.reasoning = Some(if supports("reasoning") {
            use ThinkingLevel::*;
            Reasoning::Effort(vec![Off, Low, Medium, High])
        } else {
            Reasoning::None
        });
    }
    Some(info)
}

fn apply_ollama_show(info: &mut ModelInfo, show: &Value) {
    if let Some(model_info) = show.get("model_info").and_then(Value::as_object) {
        let arch_key = model_info
            .get("general.architecture")
            .and_then(Value::as_str)
            .map(|a| format!("{a}.context_length"));
        info.context_window = arch_key
            .and_then(|k| model_info.get(&k))
            .or_else(|| {
                model_info
                    .iter()
                    .find(|(k, _)| k.ends_with(".context_length"))
                    .map(|(_, v)| v)
            })
            .and_then(Value::as_u64)
            .map(|n| n.min(u32::MAX as u64) as u32);
    }
    if let Some(caps) = show.get("capabilities").and_then(Value::as_array) {
        let has = |c: &str| caps.iter().any(|x| x.as_str() == Some(c));
        if has("embedding") && !has("completion") {
            info.role_hint = ModelRole::Embedding;
        }
        if has("thinking") {
            info.reasoning = Some(Reasoning::Toggle);
        }
    }
}
