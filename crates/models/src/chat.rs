//! Chat completions (OpenAI-style and Anthropic Messages), with thinking
//! parameter mapping and a one-shot fallback for rejected optional parameters.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::client::{Client, Failure, endpoint_label};
use crate::error::{Error, Result};
use crate::profile::{
    ModelProfile, Reasoning, ThinkingLevel, ThinkingParam, effective_level, resolve_profile,
};
use crate::provider::{Provider, ProviderKind};

/// Author of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Instructions.
    System,
    /// The user.
    User,
    /// The model.
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// One chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Author.
    pub role: Role,
    /// Plain-text content.
    pub content: String,
}

impl Message {
    /// A system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    /// A user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    /// An assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// A chat request. `profile` decides how `thinking` and `json_output` are sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model id.
    pub model: String,
    /// Conversation; system messages become Anthropic's top-level `system`.
    pub messages: Vec<Message>,
    /// Output limit. Anthropic requires one and falls back to the profile's.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Desired thinking level; ignored when the profile does not support it.
    pub thinking: Option<ThinkingLevel>,
    /// Capabilities of `model`.
    pub profile: ModelProfile,
    /// Ask for a JSON object (`response_format`) when the profile allows it.
    pub json_output: bool,
    /// Switch on the provider's own web search.
    pub web_search: Option<WebSearch>,
}

/// How a provider's own web search is switched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearch {
    /// OpenRouter's `web` plugin; sources come back as `url_citation` annotations.
    OpenRouter,
    /// Anthropic's server-side `web_search` tool.
    Anthropic,
    /// Alibaba Cloud Model Studio (DashScope) `enable_search`.
    DashScope,
    /// Zhipu's `web_search` tool.
    Zhipu,
    /// OpenAI search models' `web_search_options`.
    OpenAi,
}

impl WebSearch {
    /// The search a provider offers, judged from its protocol and host.
    /// Plain OpenAI-compatible services get `None`: most cannot search, and
    /// the few that can say so through a setting.
    pub fn detect(p: &Provider) -> Option<Self> {
        match p.kind {
            ProviderKind::OpenRouter => Some(Self::OpenRouter),
            ProviderKind::Anthropic => Some(Self::Anthropic),
            ProviderKind::Ollama => None,
            ProviderKind::OpenAiCompatible => {
                let base = p.base_url.to_ascii_lowercase();
                if base.contains("dashscope.aliyuncs.com")
                    || base.contains("dashscope-intl.aliyuncs.com")
                {
                    Some(Self::DashScope)
                } else if base.contains("bigmodel.cn") {
                    Some(Self::Zhipu)
                } else if base.contains("openrouter.ai") {
                    Some(Self::OpenRouter)
                } else {
                    None
                }
            }
        }
    }
}

/// A web page the model's search used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlCitation {
    /// Page address.
    pub url: String,
    /// Page title, possibly empty.
    pub title: String,
    /// Page excerpt, when the provider returns one.
    #[serde(default)]
    pub snippet: String,
}

impl ChatRequest {
    /// A request with the built-in (or default) profile for `model`.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        let model = model.into();
        Self {
            profile: resolve_profile(&model, None, None),
            model,
            messages,
            max_tokens: None,
            temperature: None,
            thinking: None,
            json_output: false,
            web_search: None,
        }
    }
}

/// Token usage reported by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens.
    pub prompt_tokens: u32,
    /// Output tokens (including reasoning where the provider counts it).
    pub completion_tokens: u32,
}

/// A chat answer with any reasoning split out of the content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// The answer, without any `<think>` block.
    pub content: String,
    /// Reasoning text (`reasoning_content`, `reasoning`, thinking blocks or `<think>`).
    pub reasoning: Option<String>,
    /// Why generation stopped (`stop`, `length`, `end_turn`...).
    pub finish_reason: Option<String>,
    /// Token usage.
    pub usage: Option<Usage>,
    /// Sources of a web search, in the order the provider listed them.
    #[serde(default)]
    pub citations: Vec<UrlCitation>,
}

/// Which parts of the request to send.
#[derive(Clone, Copy)]
struct BodyOptions {
    /// Temperature, thinking and `response_format`.
    optional: bool,
    /// Send `max_completion_tokens` instead of `max_tokens`.
    max_completion_tokens: bool,
}

const FULL: BodyOptions = BodyOptions {
    optional: true,
    max_completion_tokens: false,
};

impl Client {
    /// Sends a chat request. If the server rejects it with 400/422 and the
    /// error points at an optional parameter (thinking, `response_format`,
    /// temperature, unknown parameter...), retries once without them.
    pub async fn chat(&self, p: &Provider, req: &ChatRequest) -> Result<ChatResponse> {
        let url = match p.kind {
            ProviderKind::Anthropic => p.url("v1/messages")?,
            _ => p.openai_url("chat/completions")?,
        };
        let endpoint = endpoint_label("POST", &url);
        let body = chat_body(p.kind, req, FULL);
        let failure = match self.post_json_raw(p, &url, &body).await {
            Ok(v) => return parse_response(p.kind, &endpoint, &v),
            Err(f) => f,
        };
        if let Failure::Status {
            status: 400 | 422,
            body: err,
            ..
        } = &failure
        {
            let err = err.to_lowercase();
            if REJECTION_HINTS.iter().any(|h| err.contains(h)) {
                let opts = BodyOptions {
                    optional: false,
                    max_completion_tokens: err.contains("max_completion_tokens"),
                };
                let reduced = chat_body(p.kind, req, opts);
                if reduced != body {
                    let v = self.post_json(p, &url, &reduced).await?;
                    return parse_response(p.kind, &endpoint, &v);
                }
            }
        }
        Err(failure.into())
    }
}

/// Lowercased fragments of error bodies that suggest an optional parameter was rejected.
const REJECTION_HINTS: &[&str] = &[
    "thinking",
    "reasoning",
    "response_format",
    "json_object",
    "json_mode",
    "temperature",
    "max_completion_tokens",
    "unknown",
    "unsupported",
    "not support",
    "unrecognized",
    "extra inputs",
    "extra_forbidden",
    "not permitted",
    "invalid param",
    "不支持",
    "未知",
    "参数",
];

fn chat_body(kind: ProviderKind, req: &ChatRequest, opts: BodyOptions) -> Value {
    let level = effective_level(&req.profile, req.thinking).filter(|_| opts.optional);
    let mut obj = Map::new();
    obj.insert("model".into(), req.model.clone().into());

    if kind == ProviderKind::Anthropic {
        let max_tokens = req.max_tokens.unwrap_or(req.profile.max_output_tokens);
        let system: Vec<&str> = req
            .messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect();
        obj.insert("max_tokens".into(), max_tokens.into());
        if !system.is_empty() {
            obj.insert("system".into(), system.join("\n\n").into());
        }
        let messages: Vec<Value> = req
            .messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();
        obj.insert("messages".into(), messages.into());
        let budget = level.and_then(|l| anthropic_budget(&req.profile, l, max_tokens));
        if let Some(budget) = budget {
            // Anthropic rejects a custom temperature while thinking.
            obj.insert(
                "thinking".into(),
                json!({ "type": "enabled", "budget_tokens": budget }),
            );
        } else if let Some(t) = req.temperature.filter(|_| opts.optional) {
            obj.insert("temperature".into(), f32_json(t.clamp(0.0, 1.0)));
        }
        if req.web_search.is_some() {
            obj.insert(
                "tools".into(),
                json!([{ "type": "web_search_20250305", "name": "web_search", "max_uses": 3 }]),
            );
        }
        return Value::Object(obj);
    }

    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
        .collect();
    obj.insert("messages".into(), messages.into());
    if let Some(n) = req.max_tokens {
        let key = if opts.max_completion_tokens {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        obj.insert(key.into(), n.into());
    }
    if opts.optional {
        if let Some(t) = req.temperature {
            obj.insert("temperature".into(), f32_json(t));
        }
        if req.json_output && req.profile.json_mode {
            obj.insert("response_format".into(), json!({ "type": "json_object" }));
        }
    }
    if let Some(level) = level {
        insert_thinking(&mut obj, kind, req, level);
    }
    match req.web_search {
        Some(WebSearch::OpenRouter) => {
            obj.insert("plugins".into(), json!([{ "id": "web", "max_results": 5 }]));
        }
        Some(WebSearch::DashScope) => {
            obj.insert("enable_search".into(), true.into());
            obj.insert(
                "search_options".into(),
                json!({ "enable_source": true, "forced_search": true }),
            );
        }
        Some(WebSearch::Zhipu) => {
            obj.insert(
                "tools".into(),
                json!([{ "type": "web_search", "web_search": { "enable": true, "search_result": true } }]),
            );
        }
        Some(WebSearch::OpenAi) => {
            obj.insert("web_search_options".into(), json!({}));
        }
        Some(WebSearch::Anthropic) | None => {}
    }
    Value::Object(obj)
}

/// Web sources from any of the places providers put them.
fn citations(kind: ProviderKind, v: &Value) -> Vec<UrlCitation> {
    let mut out: Vec<UrlCitation> = Vec::new();
    let mut add = |url: Option<&str>, title: Option<&str>, snippet: Option<&str>| {
        let Some(url) = url
            .map(str::trim)
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        else {
            return;
        };
        if out.iter().any(|c| c.url == url) {
            return;
        }
        out.push(UrlCitation {
            url: url.to_string(),
            title: title.unwrap_or_default().trim().to_string(),
            snippet: snippet.unwrap_or_default().trim().to_string(),
        });
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(|x| x.to_owned());
    if kind == ProviderKind::Anthropic {
        for block in v
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(results) = block.get("content").and_then(Value::as_array)
                && block.get("type").and_then(Value::as_str) == Some("web_search_tool_result")
            {
                for r in results {
                    add(s(r, "url").as_deref(), s(r, "title").as_deref(), None);
                }
            }
            for c in block
                .get("citations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                add(
                    s(c, "url").as_deref(),
                    s(c, "title").as_deref(),
                    s(c, "cited_text").as_deref(),
                );
            }
        }
        return out;
    }
    let message = v.pointer("/choices/0/message").unwrap_or(&Value::Null);
    for a in message
        .get("annotations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let c = a.get("url_citation").unwrap_or(a);
        add(
            s(c, "url").as_deref(),
            s(c, "title").as_deref(),
            s(c, "content").as_deref(),
        );
    }
    // DashScope: `search_info.search_results`; Zhipu: `web_search`.
    for list in [
        v.pointer("/search_info/search_results"),
        message.pointer("/search_info/search_results"),
        v.get("web_search"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_array)
    {
        for r in list {
            let url = s(r, "url").or_else(|| s(r, "link"));
            add(
                url.as_deref(),
                s(r, "title").as_deref(),
                s(r, "content").as_deref(),
            );
        }
    }
    out
}

/// Adds the thinking parameter for an OpenAI-style request.
fn insert_thinking(
    obj: &mut Map<String, Value>,
    kind: ProviderKind,
    req: &ChatRequest,
    level: ThinkingLevel,
) {
    let on = level != ThinkingLevel::Off;
    let style = match kind {
        ProviderKind::OpenRouter => ThinkingParam::OpenRouterReasoning,
        ProviderKind::Ollama => ThinkingParam::ReasoningEffort,
        _ => req.profile.thinking_param,
    };
    let (key, value) = match style {
        ThinkingParam::ReasoningEffort => ("reasoning_effort", level.effort_str().into()),
        ThinkingParam::OpenRouterReasoning if on => {
            ("reasoning", json!({ "effort": level.effort_str() }))
        }
        ThinkingParam::OpenRouterReasoning => ("reasoning", json!({ "enabled": false })),
        ThinkingParam::EnableThinking => ("enable_thinking", on.into()),
        ThinkingParam::ThinkingType => (
            "thinking",
            json!({ "type": if on { "enabled" } else { "disabled" } }),
        ),
        ThinkingParam::AnthropicBudget => {
            let cap = req.max_tokens.unwrap_or(req.profile.max_output_tokens);
            match anthropic_budget(&req.profile, level, cap) {
                Some(b) => ("thinking", json!({ "type": "enabled", "budget_tokens": b })),
                None => return,
            }
        }
    };
    obj.insert(key.into(), value);
}

/// Anthropic's minimum thinking budget.
const MIN_BUDGET: u32 = 1_024;

/// Token budget for `level`, clamped to the profile's range and below
/// `max_tokens`; `None` when off or when no valid budget fits.
fn anthropic_budget(profile: &ModelProfile, level: ThinkingLevel, max_tokens: u32) -> Option<u32> {
    if level == ThinkingLevel::Off {
        return None;
    }
    let mut budget = level.budget_tokens();
    if let Reasoning::Budget { min, max } = profile.reasoning {
        budget = budget.clamp(min, max.max(min));
    }
    let budget = budget.min(max_tokens.saturating_sub(1));
    (budget >= MIN_BUDGET).then_some(budget)
}

/// Converts via the shortest decimal form so `0.2f32` is sent as `0.2`.
fn f32_json(x: f32) -> Value {
    x.to_string()
        .parse::<f64>()
        .map_or(Value::Null, Value::from)
}

fn parse_response(kind: ProviderKind, endpoint: &str, v: &Value) -> Result<ChatResponse> {
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| err.to_string(), str::to_string);
        return Err(Error::decode(endpoint, format!("服务返回错误：{msg}")));
    }

    let (content, reasoning, finish_reason, usage) = if kind == ProviderKind::Anthropic {
        let blocks = v
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::decode(endpoint, "缺少 content"))?;
        let collect = |ty: &str, field: &str| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some(ty))
                .filter_map(|b| b.get(field).and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        };
        let usage = v.get("usage").map(|u| Usage {
            prompt_tokens: u32_at(u, "input_tokens"),
            completion_tokens: u32_at(u, "output_tokens"),
        });
        (
            collect("text", "text"),
            Some(collect("thinking", "thinking")),
            str_at(v, "stop_reason"),
            usage,
        )
    } else {
        let choice = v
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| Error::decode(endpoint, "缺少 choices"))?;
        let msg = choice.get("message").unwrap_or(&Value::Null);
        let reasoning = str_at(msg, "reasoning_content").or_else(|| str_at(msg, "reasoning"));
        let usage = v.get("usage").filter(|u| u.is_object()).map(|u| Usage {
            prompt_tokens: u32_at(u, "prompt_tokens"),
            completion_tokens: u32_at(u, "completion_tokens"),
        });
        (
            content_text(msg.get("content")),
            reasoning,
            str_at(choice, "finish_reason"),
            usage,
        )
    };

    let (think, content) = split_think(&content);
    let reasoning = [reasoning, think]
        .into_iter()
        .flatten()
        .filter(|r| !r.trim().is_empty())
        .collect::<Vec<_>>();
    Ok(ChatResponse {
        content,
        reasoning: (!reasoning.is_empty()).then(|| reasoning.join("\n\n")),
        finish_reason,
        usage,
        citations: citations(kind, v),
    })
}

/// Content as a string, or the concatenated text parts of a content array.
fn content_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str).or_else(|| p.as_str()))
            .collect(),
        _ => String::new(),
    }
}

/// Splits a leading `<think>…</think>` block (or text before an orphan
/// `</think>`, as some templates omit the opening tag) from the answer.
pub(crate) fn split_think(content: &str) -> (Option<String>, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let trimmed = content.trim_start();
    let (think, rest) = if let Some(inner) = trimmed.strip_prefix(OPEN) {
        match inner.find(CLOSE) {
            Some(i) => (&inner[..i], &inner[i + CLOSE.len()..]),
            None => (inner, ""),
        }
    } else if let Some(i) = content.find(CLOSE)
        && !content[..i].contains(OPEN)
    {
        (&content[..i], &content[i + CLOSE.len()..])
    } else {
        return (None, content.to_string());
    };
    let think = think.trim();
    (
        (!think.is_empty()).then(|| think.to_string()),
        rest.trim_start().to_string(),
    )
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn u32_at(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(Value::as_u64)
        .map_or(0, |n| n.min(u32::MAX as u64) as u32)
}
