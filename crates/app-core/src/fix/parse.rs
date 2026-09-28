//! Reading the model's JSON answer.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub paragraphs: Vec<String>,
    pub explanation: String,
    pub citations: Vec<usize>,
}

/// The first JSON object in `text`, tolerating code fences and chatter.
fn json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// Parses an answer that must hold exactly `expected` paragraphs.
pub fn rewrite(text: &str, expected: usize) -> Result<Rewrite, String> {
    let v = json_object(text).ok_or("没有找到 JSON 对象")?;
    let paragraphs: Vec<String> = match v.get("paragraphs").or_else(|| v.get("paragraph")) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|p| match p {
                Value::String(s) => Ok(s.clone()),
                Value::Object(o) => o
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or("paragraphs 中有非文字项"),
                _ => Err("paragraphs 中有非文字项"),
            })
            .collect::<Result<_, _>>()?,
        Some(Value::String(s)) => vec![s.clone()],
        _ => return Err("缺少 paragraphs 字段".into()),
    };
    if paragraphs.len() != expected {
        return Err(format!(
            "应返回 {expected} 段，实际返回 {} 段",
            paragraphs.len()
        ));
    }
    let paragraphs = paragraphs
        .into_iter()
        .map(|p| strip_label(p.trim()).to_string())
        .collect();
    let explanation = v
        .get("explanation")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let citations = match v.get("citations") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|c| match c {
                Value::Number(n) => n.as_u64().map(|n| n as usize),
                Value::String(s) => s.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok(),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(Rewrite {
        paragraphs,
        explanation,
        citations,
    })
}

/// Drops a "[第1段]" label the model may have copied from the prompt.
fn strip_label(p: &str) -> &str {
    if let Some(rest) = p.strip_prefix("[第")
        && let Some(end) = rest.find("段]")
        && rest[..end].chars().all(|c| c.is_ascii_digit())
    {
        return rest[end + "段]".len()..].trim_start();
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let text = "好的：\n```json\n{\"paragraphs\": [\"[第1段] 新文本。\"], \"explanation\": \"补充来源\", \"citations\": [2, \"[3]\"]}\n```";
        let r = rewrite(text, 1).unwrap();
        assert_eq!(r.paragraphs, ["新文本。"]);
        assert_eq!(r.explanation, "补充来源");
        assert_eq!(r.citations, [2, 3]);
    }

    #[test]
    fn rejects_wrong_paragraph_count() {
        let err = rewrite("{\"paragraphs\": [\"a\", \"b\"]}", 1).unwrap_err();
        assert!(err.contains("应返回 1 段"));
        assert!(rewrite("没有 JSON", 1).is_err());
        assert_eq!(
            rewrite("{\"paragraphs\": \"单段\"}", 1).unwrap().paragraphs,
            ["单段"]
        );
    }
}
