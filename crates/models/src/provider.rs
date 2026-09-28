//! Provider configuration, base URL normalization and model role guessing.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Wire protocol spoken by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any OpenAI-style API (DeepSeek, DashScope, Zhipu, Moonshot, SiliconFlow,
    /// vLLM, LM Studio, one-api style gateways...).
    OpenAiCompatible,
    /// OpenRouter: OpenAI-style, with richer model metadata and `reasoning`.
    OpenRouter,
    /// Anthropic Messages API.
    Anthropic,
    /// Ollama: native API for model listing, OpenAI-style API for inference.
    Ollama,
}

/// What a model is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Text generation (revisions, summaries).
    Chat,
    /// Probability-calibrated judgement (Jev / Laya).
    Decision,
    /// Text embeddings.
    Embedding,
    /// Query-document relevance scoring.
    Rerank,
}

/// Default path (relative to the OpenAI-style base) of the Jev decision endpoint.
pub const DEFAULT_DECISION_PATH: &str = "systemone";
/// Default path (relative to the OpenAI-style base) of the rerank endpoint.
pub const DEFAULT_RERANK_PATH: &str = "rerank";

/// A configured model provider. The API key is supplied by the caller (this
/// crate never stores it); it is redacted from `Debug` and never serialized.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    /// Caller-defined identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Wire protocol.
    pub kind: ProviderKind,
    /// Base URL as entered by the user; normalized with [`normalize_base_url`].
    pub base_url: String,
    /// API key. Skipped when serializing so it cannot leak into storage.
    #[serde(skip_serializing, default)]
    pub api_key: Option<String>,
    /// Jev endpoint path relative to the OpenAI-style base, or an absolute URL.
    /// Defaults to [`DEFAULT_DECISION_PATH`].
    #[serde(default)]
    pub decision_path: Option<String>,
    /// Rerank endpoint path relative to the OpenAI-style base, or an absolute URL.
    /// Defaults to [`DEFAULT_RERANK_PATH`].
    #[serde(default)]
    pub rerank_path: Option<String>,
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("decision_path", &self.decision_path)
            .field("rerank_path", &self.rerank_path)
            .finish()
    }
}

impl Provider {
    /// Creates a provider with empty id/name, no key and default paths.
    pub fn new(kind: ProviderKind, base_url: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind,
            base_url: base_url.into(),
            api_key: None,
            decision_path: None,
            rerank_path: None,
        }
    }

    /// Sets the API key.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Sets the Jev decision endpoint path.
    pub fn with_decision_path(mut self, path: impl Into<String>) -> Self {
        self.decision_path = Some(path.into());
        self
    }

    /// Sets the rerank endpoint path.
    pub fn with_rerank_path(mut self, path: impl Into<String>) -> Self {
        self.rerank_path = Some(path.into());
        self
    }

    /// The key, if set and non-blank.
    pub(crate) fn key(&self) -> Option<&str> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
    }

    /// Normalized base URL, e.g. `https://host/v1` or `http://localhost:11434`.
    pub(crate) fn base(&self) -> Result<String> {
        let base = normalize_base_url(self.kind, &self.base_url);
        if base.is_empty() {
            return Err(Error::InvalidConfig("配置无效：Base URL 为空".into()));
        }
        Ok(base)
    }

    /// Base for OpenAI-style endpoints (`models`, `chat/completions`, ...).
    pub(crate) fn openai_base(&self) -> Result<String> {
        let base = self.base()?;
        Ok(match self.kind {
            ProviderKind::Ollama => format!("{base}/v1"),
            _ => base,
        })
    }

    /// Joins `path` onto the OpenAI-style base; absolute URLs are returned as is.
    pub(crate) fn openai_url(&self, path: &str) -> Result<String> {
        let path = path.trim();
        if path.starts_with("http://") || path.starts_with("https://") {
            return Ok(path.to_string());
        }
        Ok(format!(
            "{}/{}",
            self.openai_base()?,
            path.trim_start_matches('/')
        ))
    }

    /// Joins `path` onto the provider base (native endpoints).
    pub(crate) fn url(&self, path: &str) -> Result<String> {
        Ok(format!("{}/{}", self.base()?, path.trim_start_matches('/')))
    }

    pub(crate) fn decision_path(&self) -> &str {
        non_blank(&self.decision_path).unwrap_or(DEFAULT_DECISION_PATH)
    }

    pub(crate) fn rerank_path(&self) -> &str {
        non_blank(&self.rerank_path).unwrap_or(DEFAULT_RERANK_PATH)
    }

    /// Fails with a user-facing error when `kind` cannot serve `what`.
    pub(crate) fn require_openai_style(&self, what: &str) -> Result<()> {
        if self.kind == ProviderKind::Anthropic {
            return Err(Error::InvalidConfig(format!(
                "配置无效：Anthropic 不提供{what}接口，请换用其他服务商"
            )));
        }
        Ok(())
    }
}

fn non_blank(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Normalizes a user-entered base URL for `kind`.
///
/// - Trims whitespace and trailing `/`; adds a scheme when missing
///   (`http://` for localhost, `https://` otherwise).
/// - OpenAI-compatible / OpenRouter: strips a pasted `/chat/completions` and
///   appends `/v1` when no path segment is a version (`v1`, `v4`, `v1beta`...).
///   A bare `https://openrouter.ai` becomes `https://openrouter.ai/api/v1`.
/// - Anthropic: host only (endpoints are `/v1/...`), default `https://api.anthropic.com`.
/// - Ollama: host only, default `http://localhost:11434`.
pub fn normalize_base_url(kind: ProviderKind, url: &str) -> String {
    let mut url = url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return match kind {
            ProviderKind::OpenAiCompatible => String::new(),
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1".into(),
            ProviderKind::Anthropic => "https://api.anthropic.com".into(),
            ProviderKind::Ollama => "http://localhost:11434".into(),
        };
    }
    if !url.contains("://") {
        let local = ["localhost", "127.", "0.0.0.0", "[::1]"]
            .iter()
            .any(|p| url.starts_with(p));
        url = format!("{}://{url}", if local { "http" } else { "https" });
    }
    let (origin, path) = split_origin(&url);
    let path = path.trim_end_matches('/');
    match kind {
        ProviderKind::OpenAiCompatible | ProviderKind::OpenRouter => {
            let path = path.strip_suffix("/chat/completions").unwrap_or(path);
            if path.split('/').any(is_version_segment) {
                format!("{origin}{path}")
            } else if path.is_empty() && origin.ends_with("openrouter.ai") {
                format!("{origin}/api/v1")
            } else {
                format!("{origin}{path}/v1")
            }
        }
        ProviderKind::Anthropic => {
            let path = path.strip_suffix("/messages").unwrap_or(path);
            format!("{origin}{}", path.strip_suffix("/v1").unwrap_or(path))
        }
        ProviderKind::Ollama => {
            let path = path
                .strip_suffix("/v1")
                .or_else(|| path.strip_suffix("/api"))
                .unwrap_or(path);
            format!("{origin}{path}")
        }
    }
}

/// Splits `scheme://host[:port]` from the path (which keeps its leading `/`).
fn split_origin(url: &str) -> (&str, &str) {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    match url[after_scheme..].find('/') {
        Some(i) => url.split_at(after_scheme + i),
        None => (url, ""),
    }
}

/// `v1`, `v2`, `v4`, `v1beta`, `v1alpha2`...
fn is_version_segment(seg: &str) -> bool {
    let Some(rest) = seg.strip_prefix('v') else {
        return false;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest[digits..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// Lowercased model id without its `org/` prefix (e.g. `BAAI/bge-m3` → `bge-m3`).
pub(crate) fn base_id(model_id: &str) -> String {
    model_id
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// Guesses a model's role from its id (case-insensitive, `org/` prefix ignored).
pub fn guess_role(model_id: &str) -> ModelRole {
    let id = base_id(model_id);
    let has = |needles: &[&str]| needles.iter().any(|n| id.contains(n));
    let word = |w: &str| id.starts_with(w) || id.contains(&format!("-{w}"));
    if id.contains("rerank") {
        ModelRole::Rerank
    } else if has(&[
        "embed",
        "bge-m3",
        "bge-large",
        "bge-base",
        "bge-small",
        "text-embedding",
        "m3e",
        "jina-embeddings",
    ]) || word("e5-")
        || word("gte-")
    {
        ModelRole::Embedding
    } else if has(&["jev", "laya"]) {
        ModelRole::Decision
    } else {
        ModelRole::Chat
    }
}
