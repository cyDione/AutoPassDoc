//! Model providers as the settings screen sees them: stored records, their
//! cached model lists, and each model's resolved capability profile.

use models::{
    ModelInfo, ModelProfile, ModelRole, PartialProfile, ProfileSource, Provider, ProviderKind,
    Reasoning, ThinkingLevel,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::secrets::SecretStore;
use crate::store::{ProviderRecord, Store};

pub fn kind_from_str(kind: &str) -> ProviderKind {
    match kind {
        "openrouter" => ProviderKind::OpenRouter,
        "anthropic" => ProviderKind::Anthropic,
        "ollama" => ProviderKind::Ollama,
        _ => ProviderKind::OpenAiCompatible,
    }
}

pub fn secret_name(provider_id: &str) -> String {
    format!("provider:{provider_id}")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    #[serde(flatten)]
    pub record: ProviderRecord,
    pub has_key: bool,
}

pub fn views(store: &Store, secrets: &SecretStore) -> Result<Vec<ProviderView>> {
    store
        .providers()?
        .into_iter()
        .map(|record| {
            let has_key = secrets.get(&secret_name(&record.id))?.is_some();
            Ok(ProviderView { record, has_key })
        })
        .collect()
}

/// The provider ready for a call, with its API key.
pub fn provider(store: &Store, secrets: &SecretStore, id: &str) -> Result<Provider> {
    let r = store
        .provider(id)?
        .ok_or_else(|| Error::Setup(format!("模型服务「{id}」不存在，请在设置中重新选择")))?;
    Ok(Provider {
        id: r.id.clone(),
        name: r.name,
        kind: kind_from_str(&r.kind),
        base_url: r.base_url,
        api_key: secrets.get(&secret_name(&r.id))?,
        decision_path: r.decision_path.filter(|p| !p.trim().is_empty()),
        rerank_path: r.rerank_path.filter(|p| !p.trim().is_empty()),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfileView {
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub levels: Vec<ThinkingLevel>,
    /// "none" | "effort" | "toggle" | "budget" | "always"
    pub reasoning: &'static str,
    pub json_mode: bool,
    /// "fetched" | "builtin" | "manual" | "default"
    pub source: &'static str,
}

impl From<&ModelProfile> for ModelProfileView {
    fn from(p: &ModelProfile) -> Self {
        Self {
            context_window: p.context_window,
            max_output_tokens: p.max_output_tokens,
            levels: models::available_levels(p),
            reasoning: match p.reasoning {
                Reasoning::None => "none",
                Reasoning::Effort(_) => "effort",
                Reasoning::Toggle => "toggle",
                Reasoning::Budget { .. } => "budget",
                Reasoning::AlwaysOn => "always",
            },
            json_mode: p.json_mode,
            source: match p.source {
                ProfileSource::Fetched => "fetched",
                ProfileSource::BuiltIn => "builtin",
                ProfileSource::Manual => "manual",
                ProfileSource::Default => "default",
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    pub role_hint: ModelRole,
    pub profile: ModelProfileView,
    pub manual: bool,
}

/// User overrides from the settings screen.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileOverride {
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub reasoning: Option<String>,
    pub json_mode: Option<bool>,
}

impl ProfileOverride {
    pub fn to_partial(&self) -> PartialProfile {
        use ThinkingLevel::*;
        PartialProfile {
            context_window: self.context_window.filter(|&n| n > 0),
            max_output_tokens: self.max_output_tokens.filter(|&n| n > 0),
            reasoning: self.reasoning.as_deref().map(|r| match r {
                "effort" => Reasoning::Effort(vec![Low, Medium, High]),
                "toggle" => Reasoning::Toggle,
                "budget" => Reasoning::Budget {
                    min: 1024,
                    max: 32_000,
                },
                "always" => Reasoning::AlwaysOn,
                _ => Reasoning::None,
            }),
            json_mode: self.json_mode,
            ..PartialProfile::default()
        }
    }
}

fn view(id: &str, info: Option<&ModelInfo>, manual: Option<&PartialProfile>) -> ModelView {
    let profile = models::resolve_profile(id, info, manual);
    ModelView {
        id: id.to_string(),
        role_hint: info.map_or_else(|| models::guess_role(id), |i| i.role_hint),
        profile: ModelProfileView::from(&profile),
        manual: manual.is_some(),
    }
}

/// The cached model list of a provider with resolved profiles.
pub fn model_views(store: &Store, provider_id: &str) -> Result<Vec<ModelView>> {
    Ok(store
        .provider_models(provider_id)?
        .into_iter()
        .map(|(id, info, manual)| {
            let info: Option<ModelInfo> = info.and_then(|i| serde_json::from_str(&i).ok());
            let manual: Option<PartialProfile> = manual.and_then(|m| serde_json::from_str(&m).ok());
            view(&id, info.as_ref(), manual.as_ref())
        })
        .collect())
}

pub fn cache_models(store: &Store, provider_id: &str, models: &[ModelInfo]) -> Result<()> {
    let rows: Vec<(String, String)> = models
        .iter()
        .map(|m| Ok((m.id.clone(), serde_json::to_string(m)?)))
        .collect::<Result<_>>()?;
    store.replace_provider_models(provider_id, &rows)
}

pub fn set_override(
    store: &Store,
    provider_id: &str,
    model_id: &str,
    over: Option<&ProfileOverride>,
) -> Result<ModelView> {
    let json = over
        .map(|o| serde_json::to_string(&o.to_partial()))
        .transpose()?;
    store.set_manual_profile(provider_id, model_id, json.as_deref())?;
    model_views(store, provider_id)?
        .into_iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| Error::Invalid(format!("找不到模型 {model_id}")))
}

/// Capability profile of a model: manual overrides, then what the provider
/// reported, then the built-in table.
pub fn profile(store: &Store, provider_id: &str, model_id: &str) -> Result<ModelProfile> {
    let row = store
        .provider_models(provider_id)?
        .into_iter()
        .find(|(id, _, _)| id == model_id);
    let (info, manual) = match row {
        Some((_, info, manual)) => (
            info.and_then(|i| serde_json::from_str::<ModelInfo>(&i).ok()),
            manual.and_then(|m| serde_json::from_str::<PartialProfile>(&m).ok()),
        ),
        None => (None, None),
    };
    Ok(models::resolve_profile(
        model_id,
        info.as_ref(),
        manual.as_ref(),
    ))
}

pub fn thinking(level: &str) -> Option<ThinkingLevel> {
    match level {
        "off" => Some(ThinkingLevel::Off),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_win_and_survive_refresh() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_provider(&ProviderRecord {
                id: "gw".into(),
                name: "网关".into(),
                kind: "openai".into(),
                base_url: "https://x/v1".into(),
                decision_path: None,
                rerank_path: None,
            })
            .unwrap();
        let fetched = vec![
            ModelInfo {
                id: "cline-pass/deepseek-v4.1-flash".into(),
                display_name: None,
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                role_hint: ModelRole::Chat,
            },
            ModelInfo {
                id: "BAAI/bge-m3".into(),
                display_name: None,
                context_window: Some(8192),
                max_output_tokens: None,
                reasoning: None,
                role_hint: ModelRole::Embedding,
            },
        ];
        cache_models(&store, "gw", &fetched).unwrap();
        let views = model_views(&store, "gw").unwrap();
        let chat = views.iter().find(|v| v.id.contains("deepseek")).unwrap();
        assert_eq!(chat.profile.source, "builtin");
        assert!(
            !chat.profile.levels.is_empty(),
            "deepseek offers thinking levels"
        );
        let bge = views.iter().find(|v| v.id == "BAAI/bge-m3").unwrap();
        assert_eq!(
            (bge.profile.context_window, bge.profile.source),
            (8192, "fetched")
        );

        let over = ProfileOverride {
            context_window: Some(65_536),
            reasoning: Some("none".into()),
            ..Default::default()
        };
        let v = set_override(&store, "gw", "cline-pass/deepseek-v4.1-flash", Some(&over)).unwrap();
        assert_eq!(
            (v.profile.context_window, v.profile.source, v.manual),
            (65_536, "manual", true)
        );
        assert!(v.profile.levels.is_empty());

        cache_models(&store, "gw", &fetched).unwrap();
        let p = profile(&store, "gw", "cline-pass/deepseek-v4.1-flash").unwrap();
        assert_eq!(p.context_window, 65_536);
    }
}
