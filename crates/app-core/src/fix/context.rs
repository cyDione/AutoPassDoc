//! What the model sees about a comment: the comment thread, the paragraphs
//! it covers (as editable text), the quoted words, the section it sits in
//! and the neighbouring paragraphs.

use docx_engine::{Document, Inline, Revision};
use serde::Serialize;

use crate::error::{Error, Result};

/// Comments spanning more paragraphs than this are left to the user.
pub const MAX_PARAGRAPHS: usize = 6;
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
    /// Paragraphs to rewrite: (paragraph index, editable text).
    pub paragraphs: Vec<(usize, String)>,
    /// Headings above the comment, outermost first.
    pub heading_path: Vec<String>,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

pub fn gather(doc: &Document, comment_id: &str) -> Result<FixInput> {
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
    let (first, last) = (anchor.start_paragraph, anchor.end_paragraph);
    if last + 1 - first > MAX_PARAGRAPHS {
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

        // A reply resolves to its thread.
        if let Some(reply) = doc.comments.iter().find(|c| c.parent_id.is_some()) {
            let input = gather(&doc, &reply.id).unwrap();
            assert_eq!(Some(input.comment_id), reply.parent_id.clone());
        }
    }
}
