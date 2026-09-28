//! BYOK model adapters for AutoPassDoc: chat LLM, decision model (Jev),
//! embedding and reranker, over OpenAI-compatible, OpenRouter, Anthropic and
//! Ollama APIs.
//!
//! Everything goes through one async [`Client`]; a [`Provider`] carries the
//! endpoint and the caller-supplied API key (never stored or logged here).
//!
//! ```no_run
//! # async fn demo() -> models::Result<()> {
//! use models::{ChatRequest, Client, Message, Provider, ProviderKind, ThinkingLevel};
//!
//! let client = Client::new()?;
//! let provider = Provider::new(ProviderKind::OpenAiCompatible, "https://api.example.com")
//!     .with_api_key("sk-...");
//! let mut req = ChatRequest::new("deepseek-chat", vec![Message::user("你好")]);
//! req.thinking = Some(ThinkingLevel::Off);
//! let resp = client.chat(&provider, &req).await?;
//! println!("{}", resp.content);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

mod chat;
mod client;
mod decide;
mod embed;
mod error;
mod list;
mod probe;
mod profile;
mod provider;

pub use chat::{ChatRequest, ChatResponse, Message, Role, Usage};
pub use client::{ANTHROPIC_VERSION, Client, ClientConfig};
pub use decide::{Answer, AnswerValue, Question, QuestionKind};
pub use error::{Error, Result};
pub use list::ModelInfo;
pub use probe::ProbeReport;
pub use profile::{
    DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_OUTPUT_TOKENS, ModelProfile, PartialProfile, ProfileSource,
    Reasoning, ThinkingLevel, ThinkingParam, available_levels, builtin_profile, resolve_profile,
};
pub use provider::{
    DEFAULT_DECISION_PATH, DEFAULT_RERANK_PATH, ModelRole, Provider, ProviderKind, guess_role,
    normalize_base_url,
};
