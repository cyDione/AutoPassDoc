//! What the chat model is asked during proofreading, and how its answers
//! are read: the section check (typos, statements that contradict the
//! project, figures), the confirmation of conflicting figures, and the
//! status of a cited document given web search results.

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::rules::find_chars;
use super::{
    Category, Citation, CitationKind, Issue, ProjectFacts, ProofParagraph, Severity, Source,
};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Tolerant JSON

/// The first JSON value in `text` (object or array), tolerating code
/// fences and chatter around it.
pub fn json_value(text: &str) -> Option<Value> {
    let obj = text.find('{').zip(text.rfind('}'));
    let arr = text.find('[').zip(text.rfind(']'));
    let mut spans: Vec<(usize, usize)> = [obj, arr]
        .into_iter()
        .flatten()
        .filter(|(a, b)| a < b)
        .collect();
    spans.sort();
    spans
        .into_iter()
        .find_map(|(a, b)| serde_json::from_str(&text[a..=b]).ok())
}

/// The array under `key` (or the answer itself when it is an array).
fn items(v: &Value, key: &str) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn num_field(v: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|k| {
        let v = v.get(*k)?;
        v.as_u64().map(|n| n as usize).or_else(|| {
            v.as_str()?
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })
    })
}

// ---------------------------------------------------------------------------
// Segments

/// Splits paragraphs into sections of about `max_chars` characters for the
/// model, preferring to cut before a heading. Empty paragraphs are left
/// out; each section lists positions in `paras`.
pub fn segment(paras: &[ProofParagraph], max_chars: usize) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut size = 0;
    for (i, p) in paras.iter().enumerate() {
        let len = p.text.trim().chars().count();
        if len == 0 {
            continue;
        }
        let heading = p.heading_level.is_some();
        let full = size + len > max_chars;
        let good_cut = heading && size >= max_chars / 2;
        if !cur.is_empty() && (full || good_cut) {
            out.push(std::mem::take(&mut cur));
            size = 0;
        }
        cur.push(i);
        size += len;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// Section check

pub const CHECK_SYSTEM: &str = "你是一名严谨的中文公文和工程咨询报告校对员。逐段检查下面的文字，只找确定的错误，宁缺毋滥。

要找的问题：
1. typo：错别字、多字漏字、的地得误用、常见公文用词错误、明显的语病。
2. misattribution：与【项目要素】不符的内容，例如项目在崇明区却写成别的区，项目名称、建设单位写错。
3. consistency：同一段落内前后矛盾的说法。

要求：
- original 必须从该段原文中逐字摘录，只摘出错的最短片段（一般不超过 20 字），并能在该段中找到。
- suggestion 是改正后的片段，用来直接替换 original；无法确定怎么改时写 null。
- reason 用一句话说明错在哪里。
- 不要改动风格、不要润色、不要改标点和空格（另有规则检查）。

同时抽取“关键指标”（用于检查全文前后是否一致）：只要项目或其组成部分的总投资、分项投资、建设规模、占地面积、建筑面积、长度、处理能力、建设工期、开竣工日期这类会在全文多处出现的指标。不要抽取单价、明细数量、序号、页码、年份、标准编号和一般统计数据。
- 每条写成数组 [段号, \"主体\", \"指标名\", \"数值\", \"单位\"]：主体如“项目”“一期工程”“污水处理厂”，指标名如“总投资”“占地面积”，数值用阿拉伯数字。

只输出一个 JSON 对象，不要解释：
{\"issues\": [{\"p\": 段号, \"original\": \"原文片段\", \"suggestion\": \"改正后片段\", \"category\": \"typo\", \"reason\": \"理由\"}],
 \"facts\": [[段号, \"项目\", \"总投资\", \"3.2\", \"亿元\"]]}
没有问题时 issues 为空数组；没有关键指标或不需要抽取时 facts 为空数组。";

/// The user message for one section: project facts, then numbered
/// paragraphs.
pub fn check_prompt(facts: &ProjectFacts, paras: &[&ProofParagraph], want_facts: bool) -> String {
    let mut s = String::new();
    let known = facts.prompt_text();
    if !known.is_empty() {
        s.push_str("【项目要素】\n");
        s.push_str(&known);
        s.push('\n');
    }
    if !want_facts {
        s.push_str("（本次不需要抽取指标事实，facts 返回空数组。）\n\n");
    }
    s.push_str("【待校对段落】（方括号内是段号）\n");
    for p in paras {
        s.push_str(&format!("[{}] {}\n", p.index, p.text.trim_end()));
    }
    s
}

/// A figure stated in the text, e.g. 项目 / 总投资 / 3.2 亿元.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub paragraph: usize,
    pub subject: String,
    pub metric: String,
    pub value: String,
    pub unit: String,
    /// The words in the paragraph stating it, when they could be found.
    pub text: String,
}

/// Reads the section check: findings that quote their paragraph verbatim,
/// and the figures. Items for paragraphs outside the section are dropped.
pub fn parse_check(text: &str, paras: &[&ProofParagraph]) -> Option<(Vec<Issue>, Vec<Fact>)> {
    let v = json_value(text)?;
    let find = |item: &Value| {
        let p = num_field(item, &["p", "paragraph", "段号"])?;
        paras.iter().find(|x| x.index == p).copied()
    };
    let mut issues = Vec::new();
    for item in items(&v, "issues") {
        let Some(para) = find(&item) else { continue };
        let Some(original) = str_field(&item, &["original", "quote", "原文片段"]) else {
            continue;
        };
        let chars: Vec<char> = para.text.chars().collect();
        let Some(start) = find_chars(&chars, original) else {
            continue;
        };
        let suggestion = str_field(&item, &["suggestion", "建议"]).map(str::to_string);
        if suggestion.as_deref() == Some(original) {
            continue;
        }
        let category = str_field(&item, &["category", "类别"])
            .and_then(Category::parse)
            .filter(|c| {
                matches!(
                    c,
                    Category::Typo | Category::Misattribution | Category::Consistency
                )
            })
            .unwrap_or(Category::Typo);
        let severity = match str_field(&item, &["severity"]) {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            _ if category == Category::Typo && suggestion.is_some() => Severity::Error,
            _ => Severity::Warning,
        };
        let reason = str_field(&item, &["reason", "理由"])
            .unwrap_or(category.label())
            .to_string();
        let end = start + original.chars().count();
        let mut issue = Issue::span(
            category, severity, para.index, &chars, start, end, suggestion, reason,
        );
        issue.source = Source::Model;
        issues.push(issue);
    }
    let mut facts = Vec::new();
    for item in items(&v, "facts") {
        // Compact form: [p, subject, metric, value, unit].
        let item = match item {
            Value::Array(a) => {
                let keys = ["p", "subject", "metric", "value", "unit"];
                Value::Object(keys.iter().map(|k| k.to_string()).zip(a).collect())
            }
            other => other,
        };
        let Some(para) = find(&item) else { continue };
        let (Some(metric), Some(value)) = (
            str_field(&item, &["metric", "指标"]),
            item.get("value").and_then(|v| match v {
                Value::String(s) => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }),
        ) else {
            continue;
        };
        let unit = str_field(&item, &["unit", "单位"])
            .unwrap_or("")
            .to_string();
        let quoted = str_field(&item, &["text", "original"]).unwrap_or("");
        let text = [
            quoted.to_string(),
            format!("{metric}{value}{unit}"),
            format!("{metric}约{value}{unit}"),
            format!("{metric}为{value}{unit}"),
            format!("{value}{unit}"),
            value.clone(),
        ]
        .into_iter()
        .find(|t| !t.is_empty() && para.text.contains(t.as_str()))
        .unwrap_or_default();
        facts.push(Fact {
            paragraph: para.index,
            subject: str_field(&item, &["subject", "主体"])
                .unwrap_or("项目")
                .to_string(),
            metric: metric.to_string(),
            value,
            unit,
            text,
        });
    }
    Some((issues, facts))
}

// ---------------------------------------------------------------------------
// Conflicting figures

/// Figures about the same thing whose values differ.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictGroup {
    pub subject: String,
    pub metric: String,
    pub facts: Vec<Fact>,
}

fn normalize_subject(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let s = s
        .trim_start_matches("本")
        .trim_start_matches("该")
        .trim_start_matches("整个");
    match s {
        "" | "项目" | "工程" | "本工程" | "项目整体" | "建设项目" => "项目".into(),
        s => s.to_string(),
    }
}

fn normalize_metric(s: &str) -> String {
    let mut s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    for w in [
        "约", "共计", "合计", "总计", "预计", "计划", "估算", "概算", "项目",
    ] {
        s = s.replace(w, "");
    }
    let s = s.trim_start_matches('总');
    match s {
        "工期" | "建设周期" | "建设工期" | "施工工期" | "周期" => "工期".into(),
        "投资" | "投资额" | "投资规模" => "投资".into(),
        s => s.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dim {
    Money,
    Area,
    Duration,
    Length,
    Other,
}

/// The unit's dimension and factor to its base (元, 平方米, 月, 米).
fn unit_factor(unit: &str) -> (Dim, f64) {
    let u: String = unit.chars().filter(|c| !c.is_whitespace()).collect();
    match u.as_str() {
        "元" => (Dim::Money, 1.0),
        "千元" => (Dim::Money, 1e3),
        "万元" | "万" => (Dim::Money, 1e4),
        "亿元" | "亿" => (Dim::Money, 1e8),
        "平方米" | "㎡" | "m2" | "m²" | "平米" => (Dim::Area, 1.0),
        "万平方米" | "万㎡" | "万m2" | "万m²" => (Dim::Area, 1e4),
        "公顷" | "hm2" | "hm²" => (Dim::Area, 1e4),
        "亩" => (Dim::Area, 10_000.0 / 15.0),
        "平方公里" | "平方千米" | "km2" | "km²" => (Dim::Area, 1e6),
        "天" | "日" | "日历天" => (Dim::Duration, 1.0 / 30.0),
        "个月" | "月" => (Dim::Duration, 1.0),
        "年" => (Dim::Duration, 12.0),
        "米" | "m" => (Dim::Length, 1.0),
        "公里" | "千米" | "km" => (Dim::Length, 1000.0),
        _ => (Dim::Other, 1.0),
    }
}

/// A figure in base units, or `None` when the value is not a number.
fn base_value(f: &Fact) -> Option<(Dim, f64, String)> {
    let digits: String = f
        .value
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut n: f64 = digits.parse().ok()?;
    let mut unit = f.unit.clone();
    // "3.2亿" given as the value with an empty unit.
    if unit.is_empty() {
        unit = f
            .value
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ',')
            .to_string();
    }
    let (dim, factor) = unit_factor(&unit);
    n *= factor;
    Some((dim, n, unit))
}

fn close(a: f64, b: f64, dim: Dim) -> bool {
    let tol = if dim == Dim::Area { 0.01 } else { 0.005 };
    (a - b).abs() <= tol * a.abs().max(b.abs()).max(1e-9)
}

/// Groups figures by subject and metric (loosely: 总投资 = 投资, 工期 =
/// 建设工期), converts units, and returns the groups holding more than one
/// value.
pub fn find_conflicts(facts: &[Fact]) -> Vec<ConflictGroup> {
    struct Group {
        subject: String,
        metric: String,
        dim: Dim,
        facts: Vec<(Fact, f64, String)>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for f in facts {
        let Some((dim, value, unit)) = base_value(f) else {
            continue;
        };
        let subject = normalize_subject(&f.subject);
        let metric = normalize_metric(&f.metric);
        if metric.is_empty() {
            continue;
        }
        let found = groups.iter_mut().find(|g| {
            g.subject == subject
                && g.dim == dim
                && (g.metric == metric || g.metric.contains(&metric) || metric.contains(&g.metric))
                && (dim != Dim::Other || g.facts[0].2 == unit)
        });
        match found {
            Some(g) => g.facts.push((f.clone(), value, unit)),
            None => groups.push(Group {
                subject,
                metric,
                dim,
                facts: vec![(f.clone(), value, unit)],
            }),
        }
    }
    groups
        .into_iter()
        .filter(|g| {
            let first = g.facts[0].1;
            g.facts.iter().any(|(_, v, _)| !close(first, *v, g.dim))
        })
        .map(|g| ConflictGroup {
            subject: g.subject,
            metric: g.metric,
            facts: g.facts.into_iter().map(|(f, _, _)| f).collect(),
        })
        .collect()
}

pub const CONFIRM_SYSTEM: &str = "你在检查一份报告前后数值是否一致。下面每一组是报告不同位置对同一指标的表述，数值不同。请判断每组是否真的前后矛盾：口径不同（如总投资与其中某一部分、一期与全部、估算与概算、不同年份）不算矛盾；同一口径数值不同才算矛盾。

只输出一个 JSON 对象：
{\"groups\": [{\"id\": 组号, \"conflict\": true, \"reason\": \"一句话说明\"}]}";

pub fn confirm_prompt(groups: &[ConflictGroup], paras: &[ProofParagraph]) -> String {
    let mut s = String::new();
    for (i, g) in groups.iter().enumerate() {
        s.push_str(&format!("【第{}组】{} · {}\n", i + 1, g.subject, g.metric));
        for f in &g.facts {
            let context = paras
                .iter()
                .find(|p| p.index == f.paragraph)
                .map(|p| clip_around(&p.text, &f.text, 60))
                .unwrap_or_default();
            s.push_str(&format!(
                "- 第{}段：{}{}（原文：{}）\n",
                f.paragraph, f.value, f.unit, context
            ));
        }
        s.push('\n');
    }
    s
}

fn clip_around(text: &str, needle: &str, radius: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let at = if needle.is_empty() {
        0
    } else {
        find_chars(&chars, needle).unwrap_or(0)
    };
    let start = at.saturating_sub(radius);
    let end = (at + needle.chars().count() + radius).min(chars.len());
    chars[start..end].iter().collect()
}

/// The groups the model confirmed as real contradictions, with its reason.
pub fn parse_confirm(text: &str, count: usize) -> Vec<(usize, String)> {
    let Some(v) = json_value(text) else {
        return Vec::new();
    };
    items(&v, "groups")
        .iter()
        .filter(|g| {
            g.get("conflict")
                .or_else(|| g.get("contradiction"))
                .is_some_and(|c| c.as_bool() == Some(true) || c.as_str() == Some("true"))
        })
        .filter_map(|g| {
            let id = num_field(g, &["id", "group"])?;
            (1..=count).contains(&id).then(|| {
                (
                    id - 1,
                    str_field(g, &["reason", "理由"]).unwrap_or("").to_string(),
                )
            })
        })
        .collect()
}

/// Findings for a confirmed group: one per figure that could be located.
pub fn conflict_issues(
    group: &ConflictGroup,
    reason: &str,
    paras: &[ProofParagraph],
) -> Vec<Issue> {
    let mut out = Vec::new();
    for f in &group.facts {
        let Some(p) = paras.iter().find(|p| p.index == f.paragraph) else {
            continue;
        };
        if f.text.is_empty() {
            continue;
        }
        let chars: Vec<char> = p.text.chars().collect();
        let Some(start) = find_chars(&chars, &f.text) else {
            continue;
        };
        let others: Vec<String> = group
            .facts
            .iter()
            .filter(|o| !std::ptr::eq(*o, f))
            .map(|o| format!("第{}段为{}{}", o.paragraph, o.value, o.unit))
            .collect();
        let mut text = format!(
            "{}的{}前后不一致：此处为{}{}，{}",
            group.subject,
            group.metric,
            f.value,
            f.unit,
            others.join("，")
        );
        if !reason.is_empty() {
            text.push_str(&format!("。{reason}"));
        }
        let mut issue = Issue::span(
            Category::Consistency,
            Severity::Warning,
            p.index,
            &chars,
            start,
            start + f.text.chars().count(),
            None,
            text,
        );
        issue.source = Source::Model;
        out.push(issue);
    }
    out
}

// ---------------------------------------------------------------------------
// Citation status

/// One web search result about a cited document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Web search for the citation checks, supplied by the caller (the
/// search itself lives elsewhere). Typically searches “标题 + 废止 / 修订 /
/// 最新版”.
pub trait CitationLookup: Send + Sync {
    fn lookup<'a>(&'a self, citation: &'a Citation) -> BoxFuture<'a, Result<Vec<SearchSnippet>>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Current,
    Repealed,
    Superseded,
    Unknown,
}

/// The newer version replacing a cited document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Replacement {
    pub title: String,
    /// Standard number or document number.
    pub number: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationStatus {
    pub status: Status,
    pub replacement: Option<Replacement>,
    pub reason: String,
}

pub const CITATION_SYSTEM: &str = "你在核查报告引用的法律法规、标准规范或政策文件是否仍然有效。根据下面的网络搜索结果判断它的状态：
- current：现行有效；
- repealed：已废止，且没有替代文件；
- superseded：已被新版本或新文件替代（请给出新版本的名称、标准号或文号、链接）；
- unknown：搜索结果不足以判断。
只依据搜索结果判断，不要凭印象；标准号年份不同通常意味着有新版本。

只输出一个 JSON 对象：
{\"status\": \"current|repealed|superseded|unknown\", \"replacement\": {\"title\": \"新版名称\", \"number\": \"新版编号\", \"url\": \"链接\"}, \"reason\": \"一句话依据\"}
没有替代文件时 replacement 为 null。";

fn kind_label(k: CitationKind) -> &'static str {
    match k {
        CitationKind::Law => "法律法规",
        CitationKind::Standard => "标准规范",
        CitationKind::Policy => "政策文件",
        CitationKind::Unknown => "文件",
    }
}

pub fn citation_prompt(c: &Citation, snippets: &[SearchSnippet]) -> String {
    let mut s = format!("【被引用的{}】《{}》", kind_label(c.kind), c.title);
    if let Some(n) = &c.standard_no {
        s.push_str(&format!(" {n}"));
    }
    if let Some(n) = &c.doc_no {
        s.push_str(&format!("（{n}）"));
    }
    s.push_str("\n\n【搜索结果】\n");
    if snippets.is_empty() {
        s.push_str("（没有搜索结果）\n");
    }
    for (i, r) in snippets.iter().take(8).enumerate() {
        s.push_str(&format!(
            "{}. {}\n   {}\n   {}\n",
            i + 1,
            r.title.trim(),
            r.url.trim(),
            r.snippet.trim()
        ));
    }
    s
}

pub fn parse_citation_status(text: &str) -> Option<CitationStatus> {
    let v = json_value(text)?;
    let status = match str_field(&v, &["status", "状态"])?
        .to_ascii_lowercase()
        .as_str()
    {
        "current" | "现行" | "有效" => Status::Current,
        "repealed" | "废止" | "已废止" => Status::Repealed,
        "superseded" | "replaced" | "替代" | "已替代" => Status::Superseded,
        _ => Status::Unknown,
    };
    let replacement = v.get("replacement").and_then(|r| {
        let title = str_field(r, &["title", "name"])?.to_string();
        Some(Replacement {
            title: title.trim_matches(['《', '》']).to_string(),
            number: str_field(r, &["number", "no", "code"]).map(str::to_string),
            url: str_field(r, &["url", "link"])
                .filter(|u| u.starts_with("http"))
                .map(str::to_string),
        })
    });
    Some(CitationStatus {
        status,
        replacement: if status == Status::Superseded {
            replacement
        } else {
            None
        },
        reason: str_field(&v, &["reason", "理由"]).unwrap_or("").to_string(),
    })
}

/// Findings for an outdated citation, at each paragraph citing it: the
/// standard number (replaced by the new one) or the title in 《》.
pub fn citation_issues(c: &Citation, st: &CitationStatus, paras: &[ProofParagraph]) -> Vec<Issue> {
    let (severity, what) = match st.status {
        Status::Repealed => (Severity::Error, "已废止"),
        Status::Superseded => (Severity::Warning, "已有新版本"),
        _ => return Vec::new(),
    };
    let new = st.replacement.as_ref();
    let mut reason = format!("《{}》{what}", c.title);
    if let Some(r) = new {
        reason.push_str(&format!("，现行版本为《{}》", r.title));
        if let Some(n) = &r.number {
            reason.push_str(&format!("（{n}）"));
        }
    }
    if !st.reason.is_empty() {
        reason.push_str(&format!("。{}", st.reason));
    }
    let mut out = Vec::new();
    for &pi in &c.paragraphs {
        let Some(p) = paras.iter().find(|p| p.index == pi) else {
            continue;
        };
        let chars: Vec<char> = p.text.chars().collect();
        let by_number = c.standard_no.as_ref().and_then(|old| {
            let start = locate_standard(&p.text, old)?;
            let sug = new.and_then(|r| r.number.clone());
            Some((start.0, start.1, sug))
        });
        let located = by_number.or_else(|| {
            let quoted = format!("《{}》", c.title);
            let start = find_chars(&chars, &quoted)
                .map(|s| (s, s + quoted.chars().count()))
                .or_else(|| {
                    find_chars(&chars, &c.title).map(|s| (s, s + c.title.chars().count()))
                })?;
            let sug = new.filter(|r| r.title != c.title).map(|r| {
                if chars[start.0] == '《' {
                    format!("《{}》", r.title)
                } else {
                    r.title.clone()
                }
            });
            Some((start.0, start.1, sug))
        });
        let Some((start, end, suggestion)) = located else {
            continue;
        };
        let mut issue = Issue::span(
            Category::Citation,
            severity,
            p.index,
            &chars,
            start,
            end,
            suggestion,
            reason.clone(),
        );
        issue.source = Source::Model;
        out.push(issue);
    }
    out
}

/// Char range of the standard number `number` as written in `text`.
fn locate_standard(text: &str, number: &str) -> Option<(usize, usize)> {
    let m = super::citations::standard_numbers(text)
        .into_iter()
        .find(|m| m.number == number)?;
    // The matcher works on width-narrowed text, which keeps char counts;
    // convert its byte offsets back through that text.
    let narrow: String = text
        .chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            c => c,
        })
        .collect();
    let start = narrow[..m.start].chars().count();
    let end = narrow[..m.end].chars().count();
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(index: usize, text: &str) -> ProofParagraph {
        ProofParagraph::new(index, text)
    }

    #[test]
    fn segments_cut_before_headings() {
        let paras = vec![
            ProofParagraph::heading(0, "第一章 总论", 0),
            p(1, &"甲".repeat(60)),
            p(2, ""),
            p(3, &"乙".repeat(30)),
            ProofParagraph::heading(4, "第二章 方案", 0),
            p(5, &"丙".repeat(120)),
            p(6, &"丁".repeat(10)),
        ];
        assert_eq!(
            segment(&paras, 100),
            [vec![0, 1, 3], vec![4], vec![5], vec![6]]
        );
        assert_eq!(segment(&paras, 150), [vec![0, 1, 3], vec![4, 5, 6]]);
        assert_eq!(segment(&paras, 1000), [vec![0, 1, 3, 4, 5, 6]]);
        assert!(segment(&[p(0, " ")], 100).is_empty());
    }

    #[test]
    fn check_answers_must_quote_the_paragraph() {
        let a = p(3, "本项目位于浦东新区，总投资约3.2亿元，建设工期24个月。");
        let b = p(4, "项目建成后可有效改善区域水环竟。");
        let paras = vec![&a, &b];
        let answer = r#"好的，结果如下：
```json
{"issues": [
  {"p": 4, "original": "水环竟", "suggestion": "水环境", "category": "typo", "reason": "错别字"},
  {"p": "[3]", "original": "浦东新区", "suggestion": "崇明区", "category": "misattribution", "reason": "项目位于崇明区"},
  {"p": 3, "original": "不存在的片段", "suggestion": "x", "category": "typo", "reason": ""},
  {"p": 9, "original": "水环竟", "suggestion": "水环境"},
  {"p": 4, "original": "有效", "suggestion": "有效", "category": "typo"},
  {"p": 4, "original": "区域", "suggestion": null, "category": "语病", "reason": "表述不清"}
 ],
 "facts": [
  {"p": 3, "subject": "本项目", "metric": "总投资", "value": "3.2", "unit": "亿元", "text": "总投资约3.2亿元"},
  {"p": 3, "subject": "项目", "metric": "建设工期", "value": 24, "unit": "个月", "text": "工期二十四个月"},
  {"p": 7, "subject": "项目", "metric": "面积", "value": "1", "unit": "公顷"}
 ]}
```"#;
        let (issues, facts) = parse_check(answer, &paras).unwrap();
        let spans: Vec<_> = issues
            .iter()
            .map(|i| (i.paragraph, i.start, i.end, i.category, i.severity))
            .collect();
        assert_eq!(
            spans,
            [
                (4, 12, 15, Category::Typo, Severity::Error),
                (3, 5, 9, Category::Misattribution, Severity::Warning),
                (4, 10, 12, Category::Typo, Severity::Warning),
            ]
        );
        assert_eq!(issues[0].original, "水环竟");
        assert_eq!(issues[0].source, Source::Model);
        assert_eq!(issues[2].suggestion, None);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].text, "总投资约3.2亿元");
        assert_eq!(facts[1].value, "24");
        assert_eq!(
            facts[1].text, "建设工期24个月",
            "falls back to metric + value + unit"
        );
        // The compact form the prompt asks for.
        let (_, facts) =
            parse_check(r#"{"facts": [[3, "项目", "总投资", 3.2, "亿元"]]}"#, &paras).unwrap();
        assert_eq!(facts[0].text, "总投资约3.2亿元");
        assert_eq!(facts[0].subject, "项目");
        assert!(parse_check("没有 JSON", &paras).is_none());
        // A bare array is accepted as the issue list.
        let (issues, _) = parse_check(
            r#"[{"p":4,"original":"水环竟","suggestion":"水环境"}]"#,
            &paras,
        )
        .unwrap();
        assert_eq!(issues.len(), 1);
    }

    fn fact(p: usize, subject: &str, metric: &str, value: &str, unit: &str) -> Fact {
        Fact {
            paragraph: p,
            subject: subject.into(),
            metric: metric.into(),
            value: value.into(),
            unit: unit.into(),
            text: format!("{value}{unit}"),
        }
    }

    #[test]
    fn conflicts_after_unit_conversion() {
        let facts = vec![
            fact(1, "本项目", "总投资", "3.2", "亿元"),
            fact(5, "项目", "投资", "32000", "万元"),
            fact(9, "项目", "总投资约", "3.5", "亿元"),
            fact(2, "项目", "占地面积", "15", "亩"),
            fact(6, "项目", "占地面积", "1", "公顷"),
            fact(3, "项目", "建设工期", "2", "年"),
            fact(7, "该项目", "工期", "24", "个月"),
            fact(4, "一期工程", "总投资", "1.2", "亿元"),
            fact(8, "项目", "绿地率", "35", "%"),
            fact(10, "项目", "绿地率", "30", "%"),
            fact(11, "项目", "说明", "若干", ""),
        ];
        let groups = find_conflicts(&facts);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].metric, "投资");
        let ps: Vec<_> = groups[0].facts.iter().map(|f| f.paragraph).collect();
        assert_eq!(ps, [1, 5, 9]);
        assert_eq!(groups[1].metric, "绿地率");
    }

    #[test]
    fn confirm_round_trip() {
        let paras = vec![
            p(1, "本项目总投资3.2亿元。"),
            p(9, "经测算，项目总投资约3.5亿元。"),
        ];
        let groups = vec![ConflictGroup {
            subject: "项目".into(),
            metric: "投资".into(),
            facts: vec![
                fact(1, "项目", "总投资", "3.2", "亿元"),
                fact(9, "项目", "总投资", "3.5", "亿元"),
            ],
        }];
        let prompt = confirm_prompt(&groups, &paras);
        assert!(prompt.contains("【第1组】项目 · 投资"));
        assert!(prompt.contains("第9段：3.5亿元（原文：经测算，项目总投资约3.5亿元。）"));
        let confirmed = parse_confirm(
            r#"{"groups":[{"id":1,"conflict":true,"reason":"同一口径"},{"id":2,"conflict":true},{"id":"1","conflict":false}]}"#,
            1,
        );
        assert_eq!(confirmed, [(0, "同一口径".to_string())]);
        let issues = conflict_issues(&groups[0], "同一口径", &paras);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[1].original, "3.5亿元");
        assert!(issues[0].reason.contains("第9段为3.5亿元"));
        assert_eq!(issues[0].category, Category::Consistency);
    }

    #[test]
    fn citation_status_and_issues() {
        let st = parse_citation_status(
            r#"```json
{"status":"superseded","replacement":{"title":"《室外排水设计标准》","number":"GB 50014-2021","url":"https://openstd.samr.gov.cn/x"},"reason":"2021年版已实施"}
```"#,
        )
        .unwrap();
        assert_eq!(st.status, Status::Superseded);
        let r = st.replacement.clone().unwrap();
        assert_eq!(r.title, "室外排水设计标准");
        assert_eq!(r.number.as_deref(), Some("GB 50014-2021"));
        let current =
            parse_citation_status(r#"{"status":"current","replacement":{"title":"x"}}"#).unwrap();
        assert_eq!(current.replacement, None);
        assert_eq!(
            parse_citation_status(r#"{"status":"maybe"}"#)
                .unwrap()
                .status,
            Status::Unknown
        );

        let old = Citation {
            title: "室外排水设计规范".into(),
            kind: CitationKind::Standard,
            standard_no: Some("GB 50014-2006".into()),
            doc_no: None,
            paragraphs: vec![2, 5],
        };
        let paras = vec![
            p(2, "排水管网按《室外排水设计规范》（GB 50014-2006）设计。"),
            p(5, "详见《室外排水设计规范》。"),
        ];
        let issues = citation_issues(&old, &st, &paras);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].original, "GB 50014-2006");
        assert_eq!(issues[0].suggestion.as_deref(), Some("GB 50014-2021"));
        assert_eq!(issues[1].original, "《室外排水设计规范》");
        assert_eq!(
            issues[1].suggestion.as_deref(),
            Some("《室外排水设计标准》")
        );
        assert!(issues[1].reason.contains("已有新版本"));
        let prompt = citation_prompt(
            &old,
            &[SearchSnippet {
                title: "GB 50014-2021".into(),
                url: "https://example.com".into(),
                snippet: "代替 GB 50014-2006".into(),
            }],
        );
        assert!(prompt.contains("【被引用的标准规范】《室外排水设计规范》 GB 50014-2006"));
        let fine = CitationStatus {
            status: Status::Current,
            replacement: None,
            reason: String::new(),
        };
        assert!(citation_issues(&old, &fine, &paras).is_empty());
    }
}
