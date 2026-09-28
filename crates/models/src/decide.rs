//! Decision model: Jev's raw "systemone" API, and a fallback judge built on a
//! chat model that answers in the same shape.

use std::collections::HashSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::chat::{ChatRequest, Message};
use crate::client::{Client, endpoint_label};
use crate::error::{Error, Result, truncate};
use crate::profile::ModelProfile;
use crate::provider::Provider;

/// A question for the decision model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// Unique key; answers are matched by it.
    pub key: String,
    /// Answer type and its criteria.
    pub kind: QuestionKind,
    /// What to judge.
    pub instructions: String,
}

/// Answer type of a [`Question`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Yes/no; answered with the probability of yes (Jev `noul`).
    YesNo,
    /// Pick one option: `(label, description)` pairs.
    Choice(Vec<(String, String)>),
    /// Rate on levels ordered lowest → highest.
    Score(Vec<String>),
}

impl Question {
    /// A yes/no question.
    pub fn yes_no(key: impl Into<String>, instructions: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            kind: QuestionKind::YesNo,
            instructions: instructions.into(),
        }
    }

    /// A multiple-choice question from `(label, description)` pairs.
    pub fn choice<L: Into<String>, D: Into<String>>(
        key: impl Into<String>,
        instructions: impl Into<String>,
        options: impl IntoIterator<Item = (L, D)>,
    ) -> Self {
        Self {
            key: key.into(),
            kind: QuestionKind::Choice(
                options
                    .into_iter()
                    .map(|(l, d)| (l.into(), d.into()))
                    .collect(),
            ),
            instructions: instructions.into(),
        }
    }

    /// A score question with levels ordered lowest → highest.
    pub fn score<S: Into<String>>(
        key: impl Into<String>,
        instructions: impl Into<String>,
        levels: impl IntoIterator<Item = S>,
    ) -> Self {
        Self {
            key: key.into(),
            kind: QuestionKind::Score(levels.into_iter().map(Into::into).collect()),
            instructions: instructions.into(),
        }
    }
}

/// The answer to one [`Question`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Key of the question answered.
    pub key: String,
    /// The answer.
    pub value: AnswerValue,
}

/// An answer, with probabilities normalized to `0..=1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnswerValue {
    /// Probability that the answer is yes.
    YesNo {
        /// P(yes).
        p_yes: f32,
    },
    /// The chosen label and the distribution over all labels (question order).
    Choice {
        /// Chosen label.
        choice: String,
        /// `(label, probability)` for every option, summing to 1.
        probabilities: Vec<(String, f32)>,
        /// Confidence in `choice`.
        confidence: f32,
    },
    /// A rating.
    Score {
        /// Score in `0..=1` (0 = lowest level, 1 = highest).
        normalized: f32,
        /// Expected level index (0-based, may be fractional).
        expected_level: f32,
        /// Probability per level (lowest → highest); empty when not reported.
        probabilities: Vec<f32>,
    },
}

impl Client {
    /// Asks Jev (`POST {decision_path}`, default `systemone`) to answer all
    /// `questions` about `state`.
    pub async fn decide(
        &self,
        p: &Provider,
        model: &str,
        state: &str,
        questions: &[Question],
    ) -> Result<Vec<Answer>> {
        p.require_openai_style("决策（Jev）")?;
        validate(questions)?;
        if questions.is_empty() {
            return Ok(Vec::new());
        }
        let url = p.openai_url(p.decision_path())?;
        let body = JevRequest {
            model,
            state,
            questions: WireQuestions(questions),
        };
        let v = self.post_json(p, &url, &body).await?;
        parse_answers(&v, questions, &endpoint_label("POST", &url))
    }

    /// Fallback judge: asks a chat model to answer every question at once as
    /// strict JSON in Jev's answer shapes, then parses (tolerating code fences)
    /// and normalizes the probabilities.
    pub async fn decide_via_chat(
        &self,
        p: &Provider,
        model: &str,
        profile: &ModelProfile,
        state: &str,
        questions: &[Question],
    ) -> Result<Vec<Answer>> {
        validate(questions)?;
        if questions.is_empty() {
            return Ok(Vec::new());
        }
        let mut req = ChatRequest::new(
            model,
            vec![
                Message::system(JUDGE_SYSTEM),
                Message::user(judge_prompt(state, questions)),
            ],
        );
        req.profile = profile.clone();
        req.json_output = true;
        let resp = self.chat(p, &req).await?;
        let label = "判定模型输出";
        let v = extract_json(&resp.content).ok_or_else(|| {
            Error::decode(
                label,
                format!("不是有效的 JSON：{}", truncate(resp.content.trim(), 200)),
            )
        })?;
        parse_answers(&v, questions, label)
    }
}

fn validate(questions: &[Question]) -> Result<()> {
    let mut seen = HashSet::new();
    for q in questions {
        let invalid = |why: &str| Err(Error::InvalidConfig(format!("问题“{}”{why}", q.key)));
        if q.key.trim().is_empty() {
            return Err(Error::InvalidConfig("问题的 key 不能为空".into()));
        }
        if !seen.insert(q.key.as_str()) {
            return invalid("的 key 重复");
        }
        match &q.kind {
            QuestionKind::Choice(opts) if opts.is_empty() => return invalid("没有选项"),
            QuestionKind::Choice(opts)
                if opts.iter().map(|(l, _)| l).collect::<HashSet<_>>().len() != opts.len() =>
            {
                return invalid("的选项标签重复");
            }
            QuestionKind::Score(levels) if levels.len() < 2 => return invalid("至少需要两个等级"),
            _ => {}
        }
    }
    Ok(())
}

// Hand-written serialization keeps question and option order (serde_json's
// `Value` map would sort keys).

#[derive(Serialize)]
struct JevRequest<'a> {
    model: &'a str,
    state: &'a str,
    questions: WireQuestions<'a>,
}

struct WireQuestions<'a>(&'a [Question]);

impl Serialize for WireQuestions<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|q| (&q.key, WireQuestion::from(q))))
    }
}

#[derive(Serialize)]
struct WireQuestion<'a> {
    #[serde(rename = "type")]
    ty: &'static str,
    instructions: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    criteria: Option<Criteria<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Criteria<'a> {
    Choice(Options<'a>),
    Score(&'a [String]),
}

struct Options<'a>(&'a [(String, String)]);

impl Serialize for Options<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(label, desc)| (label, desc)))
    }
}

impl<'a> From<&'a Question> for WireQuestion<'a> {
    fn from(q: &'a Question) -> Self {
        let (ty, criteria) = match &q.kind {
            QuestionKind::YesNo => ("noul", None),
            QuestionKind::Choice(opts) => ("choice", Some(Criteria::Choice(Options(opts)))),
            QuestionKind::Score(levels) => ("score", Some(Criteria::Score(levels))),
        };
        Self {
            ty,
            instructions: &q.instructions,
            criteria,
        }
    }
}

/// Parses `{"answers": {key: {...}}}` (or the bare map, or an array of
/// answers carrying `key`) into one [`Answer`] per question, in question order.
pub(crate) fn parse_answers(
    v: &Value,
    questions: &[Question],
    endpoint: &str,
) -> Result<Vec<Answer>> {
    let answers = v.get("answers").unwrap_or(v);
    let lookup = |key: &str| match answers {
        Value::Object(m) => m.get(key),
        Value::Array(items) => items.iter().find(|a| {
            ["key", "id"]
                .iter()
                .any(|k| a.get(k).and_then(Value::as_str) == Some(key))
        }),
        _ => None,
    };
    questions
        .iter()
        .map(|q| {
            let a = lookup(&q.key)
                .ok_or_else(|| Error::decode(endpoint, format!("缺少问题“{}”的答案", q.key)))?;
            let value = parse_value(&q.kind, a).ok_or_else(|| {
                let raw = truncate(&a.to_string(), 200);
                Error::decode(endpoint, format!("问题“{}”的答案无法识别：{raw}", q.key))
            })?;
            Ok(Answer {
                key: q.key.clone(),
                value,
            })
        })
        .collect()
}

fn parse_value(kind: &QuestionKind, a: &Value) -> Option<AnswerValue> {
    match kind {
        QuestionKind::YesNo => parse_yes_no(a).map(|p| AnswerValue::YesNo {
            p_yes: p.clamp(0.0, 1.0),
        }),
        QuestionKind::Choice(opts) => parse_choice(opts, a),
        QuestionKind::Score(levels) => parse_score(levels, a),
    }
}

fn parse_yes_no(a: &Value) -> Option<f32> {
    if let Some(p) = num(a) {
        return Some(p);
    }
    for key in ["noul", "p_yes", "probability", "yes", "score"] {
        match a.get(key) {
            Some(v @ Value::Object(_)) => {
                if let Some(p) = parse_yes_no(v) {
                    return Some(p);
                }
            }
            Some(v) => {
                if let Some(p) = num(v) {
                    return Some(p);
                }
            }
            None => {}
        }
    }
    let probs = a.get("probabilities")?;
    let yes = probs
        .get("yes")
        .or_else(|| probs.get("true"))
        .or_else(|| probs.get("是"))
        .and_then(num);
    let no = probs
        .get("no")
        .or_else(|| probs.get("false"))
        .or_else(|| probs.get("否"))
        .and_then(num);
    match (yes, no) {
        (Some(y), Some(n)) if y + n > 0.0 => Some(y / (y + n)),
        (Some(y), _) => Some(y),
        (None, Some(n)) => Some(1.0 - n),
        _ => None,
    }
}

fn parse_choice(opts: &[(String, String)], a: &Value) -> Option<AnswerValue> {
    let labels: Vec<&str> = opts.iter().map(|(l, _)| l.as_str()).collect();
    let canonical = |raw: &Value| -> Option<String> {
        if let Some(i) = raw.as_u64() {
            return labels.get(i as usize).map(|l| l.to_string());
        }
        let s = raw.as_str()?.trim();
        let found = labels
            .iter()
            .find(|l| l.eq_ignore_ascii_case(s))
            .map_or(s, |l| *l);
        (!found.is_empty()).then(|| found.to_string())
    };

    let probs = distribution(a, &labels);
    let choice = ["choice", "label", "answer"]
        .iter()
        .find_map(|k| a.get(k).and_then(canonical))
        .or_else(|| {
            let p = probs.as_ref()?;
            let best = (0..p.len()).max_by(|&i, &j| p[i].total_cmp(&p[j]))?;
            Some(labels[best].to_string())
        })?;
    let chosen = labels.iter().position(|l| *l == choice);
    let probs = probs.unwrap_or_else(|| {
        (0..labels.len())
            .map(|i| if Some(i) == chosen { 1.0 } else { 0.0 })
            .collect()
    });
    let confidence = a
        .get("confidence")
        .and_then(num)
        .or_else(|| chosen.map(|i| probs[i]))
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    Some(AnswerValue::Choice {
        choice,
        probabilities: labels.iter().map(|l| l.to_string()).zip(probs).collect(),
        confidence,
    })
}

fn parse_score(levels: &[String], a: &Value) -> Option<AnswerValue> {
    let top = (levels.len().max(2) - 1) as f32;
    let names: Vec<&str> = levels.iter().map(String::as_str).collect();
    if let Some(probs) = distribution(a, &names) {
        let expected: f32 = probs.iter().enumerate().map(|(i, p)| i as f32 * p).sum();
        return Some(AnswerValue::Score {
            normalized: (expected / top).clamp(0.0, 1.0),
            expected_level: expected,
            probabilities: probs,
        });
    }
    let expected = if let Some(level) = a.get("level").and_then(num) {
        level
    } else {
        let s = a.get("score").and_then(num).or_else(|| num(a))?;
        // Values above 1 can only be level indices; 0..=1 is read as normalized.
        if s > 1.0 { s } else { s * top }
    };
    let expected = expected.clamp(0.0, top);
    Some(AnswerValue::Score {
        normalized: expected / top,
        expected_level: expected,
        probabilities: Vec::new(),
    })
}

/// Reads `probabilities` as an object keyed by label (or index) or an array
/// aligned with `labels`; clamps negatives and renormalizes to sum 1.
fn distribution(a: &Value, labels: &[&str]) -> Option<Vec<f32>> {
    let raw = ["probabilities", "probs", "distribution"]
        .iter()
        .find_map(|k| a.get(k))?;
    let values: Vec<f32> = match raw {
        Value::Array(items) if items.len() == labels.len() => {
            items.iter().map(|x| num(x).unwrap_or(0.0)).collect()
        }
        Value::Object(map) => labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                map.get(*l)
                    .or_else(|| {
                        map.iter()
                            .find(|(k, _)| k.trim().eq_ignore_ascii_case(l))
                            .map(|(_, v)| v)
                    })
                    .or_else(|| map.get(&i.to_string()))
                    .and_then(num)
                    .unwrap_or(0.0)
            })
            .collect(),
        _ => return None,
    };
    normalize(values)
}

fn normalize(values: Vec<f32>) -> Option<Vec<f32>> {
    let values: Vec<f32> = values
        .into_iter()
        .map(|x| if x.is_finite() { x.max(0.0) } else { 0.0 })
        .collect();
    let sum: f32 = values.iter().sum();
    (sum > 0.0).then(|| values.into_iter().map(|x| x / sum).collect())
}

/// A finite number given as a JSON number, numeric string or boolean.
fn num(v: &Value) -> Option<f32> {
    let x = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s
            .trim()
            .trim_end_matches('%')
            .parse::<f64>()
            .ok()
            .map(|x| {
                if s.trim().ends_with('%') {
                    x / 100.0
                } else {
                    x
                }
            })?,
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => return None,
    };
    x.is_finite().then_some(x as f32)
}

/// Parses JSON from model output, tolerating code fences and surrounding prose.
pub(crate) fn extract_json(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(v) = serde_json::from_str(text) {
        return Some(v);
    }
    if let Some(start) = text.find("```") {
        let body = &text[start + 3..];
        let body = body.find('\n').map_or(body, |i| &body[i + 1..]);
        let body = body.find("```").map_or(body, |i| &body[..i]);
        if let Ok(v) = serde_json::from_str(body.trim()) {
            return Some(v);
        }
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
}

const JUDGE_SYSTEM: &str = "你是一名严谨、客观的评审员。请依据给定材料逐一回答问题，用 0 到 1 之间的概率表达把握程度。只输出一个 JSON 对象，不要输出任何其他文字。";

fn judge_prompt(state: &str, questions: &[Question]) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut out = format!("## 材料\n{state}\n\n## 问题\n");
    let mut example = Vec::new();
    for (n, question) in questions.iter().enumerate() {
        let key = q(&question.key);
        let _ = write!(out, "{}. 键：{key}", n + 1);
        match &question.kind {
            QuestionKind::YesNo => {
                let _ = writeln!(out, "（是非题）\n   说明：{}", question.instructions);
                example.push(format!(
                    r#"{key}: {{"type": "noul", "noul": <回答“是”的概率>}}"#
                ));
            }
            QuestionKind::Choice(opts) => {
                let _ = writeln!(
                    out,
                    "（选择题）\n   说明：{}\n   选项：",
                    question.instructions
                );
                for (label, desc) in opts {
                    let _ = writeln!(out, "   - {}：{desc}", q(label));
                }
                let probs: Vec<String> = opts
                    .iter()
                    .map(|(l, _)| format!("{}: <概率>", q(l)))
                    .collect();
                example.push(format!(
                    r#"{key}: {{"type": "choice", "choice": <选项标签>, "probabilities": {{{}}}, "confidence": <0~1>}}"#,
                    probs.join(", ")
                ));
            }
            QuestionKind::Score(levels) => {
                let _ = writeln!(
                    out,
                    "（打分题，等级从低到高）\n   说明：{}\n   等级：",
                    question.instructions
                );
                for (i, level) in levels.iter().enumerate() {
                    let _ = writeln!(out, "   {i}. {level}");
                }
                example.push(format!(
                    r#"{key}: {{"type": "score", "score": <0~1 的归一化得分>, "probabilities": [<依次为等级 0 到 {} 的概率，共 {} 个>]}}"#,
                    levels.len() - 1,
                    levels.len()
                ));
            }
        }
    }
    let _ = write!(
        out,
        "\n## 输出要求\n只输出如下结构的 JSON。所有概率都是 0 到 1 之间的数字，同一题的概率之和为 1：\n{{\"answers\": {{\n  {}\n}}}}",
        example.join(",\n  ")
    );
    out
}
