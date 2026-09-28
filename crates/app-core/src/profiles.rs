//! Reviewer profiles: what each reviewer keeps asking for, distilled by the
//! chat model from the fixes the user accepted, edited or rejected, plus
//! the reviewer's most similar past fixes used as examples.

use std::collections::HashSet;

use models::{ChatRequest, Message};
use serde::{Deserialize, Serialize};

use crate::core::{Core, RoleName};
use crate::error::{Error, Result};
use crate::fix::prompt::Example;
use crate::store::{CaseAction, Profile, Store};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileSummary {
    pub summary: String,
    pub focus: Vec<String>,
    pub preferences: Vec<String>,
    pub common_requests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub reviewer_id: i64,
    pub version: i64,
    pub created_at: i64,
    pub case_count: usize,
    #[serde(flatten)]
    pub content: ProfileSummary,
}

impl From<&Profile> for ProfileView {
    fn from(p: &Profile) -> Self {
        Self {
            reviewer_id: p.reviewer_id,
            version: p.version,
            created_at: p.created_at,
            case_count: p.case_count,
            content: serde_json::from_str(&p.summary).unwrap_or_default(),
        }
    }
}

pub fn view(store: &Store, reviewer_id: i64) -> Result<Option<ProfileView>> {
    Ok(store
        .latest_profile(reviewer_id)?
        .as_ref()
        .map(ProfileView::from))
}

/// The profile as plain text for the fix prompt.
pub fn prompt_text(store: &Store, reviewer_id: i64) -> Result<Option<String>> {
    let Some(p) = view(store, reviewer_id)? else {
        return Ok(None);
    };
    let c = p.content;
    let mut out = String::new();
    if !c.summary.is_empty() {
        out.push_str(&c.summary);
        out.push('\n');
    }
    for (title, items) in [
        ("关注点", &c.focus),
        ("表述偏好", &c.preferences),
        ("常见要求", &c.common_requests),
    ] {
        if !items.is_empty() {
            out.push_str(&format!("{title}：{}\n", items.join("；")));
        }
    }
    Ok((!out.trim().is_empty()).then_some(out))
}

fn bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

fn similarity(a: &HashSet<(char, char)>, b: &HashSet<(char, char)>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let shared = a.intersection(b).count() as f32;
    shared / (a.len() + b.len()) as f32 * 2.0
}

/// The reviewer's accepted fixes whose comments look most like `comment`.
pub fn examples(store: &Store, reviewer_id: i64, comment: &str, k: usize) -> Result<Vec<Example>> {
    let target = bigrams(comment);
    let mut scored: Vec<(f32, Example)> = store
        .reviewer_cases(reviewer_id, 200)?
        .into_iter()
        .filter(|c| matches!(c.action, CaseAction::Accepted | CaseAction::Edited))
        .filter_map(|c| {
            let revised = c.final_text.clone()?;
            Some((
                similarity(&target, &bigrams(&c.comment)),
                Example {
                    comment: c.comment,
                    original: c.original,
                    revised,
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(scored.into_iter().take(k).map(|(_, e)| e).collect())
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

const DISTILL_SYSTEM: &str = "你是报告评审方面的分析助手。根据一位审稿专家的历史批注，以及作者对 AI 修改建议的处理结果，总结这位专家的审稿风格，帮助下次按他的要求一次改到位。

只输出一个 JSON 对象：
{\"summary\": \"一段话概括这位专家的审稿风格\", \"focus\": [\"最关注的问题，按重要性排序\"], \"preferences\": [\"偏好的表述方式和用词\"], \"commonRequests\": [\"经常提出的具体要求\"]}
每个数组 3～6 条，每条不超过 40 字；只写从材料中能看出来的内容。";

impl Core {
    /// Re-distils a reviewer's profile from their decided cases.
    pub async fn distill_profile(&self, reviewer_id: i64) -> Result<ProfileView> {
        let chat = self.require(RoleName::Chat)?;
        let (reviewer, previous, cases) = {
            let store = self.store();
            let reviewer = store
                .reviewer(reviewer_id)?
                .ok_or_else(|| Error::Invalid("审稿人不存在".into()))?;
            (
                reviewer,
                prompt_text(&store, reviewer_id)?,
                store.reviewer_cases(reviewer_id, 40)?,
            )
        };
        if cases.is_empty() {
            return Err(Error::Invalid(
                "这位审稿人还没有已处理的修改记录，处理几条批注后再提炼".into(),
            ));
        }
        let mut user = format!("【审稿专家】{}\n", reviewer.name);
        if !reviewer.note.is_empty() {
            user.push_str(&format!("【备注】{}\n", reviewer.note));
        }
        if let Some(p) = previous {
            user.push_str(&format!("\n【上一版画像】\n{p}\n"));
        }
        user.push_str("\n【历史批注与处理结果】\n");
        for (i, c) in cases.iter().enumerate() {
            let outcome = match c.action {
                CaseAction::Accepted => "采纳 AI 修改",
                CaseAction::Edited => "修改后采纳",
                CaseAction::Rejected => "拒绝 AI 修改",
                CaseAction::Pending => "未处理",
            };
            user.push_str(&format!(
                "{}. [{}] 批注：{}\n   原文：{}\n   结果：{outcome}{}\n",
                i + 1,
                c.category.as_deref().unwrap_or("未分类"),
                clip(&c.comment, 200),
                clip(&c.original, 150),
                c.final_text
                    .as_deref()
                    .map(|f| format!("；最终文字：{}", clip(f, 150)))
                    .unwrap_or_default(),
            ));
        }
        let mut req = ChatRequest::new(
            chat.model.clone(),
            vec![Message::system(DISTILL_SYSTEM), Message::user(user)],
        );
        req.profile = chat.profile.clone();
        req.thinking = chat.thinking;
        req.json_output = true;
        req.temperature = Some(0.2);
        req.max_tokens = Some(chat.profile.max_output_tokens.min(4096));
        let response = self
            .client()
            .chat(&chat.provider, &req)
            .await
            .map_err(|e| Error::Invalid(format!("大语言模型调用失败：{e}")))?;
        let text = &response.content;
        let json = text
            .find('{')
            .zip(text.rfind('}'))
            .filter(|(a, b)| a < b)
            .map(|(a, b)| &text[a..=b])
            .ok_or_else(|| Error::Invalid("模型没有返回画像 JSON".into()))?;
        let summary: ProfileSummary =
            serde_json::from_str(json).map_err(|e| Error::Invalid(format!("画像格式不对：{e}")))?;
        let profile = self.store().add_profile(
            reviewer_id,
            &serde_json::to_string(&summary)?,
            cases.len(),
        )?;
        Ok(ProfileView::from(&profile))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Case;

    #[test]
    fn picks_similar_accepted_cases() {
        let store = Store::open_in_memory().unwrap();
        let r = store.create_reviewer("张处长", "").unwrap();
        for (comment, action) in [
            ("数据来源不明确，请注明统计口径", CaseAction::Accepted),
            ("表述不规范", CaseAction::Edited),
            ("数据口径与上年不一致", CaseAction::Rejected),
            ("请补充数据来源", CaseAction::Accepted),
        ] {
            let id = store
                .add_case(&Case {
                    id: 0,
                    reviewer_id: Some(r.id),
                    doc_key: "d".into(),
                    doc_name: "d".into(),
                    comment_id: "1".into(),
                    author: "张".into(),
                    comment: comment.into(),
                    original: "原文".into(),
                    suggestion: "建议".into(),
                    final_text: None,
                    action: CaseAction::Pending,
                    confidence: None,
                    judge: None,
                    category: None,
                    created_at: 0,
                })
                .unwrap();
            store.set_case_outcome(id, action, Some("最终")).unwrap();
        }
        let ex = examples(&store, r.id, "这里的数据来源是什么", 2).unwrap();
        assert_eq!(ex.len(), 2);
        assert!(ex[0].comment.contains("数据来源"));
        assert!(
            ex.iter().all(|e| !e.comment.contains("上年")),
            "rejected cases are not examples"
        );

        assert!(prompt_text(&store, r.id).unwrap().is_none());
        let summary = ProfileSummary {
            summary: "重视数据".into(),
            focus: vec!["数据来源".into()],
            ..Default::default()
        };
        store
            .add_profile(r.id, &serde_json::to_string(&summary).unwrap(), 4)
            .unwrap();
        let text = prompt_text(&store, r.id).unwrap().unwrap();
        assert!(text.contains("重视数据") && text.contains("关注点：数据来源"));
    }
}
