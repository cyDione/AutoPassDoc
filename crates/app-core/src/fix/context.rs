//! What the model sees about a comment: the comment thread, the paragraphs
//! it covers (as editable text), the quoted words, the section it sits in
//! and the neighbouring paragraphs.

use docx_engine::{Document, Inline, Revision};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Comments spanning more paragraphs than this are left to the user.
pub const MAX_PARAGRAPHS: usize = 6;
/// A range the user selected by hand may be larger.
pub const MAX_SELECTED_PARAGRAPHS: usize = 12;
const NEIGHBOURS: usize = 2;

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

    let text_of = |i: usize| {
        let p = &doc.paragraphs[i];
        match doc.list_label(i) {
            Some(label) => format!("{label} {}", p.accepted_text()),
            None => p.accepted_text(),
        }
    };
    let mut heading_path = Vec::new();
    let mut level = u8::MAX;
    for i in (0..first).rev() {
        if let Some(l) = doc.heading_level(i)
            && l < level
        {
            heading_path.push(text_of(i).trim().to_string());
            level = l;
            if l == 0 {
                break;
            }
        }
    }
    heading_path.reverse();

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

        // A reply resolves to its thread.
        if let Some(reply) = doc.comments.iter().find(|c| c.parent_id.is_some()) {
            let input = gather(&doc, &reply.id).unwrap();
            assert_eq!(Some(input.comment_id), reply.parent_id.clone());
        }
    }
}
