//! Pre-review: predicts the comments a reviewer would likely make on a
//! stretch of the document, from their distilled profile and past comments,
//! so the author can fix those spots before sending the report out.

use models::{ChatRequest, Message};
use serde::Serialize;
use serde_json::Value;

use crate::core::{Core, RoleName};
use crate::error::{Error, Result};
use crate::fix::judge::CATEGORIES;
use crate::fix::prompt::estimate_tokens;
use crate::profiles;

/// Most items kept from one model call.
const MAX_ITEMS: usize = 15;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreReviewItem {
    pub paragraph_index: usize,
    /// The words in the paragraph the comment is about; empty when the model
    /// quoted something that is not in the paragraph.
    pub quote: String,
    pub comment: String,
    pub category: Option<String>,
}

const SYSTEM: &str = "你在模拟一位审稿专家预审报告。根据这位专家的审稿画像和他以往写过的批注，找出下面段落中他最可能提出批注的地方，并按他的口吻写出批注。

要求：
1. 只挑他真正会在意的问题，宁缺毋滥；没有问题就返回空数组。
2. quote 必须从段落原文中逐字摘录（不超过 40 字），用来定位问题。
3. comment 写成他会写的批注，具体指出问题和修改方向。
4. category 从 数据口径、政策依据、措辞规范、格式、逻辑结构、其他 中选一个。

只输出一个 JSON 对象：
{\"items\": [{\"paragraph\": 段落编号, \"quote\": \"原文摘录\", \"comment\": \"批注内容\", \"category\": \"数据口径\"}]}";

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Reads the model's answer, keeping only items that point at one of the
/// given paragraphs.
pub fn parse(text: &str, paragraphs: &[(usize, String)]) -> Result<Vec<PreReviewItem>> {
    let json = text
        .find('{')
        .zip(text.rfind('}'))
        .filter(|(a, b)| a < b)
        .and_then(|(a, b)| serde_json::from_str::<Value>(&text[a..=b]).ok())
        .ok_or_else(|| Error::Invalid("模型没有返回预审 JSON".into()))?;
    let items = json
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let Some(index) = item
            .get("paragraph")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
            .map(|n| n as usize)
        else {
            continue;
        };
        let Some((_, para)) = paragraphs.iter().find(|(i, _)| *i == index) else {
            continue;
        };
        let comment = item
            .get("comment")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if comment.is_empty() {
            continue;
        }
        let quote = item
            .get("quote")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let quote = if !quote.is_empty() && para.contains(&quote) {
            quote
        } else {
            String::new()
        };
        let category = item
            .get("category")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| CATEGORIES.iter().any(|(name, _)| name == c))
            .map(str::to_string);
        out.push(PreReviewItem {
            paragraph_index: index,
            quote,
            comment,
            category,
        });
        if out.len() == MAX_ITEMS {
            break;
        }
    }
    out.sort_by_key(|i| i.paragraph_index);
    Ok(out)
}

/// Most model calls one pre-review makes; each covers what fits the budget.
const MAX_BATCHES: usize = 8;

impl Core {
    /// Predicts the reviewer's comments on `paragraphs` (document index and
    /// editable text), in as many model calls as the context window needs.
    pub async fn pre_review(
        &self,
        reviewer_id: i64,
        paragraphs: &[(usize, String)],
    ) -> Result<Vec<PreReviewItem>> {
        let chat = self.require(RoleName::Chat)?;
        let (reviewer, profile, cases) = {
            let store = self.store();
            let reviewer = store
                .reviewer(reviewer_id)?
                .ok_or_else(|| Error::Invalid("审稿人不存在".into()))?;
            (
                reviewer,
                profiles::prompt_text(&store, reviewer_id)?,
                store.reviewer_cases(reviewer_id, 30)?,
            )
        };
        if profile.is_none() && cases.is_empty() {
            return Err(Error::Invalid(format!(
                "还不了解「{}」的审稿习惯：先用 AI 修复处理几条他的批注，或在审稿人页面提炼画像",
                reviewer.name
            )));
        }
        let mut head = format!("【审稿专家】{}\n", reviewer.name);
        if !reviewer.note.is_empty() {
            head.push_str(&format!("【备注】{}\n", reviewer.note));
        }
        if let Some(p) = &profile {
            head.push_str(&format!("\n【审稿画像】\n{p}\n"));
        }
        if !cases.is_empty() {
            head.push_str("\n【他以往写过的批注】\n");
            for c in cases.iter().take(20) {
                head.push_str(&format!("- {}\n", clip(&c.comment, 120)));
            }
        }
        head.push_str("\n【待预审段落】（方括号内是段落编号）\n");
        let budget = (chat.profile.context_window as usize * 6 / 10)
            .saturating_sub(estimate_tokens(SYSTEM) + estimate_tokens(&head) + 4096)
            .max(1_000);

        let todo: Vec<&(usize, String)> = paragraphs
            .iter()
            .filter(|(_, t)| !t.trim().is_empty())
            .collect();
        let mut out = Vec::new();
        let mut next = 0;
        for _ in 0..MAX_BATCHES {
            if next >= todo.len() {
                break;
            }
            let mut user = head.clone();
            let mut batch = Vec::new();
            let mut used = 0;
            while let Some((index, text)) = todo.get(next) {
                let line = format!("[{index}] {}\n", clip(text, 3_000));
                let cost = estimate_tokens(&line);
                if used + cost > budget && !batch.is_empty() {
                    break;
                }
                used += cost;
                user.push_str(&line);
                batch.push((*index, text.clone()));
                next += 1;
            }
            let mut req = ChatRequest::new(
                chat.model.clone(),
                vec![Message::system(SYSTEM), Message::user(user)],
            );
            req.profile = chat.profile.clone();
            req.thinking = chat.thinking;
            req.json_output = true;
            req.temperature = Some(0.3);
            req.max_tokens = Some(chat.profile.max_output_tokens.clamp(1024, 8192));
            let response = self
                .client()
                .chat(&chat.provider, &req)
                .await
                .map_err(|e| Error::Invalid(format!("大语言模型调用失败：{e}")))?;
            out.extend(parse(&response.content, &batch)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_items_on_given_paragraphs() {
        let paragraphs = vec![
            (3, "项目总投资约1.2亿元，建设周期两年。".to_string()),
            (4, "系统采用云原生架构。".to_string()),
        ];
        let answer = r#"好的：```json
{"items": [
  {"paragraph": 4, "quote": "云原生架构", "comment": "请说明架构选型依据", "category": "逻辑结构"},
  {"paragraph": "3", "quote": "约1.2亿元", "comment": "请注明投资测算口径", "category": "数据"},
  {"paragraph": 9, "quote": "x", "comment": "不在范围内"},
  {"paragraph": 3, "quote": "不存在的文字", "comment": "引文对不上也保留批注"},
  {"paragraph": 3, "comment": ""}
]}
```"#;
        let items = parse(answer, &paragraphs).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].paragraph_index, 3);
        assert_eq!(items[0].quote, "约1.2亿元");
        assert_eq!(items[0].category, None, "unknown categories are dropped");
        assert_eq!(
            items[1].quote, "",
            "a quote not in the paragraph is cleared"
        );
        assert_eq!(items[2].category.as_deref(), Some("逻辑结构"));
        assert!(parse("没有 JSON", &paragraphs).is_err());
    }
}
