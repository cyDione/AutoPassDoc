//! What the model sees about a comment: the comment thread, the paragraphs
//! it covers (as editable text), the quoted words, the section it sits in
//! and the neighbouring paragraphs, plus what other sections of the document
//! say about the same thing.

use std::collections::{HashMap, HashSet};

use docx_engine::{Document, Inline, Revision};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::proofread::rules::is_han;

/// Comments spanning more paragraphs than this are left to the user.
pub const MAX_PARAGRAPHS: usize = 6;
/// A range the user selected by hand may be larger.
pub const MAX_SELECTED_PARAGRAPHS: usize = 12;
const NEIGHBOURS: usize = 2;
/// Paragraphs from elsewhere in the document given as related content.
const RELATED: usize = 8;
const RELATED_CHARS: usize = 300;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixInput {
    pub comment_id: String,
    pub author: String,
    pub initials: String,
    pub comment: String,
    /// Later messages in the thread: (author, text).
    pub replies: Vec<(String, String)>,
    /// The words the comment is attached to.
    pub quote: String,
    /// Text the user selected by hand because the reviewer's highlight
    /// missed part of what the comment is about.
    pub selection: Option<String>,
    /// Paragraphs to rewrite: (paragraph index, editable text).
    pub paragraphs: Vec<(usize, String)>,
    /// Headings above the comment, outermost first.
    pub heading_path: Vec<String>,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub mode: FixMode,
    /// What the user wants changed, steering the model ahead of everything
    /// but the no-fabrication rule.
    pub direction: Option<String>,
    /// Paragraphs elsewhere in the document about what the comment asks,
    /// where the project's own figures and methods are found.
    pub related: Vec<Related>,
    /// Material the user found and asked the fix to use.
    pub sources: Vec<FixSource>,
}

/// A paragraph from another part of the document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Related {
    pub index: usize,
    pub heading_path: Vec<String>,
    pub text: String,
}

/// Something the user found with 查找资料 and handed to the next fix.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FixSource {
    pub title: String,
    pub url: String,
    /// The answer or snippet shown to the user.
    pub text: String,
}

/// How far a rewrite may go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FixMode {
    /// Change only what the comment points out, as little as possible.
    #[default]
    Fix,
    /// The original text is wrong: rewrite the paragraphs as needed.
    Rewrite,
}

/// Paragraphs the user selected in the document to rewrite instead of the
/// ones under the comment's highlight.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub start_paragraph: usize,
    pub end_paragraph: usize,
    /// The selected text, for the model to know what exactly is meant.
    #[serde(default)]
    pub text: String,
}

/// What the user asked for beyond the comment itself.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FixRequest {
    pub selection: Option<Selection>,
    pub mode: FixMode,
    pub direction: Option<String>,
    /// Material found with 查找资料 for the model to use.
    pub sources: Vec<FixSource>,
}

/// Gathers the comment's context and applies the request to it.
pub fn gather_request(doc: &Document, comment_id: &str, request: &FixRequest) -> Result<FixInput> {
    let mut input = gather_with(doc, comment_id, request.selection.as_ref())?;
    input.mode = request.mode;
    input.direction = request
        .direction
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    input.sources = request
        .sources
        .iter()
        .filter(|s| !s.text.trim().is_empty() || !s.url.trim().is_empty())
        .take(8)
        .cloned()
        .collect();
    let mut asks = vec![input.comment.as_str()];
    asks.extend(input.direction.as_deref());
    asks.extend(input.replies.iter().map(|(_, t)| t.as_str()));
    input.related = related(doc, &asks.join("\n"), &input);
    Ok(input)
}

/// Terms of a text for matching: pairs of adjacent Han characters and
/// runs of letters and digits.
pub fn terms(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let chars: Vec<char> = text.chars().collect();
    for w in chars.windows(2) {
        if is_han(w[0]) && is_han(w[1]) {
            out.insert(w.iter().collect());
        }
    }
    for word in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '.') {
        let word = word.trim_matches('.');
        if word.len() >= 2 && word.chars().any(|c| c.is_ascii_alphabetic()) {
            out.insert(word.to_lowercase());
        }
    }
    out
}

/// Paragraphs outside the comment's range that share the ask's rarer terms,
/// best first. Terms common across the document count for little.
fn related(doc: &Document, ask: &str, input: &FixInput) -> Vec<Related> {
    let wanted = terms(ask);
    if wanted.is_empty() {
        return Vec::new();
    }
    let (first, last) = match (input.paragraphs.first(), input.paragraphs.last()) {
        (Some(f), Some(l)) => (f.0, l.0),
        _ => return Vec::new(),
    };
    let skip = first.saturating_sub(NEIGHBOURS)..=last + NEIGHBOURS;
    let texts: Vec<String> = (0..doc.paragraphs.len())
        .map(|i| doc.paragraphs[i].accepted_text())
        .collect();
    let mut df: HashMap<&str, usize> = HashMap::new();
    let found: Vec<HashSet<&str>> = texts
        .iter()
        .map(|t| {
            let have: HashSet<&str> = wanted
                .iter()
                .filter(|w| t.contains(w.as_str()))
                .map(String::as_str)
                .collect();
            for w in &have {
                *df.entry(w).or_default() += 1;
            }
            have
        })
        .collect();
    let n = texts.len().max(1) as f32;
    let weight = |w: &str| (n / (1.0 + df[w] as f32)).ln().max(0.0);
    let mut scored: Vec<(f32, usize)> = found
        .iter()
        .enumerate()
        .filter(|(i, have)| {
            !skip.contains(i) && have.len() >= 2 && texts[*i].trim().chars().count() >= 8
        })
        .map(|(i, have)| (have.iter().map(|w| weight(w)).sum::<f32>(), i))
        .filter(|(score, _)| *score > 2.0)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut picked: Vec<usize> = scored.iter().take(RELATED).map(|s| s.1).collect();
    picked.sort_unstable();
    picked
        .into_iter()
        .map(|i| {
            let mut text: String = texts[i].trim().chars().take(RELATED_CHARS).collect();
            if texts[i].trim().chars().count() > RELATED_CHARS {
                text.push('…');
            }
            Related {
                index: i,
                heading_path: headings_above(doc, i),
                text,
            }
        })
        .collect()
}

fn label_text(doc: &Document, i: usize) -> String {
    let p = &doc.paragraphs[i];
    match doc.list_label(i) {
        Some(label) => format!("{label} {}", p.accepted_text()),
        None => p.accepted_text(),
    }
}

/// Headings above paragraph `first`, outermost first.
fn headings_above(doc: &Document, first: usize) -> Vec<String> {
    let mut path = Vec::new();
    let mut level = u8::MAX;
    for i in (0..first).rev() {
        if let Some(l) = doc.heading_level(i)
            && l < level
        {
            path.push(label_text(doc, i).trim().to_string());
            level = l;
            if l == 0 {
                break;
            }
        }
    }
    path.reverse();
    path
}

pub fn gather(doc: &Document, comment_id: &str) -> Result<FixInput> {
    gather_with(doc, comment_id, None)
}

/// Like [`gather`], rewriting the selected paragraphs when there is a selection.
pub fn gather_with(
    doc: &Document,
    comment_id: &str,
    selection: Option<&Selection>,
) -> Result<FixInput> {
    let comment = doc
        .comment(comment_id)
        .ok_or_else(|| Error::Invalid(format!("找不到批注 {comment_id}")))?;
    let root = match &comment.parent_id {
        Some(p) => doc.comment(p).unwrap_or(comment),
        None => comment,
    };
    let anchor = root
        .anchor
        .as_ref()
        .ok_or_else(|| Error::Invalid("这条批注没有对应的正文位置".into()))?;
    let (first, last) = match selection {
        Some(s) => {
            let (first, last) = (
                s.start_paragraph.min(s.end_paragraph),
                s.start_paragraph.max(s.end_paragraph),
            );
            if last >= doc.paragraphs.len() {
                return Err(Error::Invalid("选中的范围超出了文档".into()));
            }
            if last + 1 - first > MAX_SELECTED_PARAGRAPHS {
                return Err(Error::Invalid(format!(
                    "选中了 {} 段，一次最多修改 {MAX_SELECTED_PARAGRAPHS} 段，请缩小选区",
                    last + 1 - first
                )));
            }
            (first, last)
        }
        None => (anchor.start_paragraph, anchor.end_paragraph),
    };
    if selection.is_none() && last + 1 - first > MAX_PARAGRAPHS {
        return Err(Error::Invalid(format!(
            "这条批注覆盖了 {} 段，超过 {MAX_PARAGRAPHS} 段，请手动修改或把批注拆细",
            last + 1 - first
        )));
    }

    let paragraphs = (first..=last).map(|i| (i, doc.editable_text(i))).collect();
    let replies = doc
        .comments
        .iter()
        .filter(|c| c.parent_id.as_deref() == Some(root.id.as_str()))
        .map(|c| (c.author.clone(), c.text.clone()))
        .collect();

    let text_of = |i: usize| label_text(doc, i);
    let heading_path = headings_above(doc, first);

    let non_empty = |i: &usize| !doc.paragraphs[*i].accepted_text().trim().is_empty();
    let mut before: Vec<String> = (0..first)
        .rev()
        .filter(non_empty)
        .take(NEIGHBOURS)
        .map(text_of)
        .collect();
    before.reverse();
    let after = (last + 1..doc.paragraphs.len())
        .filter(non_empty)
        .take(NEIGHBOURS)
        .map(text_of)
        .collect();

    Ok(FixInput {
        comment_id: root.id.clone(),
        author: root.author.clone(),
        initials: root.initials.clone().unwrap_or_default(),
        comment: root.text.clone(),
        replies,
        quote: quote(doc, anchor),
        selection: selection
            .map(|s| s.text.trim().to_string())
            .filter(|t| !t.is_empty()),
        paragraphs,
        heading_path,
        before,
        after,
        mode: FixMode::Fix,
        direction: None,
        related: Vec::new(),
        sources: Vec::new(),
    })
}

fn quote(doc: &Document, a: &docx_engine::CommentAnchor) -> String {
    let mut out = String::new();
    for pi in a.start_paragraph..=a.end_paragraph {
        let p = &doc.paragraphs[pi];
        let chars: Vec<char> = p.text.chars().collect();
        let start = if pi == a.start_paragraph {
            a.start_offset
        } else {
            0
        };
        let end = if pi == a.end_paragraph {
            a.end_offset
        } else {
            chars.len()
        };
        for run in &p.runs {
            if run.revision == Revision::Delete || run.inline != Inline::Text {
                continue;
            }
            let (s, e) = (run.start.max(start), run.end.min(end));
            if s < e {
                out.extend(&chars[s..e]);
            }
        }
        if pi != a.end_paragraph {
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_engine::testgen::{Spec, generate};

    #[test]
    fn terms_are_han_pairs_and_words() {
        let t = terms("PM2.5 测算方法");
        assert!(t.contains("pm2.5") && t.contains("测算") && t.contains("算方"));
        assert!(!t.contains("5 "));
    }

    #[test]
    fn gathers_comment_context() {
        let doc = Document::from_bytes(generate(&Spec {
            target_chars: 20_000,
            comments: 20,
            seed: 5,
        }))
        .unwrap();
        let c = doc
            .comments
            .iter()
            .find(|c| c.parent_id.is_none() && c.anchor.is_some())
            .unwrap();
        let input = gather(&doc, &c.id).unwrap();
        assert_eq!(input.comment, c.text);
        assert!(!input.paragraphs.is_empty());
        assert!(!input.quote.is_empty());
        assert!(
            !input.heading_path.is_empty(),
            "comments sit under a heading"
        );
        let joined: String = input.paragraphs.iter().map(|p| p.1.as_str()).collect();
        assert!(joined.contains(input.quote.lines().next().unwrap()));

        // A hand-made selection replaces the highlighted paragraphs.
        let anchor = c.anchor.as_ref().unwrap();
        let end = (anchor.end_paragraph + 2).min(doc.paragraphs.len() - 1);
        let sel = Selection {
            start_paragraph: anchor.start_paragraph,
            end_paragraph: end,
            text: " 选中的文字 ".into(),
        };
        let wide = gather_with(&doc, &c.id, Some(&sel)).unwrap();
        let indices: Vec<usize> = wide.paragraphs.iter().map(|p| p.0).collect();
        assert_eq!(indices, (anchor.start_paragraph..=end).collect::<Vec<_>>());
        assert_eq!(wide.selection.as_deref(), Some("选中的文字"));
        let too_many = Selection {
            start_paragraph: 0,
            end_paragraph: MAX_SELECTED_PARAGRAPHS,
            text: String::new(),
        };
        assert!(gather_with(&doc, &c.id, Some(&too_many)).is_err());

        // Other sections that share the ask's terms come along.
        let far = (anchor.end_paragraph + 10..doc.paragraphs.len())
            .find(|&i| doc.paragraphs[i].accepted_text().trim().chars().count() >= 30)
            .unwrap();
        let words: String = doc.paragraphs[far]
            .accepted_text()
            .trim()
            .chars()
            .take(16)
            .collect();
        let request = FixRequest {
            direction: Some(format!("结合{words}说明")),
            ..FixRequest::default()
        };
        let input = gather_request(&doc, &c.id, &request).unwrap();
        assert!(
            input.related.iter().any(|r| r.index == far),
            "{:?}",
            input.related
        );
        assert!(input.related.len() <= RELATED);

        // A reply resolves to its thread.
        if let Some(reply) = doc.comments.iter().find(|c| c.parent_id.is_some()) {
            let input = gather(&doc, &reply.id).unwrap();
            assert_eq!(Some(input.comment_id), reply.parent_id.clone());
        }
    }
}
