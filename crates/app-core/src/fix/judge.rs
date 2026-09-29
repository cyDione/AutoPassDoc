//! Scoring a proposed fix with the decision model.
//!
//! Three yes/no questions must each pass on their own (they are "hard"): the
//! fix answers the comment, keeps the original meaning, and invents nothing.
//! A 1–5 style rating adds a little weight. Confidence is the weighted mean;
//! one-click apply needs both the confidence and every hard item to reach
//! the threshold. A choice question classifies the comment for the reviewer
//! profile.

use models::{Answer, AnswerValue, Question};
use serde::Serialize;

use super::context::{FixInput, FixMode};
use super::prompt::Passage;

pub const CATEGORIES: &[(&str, &str)] = &[
    ("数据口径", "数字、统计口径、单位、测算依据方面的问题"),
    ("政策依据", "政策文件、法规依据、上级要求方面的问题"),
    ("措辞规范", "用词、语气、表述是否规范准确"),
    ("格式", "标点、编号、格式体例方面的问题"),
    ("逻辑结构", "论证逻辑、层次结构、前后一致性方面的问题"),
    ("其他", "不属于以上各类"),
];

const STYLE_LEVELS: &[&str] = &["很不规范", "不太规范", "基本规范", "比较规范", "非常规范"];
/// Expected rating 3.5 of 5.
const STYLE_PASS: f32 = 0.625;

struct Item {
    key: &'static str,
    label: &'static str,
    weight: f32,
    hard: bool,
}

const ITEMS: &[Item] = &[
    Item {
        key: "addresses",
        label: "回应批注",
        weight: 0.4,
        hard: true,
    },
    Item {
        key: "meaning",
        label: "保持原意",
        weight: 0.25,
        hard: true,
    },
    Item {
        key: "grounded",
        label: "有据可查",
        weight: 0.25,
        hard: true,
    },
    Item {
        key: "style",
        label: "公文规范",
        weight: 0.1,
        hard: false,
    },
];

pub fn questions() -> Vec<Question> {
    vec![
        Question::yes_no(
            "addresses",
            "修改后的文字是否切实回应并解决了审稿专家在批注中提出的问题？",
        ),
        Question::yes_no(
            "meaning",
            "修改是否保持了原文的核心意思和关键信息（没有歪曲原意，没有删掉批注未要求删除的重要内容）？",
        ),
        Question::yes_no(
            "grounded",
            "修改中新增的事实、数据、文件名称或文号，是否都能在原文、批注或参考资料中找到依据（没有凭空编造）？用“【待补充：…】”标出的待补充内容不算编造。",
        ),
        Question::score(
            "style",
            "修改后的表述是否符合党政机关公文和正式报告的写作规范（准确、简明、庄重、用语规范）？",
            STYLE_LEVELS.iter().copied(),
        ),
        Question::choice(
            "category",
            "这条批注主要指出的是哪一类问题？",
            CATEGORIES.iter().copied(),
        ),
    ]
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// The situation the decision model judges.
pub fn state(
    input: &FixInput,
    new_paragraphs: &[String],
    explanation: &str,
    passages: &[Passage],
) -> String {
    let mut s = format!("【审稿专家批注】\n{}\n\n", input.comment.trim());
    if !input.quote.is_empty() {
        s.push_str(&format!("【批注所指原文】\n{}\n\n", input.quote));
    }
    s.push_str("【修改前】\n");
    for (i, (_, old)) in input.paragraphs.iter().enumerate() {
        s.push_str(&format!("（第{}段）{old}\n", i + 1));
    }
    s.push_str("\n【修改后】\n");
    for (i, new) in new_paragraphs.iter().enumerate() {
        s.push_str(&format!("（第{}段）{new}\n", i + 1));
    }
    if !explanation.is_empty() {
        s.push_str(&format!("\n【修改说明】\n{explanation}\n"));
    }
    if !passages.is_empty() {
        s.push_str("\n【参考资料】\n");
        for p in passages {
            s.push_str(&format!(
                "[{}] 《{}》{}\n",
                p.n,
                p.title,
                clip(&p.text, 400)
            ));
        }
    }
    s
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeItem {
    pub key: &'static str,
    pub label: &'static str,
    /// Probability of the good outcome.
    pub value: f32,
    pub weight: f32,
    pub hard: bool,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Judgement {
    pub confidence: f32,
    pub threshold: f32,
    pub passed: bool,
    pub items: Vec<JudgeItem>,
    pub category: Option<String>,
    /// "jev" | "chat"
    pub backend: &'static str,
    pub model: String,
}

/// In [`FixMode::Rewrite`] the meaning may change on purpose, so "保持原意"
/// only weighs in and does not have to pass on its own.
pub fn score(
    answers: &[Answer],
    mode: FixMode,
    threshold: f32,
    backend: &'static str,
    model: &str,
) -> Result<Judgement, String> {
    let find = |key: &str| answers.iter().find(|a| a.key == key).map(|a| &a.value);
    let mut items = Vec::new();
    let mut confidence = 0.0;
    for item in ITEMS {
        let value = match find(item.key) {
            Some(AnswerValue::YesNo { p_yes }) => *p_yes,
            Some(AnswerValue::Score { normalized, .. }) => *normalized,
            Some(AnswerValue::Choice { confidence, .. }) => *confidence,
            None => return Err(format!("决策模型没有回答「{}」", item.label)),
        }
        .clamp(0.0, 1.0);
        let hard = item.hard && !(mode == FixMode::Rewrite && item.key == "meaning");
        let passed = if hard {
            value >= threshold
        } else {
            value >= STYLE_PASS
        };
        confidence += item.weight * value;
        items.push(JudgeItem {
            key: item.key,
            label: item.label,
            value,
            weight: item.weight,
            hard,
            passed,
        });
    }
    let category = match find("category") {
        Some(AnswerValue::Choice { choice, .. }) => Some(choice.clone()),
        _ => None,
    };
    let passed = confidence >= threshold && items.iter().all(|i| !i.hard || i.passed);
    Ok(Judgement {
        confidence,
        threshold,
        passed,
        items,
        category,
        backend,
        model: model.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yes(key: &str, p: f32) -> Answer {
        Answer {
            key: key.into(),
            value: AnswerValue::YesNo { p_yes: p },
        }
    }

    fn answers(addresses: f32, grounded: f32) -> Vec<Answer> {
        vec![
            yes("addresses", addresses),
            yes("meaning", 0.95),
            yes("grounded", grounded),
            Answer {
                key: "style".into(),
                value: AnswerValue::Score {
                    normalized: 0.75,
                    expected_level: 3.0,
                    probabilities: vec![],
                },
            },
            Answer {
                key: "category".into(),
                value: AnswerValue::Choice {
                    choice: "数据口径".into(),
                    probabilities: vec![],
                    confidence: 0.8,
                },
            },
        ]
    }

    #[test]
    fn weighs_items_and_enforces_hard_ones() {
        let j = score(&answers(0.9, 0.9), FixMode::Fix, 0.8, "jev", "jev-latest").unwrap();
        assert!((j.confidence - (0.4 * 0.9 + 0.25 * 0.95 + 0.25 * 0.9 + 0.1 * 0.75)).abs() < 1e-5);
        assert!(j.passed);
        assert_eq!(j.category.as_deref(), Some("数据口径"));

        let j = score(&answers(0.99, 0.7), FixMode::Fix, 0.8, "jev", "jev-latest").unwrap();
        assert!(j.confidence >= 0.8, "overall confidence is high");
        assert!(!j.passed, "but an invented fact blocks one-click apply");
        assert!(!j.items.iter().find(|i| i.key == "grounded").unwrap().passed);

        assert!(score(&answers(0.9, 0.9)[..2], FixMode::Fix, 0.8, "jev", "m").is_err());
    }

    #[test]
    fn rewrites_may_change_the_meaning() {
        let mut a = answers(0.95, 0.95);
        a[1] = yes("meaning", 0.3);
        assert!(!score(&a, FixMode::Fix, 0.7, "jev", "m").unwrap().passed);
        let j = score(&a, FixMode::Rewrite, 0.7, "jev", "m").unwrap();
        assert!(j.passed);
        assert!(!j.items.iter().find(|i| i.key == "meaning").unwrap().hard);
    }

    #[test]
    fn question_keys_match_items() {
        let keys: Vec<String> = questions().into_iter().map(|q| q.key).collect();
        for item in ITEMS {
            assert!(keys.iter().any(|k| k == item.key));
        }
        assert!((ITEMS.iter().map(|i| i.weight).sum::<f32>() - 1.0).abs() < 1e-6);
    }
}
