//! Document proofreading (文档校对): the self-check before a report is
//! handed in, as opposed to fixing reviewers' comments.
//!
//! Rules settle what they can (format, numbering, place names, a list of
//! wrong words, the key figures and the cited documents), fast and free;
//! the decision model screens paragraphs, and the chat model reads the ones
//! it flags (or all of them) in sections for typos and statements that
//! contradict the project. Conflicting figures are compared locally and
//! then confirmed by the chat model. Every model finding must quote the paragraph verbatim or it is
//! dropped. See `docs/technical-design.md` §13.9.

mod apply;
pub mod citations;
pub mod divisions;
pub mod facts;
pub mod model;
pub mod rules;
mod run;
pub mod screen;

use docx_engine::{Block, Document};
use serde::{Deserialize, Serialize};

pub use apply::PROOFREAD_LABEL;
pub use citations::extract_citations;
pub use facts::extract_facts;
pub use model::{CitationLookup, CitationStatus, Replacement, SearchSnippet, Status};
pub use run::{CitationReport, ProofOptions, ProofProgress, ProofReport, ProofStage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Typo,
    Numbering,
    Consistency,
    Misattribution,
    Format,
    Citation,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Typo,
        Category::Numbering,
        Category::Consistency,
        Category::Misattribution,
        Category::Format,
        Category::Citation,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Typo => "typo",
            Category::Numbering => "numbering",
            Category::Consistency => "consistency",
            Category::Misattribution => "misattribution",
            Category::Format => "format",
            Category::Citation => "citation",
        }
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Category::Typo => "错别字",
            Category::Numbering => "序号",
            Category::Consistency => "前后一致",
            Category::Misattribution => "张冠李戴",
            Category::Format => "格式",
            Category::Citation => "引用时效",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Category::ALL
            .into_iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s) || c.label() == s)
            .or(match s {
                "错字" | "别字" | "错别字/语病" | "语病" | "用词" => {
                    Some(Category::Typo)
                }
                "前后不一致" | "前后矛盾" | "一致性" => Some(Category::Consistency),
                "项目要素" => Some(Category::Misattribution),
                _ => None,
            })
    }
}

/// How sure the finding is: `Error` is almost certainly wrong, `Warning`
/// is worth a look (建议).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Rule,
    Model,
}

/// One finding, located in a paragraph's editable text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Stable within one report: `category-paragraph-start-end`.
    pub id: String,
    pub category: Category,
    pub severity: Severity,
    /// Document paragraph index.
    pub paragraph: usize,
    /// Character offsets in the paragraph's editable text; `start == end`
    /// for a finding about the paragraph as a whole (e.g. an extra empty
    /// paragraph).
    pub start: usize,
    pub end: usize,
    /// The text at `start..end` when the finding was made.
    pub original: String,
    /// Replacement for `original`; `Some("")` deletes it, `None` means the
    /// user has to decide.
    pub suggestion: Option<String>,
    pub reason: String,
    pub source: Source,
}

impl Issue {
    /// A finding on `chars[start..end]` of paragraph `paragraph`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn span(
        category: Category,
        severity: Severity,
        paragraph: usize,
        chars: &[char],
        start: usize,
        end: usize,
        suggestion: Option<String>,
        reason: impl Into<String>,
    ) -> Self {
        Issue {
            id: String::new(),
            category,
            severity,
            paragraph,
            start,
            end,
            original: chars[start..end].iter().collect(),
            suggestion,
            reason: reason.into(),
            source: Source::Rule,
        }
    }
}

/// What is known about the project, used to spot text copied from another
/// project. Read from the document by [`extract_facts`]; the user can
/// correct it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectFacts {
    pub name: Option<String>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub district: Option<String>,
    pub owner: Option<String>,
    /// Other key figures: (label, value), e.g. ("总投资", "3.2亿元").
    pub others: Vec<(String, String)>,
}

impl ProjectFacts {
    /// Lines for a prompt; empty when nothing is known.
    pub fn prompt_text(&self) -> String {
        let mut out = String::new();
        let mut line = |k: &str, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                out.push_str(&format!("- {k}：{v}\n"));
            }
        };
        line("项目名称", &self.name);
        let place: String = [&self.province, &self.city, &self.district]
            .iter()
            .filter_map(|v| v.as_deref())
            .fold(Vec::<&str>::new(), |mut acc, v| {
                if !acc.contains(&v) {
                    acc.push(v);
                }
                acc
            })
            .concat();
        line("所在地", &(!place.is_empty()).then_some(place));
        line("建设单位", &self.owner);
        for (k, v) in &self.others {
            out.push_str(&format!("- {k}：{v}\n"));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CitationKind {
    Law,
    Standard,
    Policy,
    Unknown,
}

/// A law, standard or policy document the report cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// Title without 《》, or the standard / document number when the text
    /// cites only the number.
    pub title: String,
    pub kind: CitationKind,
    /// E.g. `GB/T 50378-2019`.
    pub standard_no: Option<String>,
    /// E.g. `沪府办发〔2023〕5号`.
    pub doc_no: Option<String>,
    /// Paragraphs citing it.
    pub paragraphs: Vec<usize>,
}

/// One paragraph to proofread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofParagraph {
    /// Document paragraph index.
    pub index: usize,
    /// Editable text (`Document::editable_text`).
    pub text: String,
    /// 0 = heading 1.
    pub heading_level: Option<u8>,
    /// Word's automatic numbering label, e.g. "1." or "（一）".
    pub list_label: Option<String>,
    /// Inside a table cell, where empty paragraphs and numbering follow
    /// other conventions.
    pub in_table: bool,
}

impl ProofParagraph {
    pub fn new(index: usize, text: impl Into<String>) -> Self {
        Self {
            index,
            text: text.into(),
            heading_level: None,
            list_label: None,
            in_table: false,
        }
    }

    pub fn heading(index: usize, text: impl Into<String>, level: u8) -> Self {
        Self {
            heading_level: Some(level),
            ..Self::new(index, text)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofInput {
    pub paragraphs: Vec<ProofParagraph>,
}

/// The paragraphs to proofread: all of them, or `range` (first and last
/// paragraph index, inclusive).
pub fn gather(doc: &Document, range: Option<(usize, usize)>) -> ProofInput {
    let count = doc.paragraphs.len();
    let (first, last) = match range {
        Some((a, b)) => (a.min(b), a.max(b).min(count.saturating_sub(1))),
        None => (0, count.saturating_sub(1)),
    };
    if count == 0 || first >= count {
        return ProofInput::default();
    }
    let paragraphs = (first..=last)
        .map(|i| ProofParagraph {
            index: i,
            text: doc.editable_text(i),
            heading_level: doc.heading_level(i),
            list_label: doc.list_label(i).map(str::to_string),
            in_table: matches!(
                doc.blocks.get(doc.block_of_paragraph(i)),
                Some(Block::Table(_))
            ),
        })
        .collect();
    ProofInput { paragraphs }
}

/// Runs every rule check that `categories` allows.
pub fn check_rules(
    input: &ProofInput,
    facts: &ProjectFacts,
    categories: &[Category],
) -> Vec<Issue> {
    let paras = &input.paragraphs;
    let mut out = Vec::new();
    if categories.contains(&Category::Format) {
        out.extend(rules::check_format(paras));
    }
    if categories.contains(&Category::Typo) {
        out.extend(screen::check_typo_words(paras));
    }
    if categories.contains(&Category::Numbering) {
        out.extend(rules::check_numbering(paras));
    }
    if categories.contains(&Category::Misattribution) {
        out.extend(rules::check_misattribution(paras, facts));
    }
    out
}

fn overlaps(a: &Issue, b: &Issue) -> bool {
    if a.paragraph != b.paragraph {
        return false;
    }
    if a.start == a.end || b.start == b.end {
        return a.start == b.start && a.end == b.end && a.category == b.category;
    }
    a.start < b.end && b.start < a.end
}

fn rank(i: &Issue) -> (Severity, bool) {
    (i.severity, i.source == Source::Rule)
}

/// Drops findings that overlap a stronger one (errors beat warnings, rules
/// beat the model), sorts by position and assigns ids.
pub fn merge_issues(mut issues: Vec<Issue>) -> Vec<Issue> {
    issues.sort_by(|a, b| {
        (a.paragraph, a.start, a.end)
            .cmp(&(b.paragraph, b.start, b.end))
            .then(rank(b).cmp(&rank(a)))
    });
    let mut kept: Vec<Issue> = Vec::with_capacity(issues.len());
    'next: for issue in issues {
        let mut i = kept.len();
        while i > 0 {
            i -= 1;
            if kept[i].paragraph != issue.paragraph {
                break;
            }
            if overlaps(&kept[i], &issue) {
                if rank(&issue) > rank(&kept[i]) {
                    kept[i] = issue;
                }
                continue 'next;
            }
        }
        kept.push(issue);
    }
    kept.sort_by_key(|i| (i.paragraph, i.start, i.end));
    let mut seen = std::collections::HashSet::new();
    for issue in &mut kept {
        let base = format!(
            "{}-{}-{}-{}",
            issue.category.as_str(),
            issue.paragraph,
            issue.start,
            issue.end
        );
        let mut id = base.clone();
        let mut n = 1;
        while !seen.insert(id.clone()) {
            n += 1;
            id = format!("{base}-{n}");
        }
        issue.id = id;
    }
    kept
}

/// The paragraph text with `issue` applied, or `None` when the text at the
/// span is no longer `issue.original` or there is no suggestion.
pub fn apply_issue(text: &str, issue: &Issue) -> Option<String> {
    let suggestion = issue.suggestion.as_deref()?;
    let chars: Vec<char> = text.chars().collect();
    if issue.start > issue.end || issue.end > chars.len() {
        return None;
    }
    let current: String = chars[issue.start..issue.end].iter().collect();
    if current != issue.original {
        return None;
    }
    let mut out: String = chars[..issue.start].iter().collect();
    out.push_str(suggestion);
    out.extend(&chars[issue.end..]);
    Some(out)
}

/// Applies several findings on one paragraph at once, last first so the
/// earlier offsets stay valid; overlapping or stale ones are skipped.
/// Returns the new text and the ids applied.
pub fn apply_issues(text: &str, issues: &[&Issue]) -> (String, Vec<String>) {
    let mut sorted: Vec<&Issue> = issues
        .iter()
        .copied()
        .filter(|i| i.suggestion.is_some())
        .collect();
    sorted.sort_by_key(|i| std::cmp::Reverse((i.start, i.end)));
    let mut text = text.to_string();
    let mut applied = Vec::new();
    let mut limit = usize::MAX;
    for issue in sorted {
        if issue.end > limit || (issue.end == limit && issue.start == issue.end) {
            continue;
        }
        if let Some(next) = apply_issue(&text, issue) {
            text = next;
            applied.push(issue.id.clone());
            limit = issue.start;
        }
    }
    (text, applied)
}

/// Stable 64-bit FNV-1a hash as hex, for cache keys.
pub(crate) fn hash(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.as_bytes().iter().chain([&0xffu8]) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(p: usize, start: usize, end: usize, sev: Severity, source: Source) -> Issue {
        Issue {
            id: String::new(),
            category: Category::Format,
            severity: sev,
            paragraph: p,
            start,
            end,
            original: String::new(),
            suggestion: None,
            reason: String::new(),
            source,
        }
    }

    #[test]
    fn merge_keeps_the_stronger_finding() {
        let issues = vec![
            issue(2, 5, 8, Severity::Warning, Source::Model),
            issue(2, 6, 7, Severity::Warning, Source::Rule),
            issue(1, 0, 3, Severity::Warning, Source::Rule),
            issue(1, 2, 4, Severity::Error, Source::Model),
            issue(3, 0, 0, Severity::Warning, Source::Rule),
            issue(3, 0, 2, Severity::Warning, Source::Rule),
        ];
        let merged = merge_issues(issues);
        let spans: Vec<_> = merged
            .iter()
            .map(|i| (i.paragraph, i.start, i.end, i.source))
            .collect();
        assert_eq!(
            spans,
            [
                (1, 2, 4, Source::Model),
                (2, 6, 7, Source::Rule),
                (3, 0, 0, Source::Rule),
                (3, 0, 2, Source::Rule)
            ]
        );
        assert_eq!(merged[0].id, "format-1-2-4");
    }

    #[test]
    fn applies_when_the_span_still_matches() {
        let text = "项目位于浦东新区，，总投资3亿元。";
        let chars: Vec<char> = text.chars().collect();
        let mut a = Issue::span(
            Category::Misattribution,
            Severity::Error,
            0,
            &chars,
            4,
            8,
            Some("崇明区".into()),
            "",
        );
        a.id = "a".into();
        let mut b = Issue::span(
            Category::Format,
            Severity::Error,
            0,
            &chars,
            8,
            10,
            Some("，".into()),
            "",
        );
        b.id = "b".into();
        assert_eq!(
            apply_issue(text, &a).unwrap(),
            "项目位于崇明区，，总投资3亿元。"
        );
        assert_eq!(apply_issue("项目位于崇明区", &a), None);
        let (out, applied) = apply_issues(text, &[&a, &b]);
        assert_eq!(out, "项目位于崇明区，总投资3亿元。");
        assert_eq!(applied, ["b", "a"]);
        let mut none = a.clone();
        none.suggestion = None;
        assert_eq!(apply_issue(text, &none), None);
    }

    #[test]
    fn category_parsing_and_hash() {
        assert_eq!(Category::parse("错别字"), Some(Category::Typo));
        assert_eq!(
            Category::parse("Misattribution"),
            Some(Category::Misattribution)
        );
        assert_eq!(Category::parse("前后矛盾"), Some(Category::Consistency));
        assert_eq!(Category::parse("x"), None);
        assert_eq!(hash(&["a", "b"]), hash(&["a", "b"]));
        assert_ne!(hash(&["ab"]), hash(&["a", "b"]));
        let json = serde_json::to_value(Severity::Error).unwrap();
        assert_eq!(json, "error");
    }

    #[test]
    fn facts_prompt_text() {
        let f = ProjectFacts {
            name: Some("崇明区示范项目".into()),
            province: Some("上海市".into()),
            city: Some("上海市".into()),
            district: Some("崇明区".into()),
            owner: None,
            others: vec![("总投资".into(), "3.2亿元".into())],
        };
        assert_eq!(
            f.prompt_text(),
            "- 项目名称：崇明区示范项目\n- 所在地：上海市崇明区\n- 总投资：3.2亿元\n"
        );
    }
}
