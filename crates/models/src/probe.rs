//! "Test connection": one tiny call per role.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::chat::{ChatRequest, Message};
use crate::client::Client;
use crate::decide::{AnswerValue, Question};
use crate::error::{Result, truncate};
use crate::profile::ThinkingLevel;
use crate::provider::{ModelRole, Provider};

/// Outcome of a successful [`Client::probe`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    /// Role tested.
    pub role: ModelRole,
    /// Model tested.
    pub model: String,
    /// Round-trip time of the call.
    pub latency_ms: u64,
    /// Short human-readable result, e.g. `向量维度 1024`.
    pub summary: String,
}

impl Client {
    /// Makes one minimal call for `role` and reports latency and a summary.
    pub async fn probe(&self, p: &Provider, role: ModelRole, model: &str) -> Result<ProbeReport> {
        let start = Instant::now();
        let summary = match role {
            ModelRole::Chat => {
                let mut req = ChatRequest::new(model, vec![Message::user("请只回复一个字：好")]);
                req.max_tokens = Some(64);
                req.thinking = Some(ThinkingLevel::Off);
                let resp = self.chat(p, &req).await?;
                let content = resp.content.trim();
                if content.is_empty() {
                    let reason = resp.finish_reason.as_deref().unwrap_or("未知");
                    format!("已连通，但回复为空（finish_reason：{reason}）")
                } else {
                    format!("回复：{}", truncate(content, 40))
                }
            }
            ModelRole::Decision => {
                let q = Question::yes_no("probe", "这段文字是中文吗？");
                let answers = self
                    .decide(p, model, "今天天气晴朗，适合出行。", &[q])
                    .await?;
                match answers.first().map(|a| &a.value) {
                    Some(AnswerValue::YesNo { p_yes }) => format!("P(是) = {p_yes:.3}"),
                    _ => "已连通".into(),
                }
            }
            ModelRole::Embedding => {
                let vectors = self.embed(p, model, &["连接测试".to_string()]).await?;
                format!("向量维度 {}", vectors[0].len())
            }
            ModelRole::Rerank => {
                let docs = ["苹果是一种水果".to_string(), "汽车需要加油".to_string()];
                let scores = self.rerank(p, model, "苹果", &docs).await?;
                let scores: Vec<String> = scores.iter().map(|s| format!("{s:.3}")).collect();
                format!("得分 [{}]", scores.join(", "))
            }
        };
        Ok(ProbeReport {
            role,
            model: model.to_string(),
            latency_ms: start.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            summary,
        })
    }
}
