//! Comments from `word/comments.xml`, reply threads and resolved state from
//! `word/commentsExtended.xml`, and anchors from the markers in the body.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::{Error, Result};
use crate::model::{Comment, CommentAnchor, MarkerKind, Paragraph};
use crate::xml::{attr, local, read_text};

pub fn parse_comments(xml: &str) -> Result<Vec<Comment>> {
    const PART: &str = "word/comments.xml";
    let mut reader = Reader::from_str(xml);
    let mut comments = Vec::new();
    let mut current: Option<Comment> = None;
    let mut paragraphs: Vec<String> = Vec::new();
    loop {
        match reader.read_event().map_err(|e| Error::xml(PART, e))? {
            Event::Start(e) => match local(&e).as_str() {
                "comment" => {
                    paragraphs.clear();
                    current = Some(Comment {
                        id: attr(&e, "id").unwrap_or_default(),
                        author: attr(&e, "author").unwrap_or_default(),
                        initials: attr(&e, "initials").filter(|s| !s.is_empty()),
                        date: attr(&e, "date").filter(|s| !s.is_empty()),
                        text: String::new(),
                        para_id: None,
                        parent_id: None,
                        done: false,
                        anchor: None,
                    });
                }
                "p" if current.is_some() => {
                    paragraphs.push(String::new());
                    if let Some(id) = attr(&e, "paraId") {
                        current.as_mut().unwrap().para_id = Some(id);
                    }
                }
                "t" if current.is_some() => {
                    let text = read_text(&mut reader, &e, PART)?;
                    if paragraphs.is_empty() {
                        paragraphs.push(String::new());
                    }
                    paragraphs.last_mut().unwrap().push_str(&text);
                }
                "instrText" | "delText" => {
                    reader
                        .read_to_end(e.name())
                        .map_err(|err| Error::xml(PART, err))?;
                }
                _ => {}
            },
            Event::Empty(e) => match local(&e).as_str() {
                "p" if current.is_some() => {
                    paragraphs.push(String::new());
                    if let Some(id) = attr(&e, "paraId") {
                        current.as_mut().unwrap().para_id = Some(id);
                    }
                }
                "tab" | "br" if current.is_some() => {
                    if let Some(last) = paragraphs.last_mut() {
                        last.push(if local(&e) == "tab" { '\t' } else { '\n' });
                    }
                }
                _ => {}
            },
            Event::End(e) if e.local_name().as_ref() == "comment" => {
                if let Some(mut comment) = current.take() {
                    comment.text = paragraphs.join("\n").trim().to_string();
                    comments.push(comment);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(comments)
}

/// Applies `commentsExtended.xml`: reply parent links and the resolved flag.
pub fn apply_extended(xml: &str, comments: &mut [Comment]) -> Result<()> {
    const PART: &str = "word/commentsExtended.xml";
    let by_para: HashMap<String, usize> = comments
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.para_id.clone().map(|p| (p, i)))
        .collect();
    let mut links = Vec::new();
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|e| Error::xml(PART, e))? {
            Event::Start(e) | Event::Empty(e) if local(&e) == "commentEx" => {
                let Some(index) = attr(&e, "paraId").and_then(|p| by_para.get(&p).copied()) else {
                    continue;
                };
                comments[index].done = attr(&e, "done").is_some_and(|v| v == "1" || v == "true");
                if let Some(parent) =
                    attr(&e, "paraIdParent").and_then(|p| by_para.get(&p).copied())
                {
                    links.push((index, comments[parent].id.clone()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    for (index, parent_id) in links {
        comments[index].parent_id = Some(parent_id);
    }
    Ok(())
}

/// Resolves each comment's anchor from the range markers found in the body.
/// Comments without a range fall back to the position of their reference mark.
pub fn resolve_anchors(comments: &mut [Comment], paragraphs: &[Paragraph]) {
    let mut starts = HashMap::new();
    let mut ends = HashMap::new();
    let mut refs = HashMap::new();
    for p in paragraphs {
        for m in &p.markers {
            let target = match m.kind {
                MarkerKind::CommentStart => &mut starts,
                MarkerKind::CommentEnd => &mut ends,
                MarkerKind::CommentReference => &mut refs,
            };
            target.entry(m.id.as_str()).or_insert((p.index, m.offset));
        }
    }
    for comment in comments.iter_mut() {
        let id = comment.id.as_str();
        let (start, end) = match (starts.get(id), ends.get(id), refs.get(id)) {
            (Some(&s), Some(&e), _) if e >= s => (s, e),
            (Some(&s), _, _) => (s, (s.0, paragraphs[s.0].char_len())),
            (None, Some(&e), _) => ((e.0, 0), e),
            (None, None, Some(&r)) => (r, r),
            (None, None, None) => continue,
        };
        comment.anchor = Some(CommentAnchor {
            start_paragraph: start.0,
            start_offset: start.1,
            end_paragraph: end.0,
            end_offset: end.1,
        });
    }
    // Replies usually carry their own copy of the parent's range; if not, share it.
    let anchors: HashMap<String, CommentAnchor> = comments
        .iter()
        .filter_map(|c| c.anchor.clone().map(|a| (c.id.clone(), a)))
        .collect();
    for comment in comments.iter_mut() {
        if comment.anchor.is_none()
            && let Some(parent) = comment.parent_id.as_ref().and_then(|p| anchors.get(p))
        {
            comment.anchor = Some(parent.clone());
        }
    }
}
