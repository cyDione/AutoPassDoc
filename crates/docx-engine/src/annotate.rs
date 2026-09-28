//! Comment write-back: resolved state, author names and replies.
//!
//! Comments live in `word/comments.xml`; replies and the resolved flag live
//! in `word/commentsExtended.xml`, keyed by the `w14:paraId` of each
//! comment's last paragraph. Edits here patch start tags in place and append
//! new elements, leaving the rest of each part untouched.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::edit::{escape_attr, escape_text};
use crate::error::{Error, Result};
use crate::xml::{attr, local};

pub const COMMENTS_PART: &str = "word/comments.xml";
pub const EXTENDED_PART: &str = "word/commentsExtended.xml";
pub const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
pub const DOCUMENT_RELS_PART: &str = "word/_rels/document.xml.rels";

const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const EXTENDED_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml";
const EXTENDED_REL: &str =
    "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";

/// Rewrites a start tag with attributes set (`Some`) or removed (`None`).
/// Attributes are matched by local name, so `w:author` also finds `author`
/// written with another prefix.
pub fn set_attrs(tag: &str, changes: &[(&str, Option<&str>)]) -> Result<String> {
    let mut reader = Reader::from_str(tag);
    let (e, empty) = match reader.read_event() {
        Ok(Event::Start(e)) => (e, false),
        Ok(Event::Empty(e)) => (e, true),
        _ => return Err(Error::Invalid(format!("无法解析标签 {tag}"))),
    };
    let local_of = |q: &str| q.rsplit(':').next().unwrap_or(q).to_string();
    let mut out = format!("<{}", e.name().as_ref());
    let mut done = HashSet::new();
    for a in e.attributes().with_checks(false).flatten() {
        let key = a.key.as_ref().to_string();
        let key_local = a.key.local_name().as_ref().to_string();
        let change = changes
            .iter()
            .find(|(q, _)| !key.starts_with("xmlns") && local_of(q) == key_local);
        let value = match change {
            Some((q, v)) => {
                done.insert(*q);
                match v {
                    Some(v) => v.to_string(),
                    None => continue,
                }
            }
            None => a
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_default(),
        };
        out.push_str(&format!(" {key}=\"{}\"", escape_attr(&value)));
    }
    for (q, v) in changes {
        if let (false, Some(v)) = (done.contains(q), v) {
            out.push_str(&format!(" {q}=\"{}\"", escape_attr(v)));
        }
    }
    out.push_str(if empty { "/>" } else { ">" });
    Ok(out)
}

/// Applies `(range, replacement)` edits to `xml`; ranges must not overlap.
pub fn splice(xml: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(xml.len() + edits.iter().map(|e| e.1.len()).sum::<usize>());
    let mut at = 0;
    for (range, text) in edits {
        out.push_str(&xml[at..range.start]);
        out.push_str(&text);
        at = range.end;
    }
    out.push_str(&xml[at..]);
    out
}

/// Byte range and qualified name of the root element's start tag, and the
/// offset of its end tag.
fn root(xml: &str, part: &str) -> Result<(Range<usize>, String, usize)> {
    let mut reader = Reader::from_str(xml);
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event().map_err(|e| Error::xml(part, e))? {
            Event::Start(e) => {
                let name = e.name().as_ref().to_string();
                let end_tag = xml
                    .rfind("</")
                    .ok_or_else(|| Error::xml(part, "missing root end tag"))?;
                return Ok((start..reader.buffer_position() as usize, name, end_tag));
            }
            Event::Empty(_) => {
                return Err(Error::xml(part, "empty root element"));
            }
            Event::Eof => return Err(Error::xml(part, "missing root element")),
            _ => {}
        }
    }
}

fn prefix_of(qname: &str) -> &str {
    qname.split_once(':').map_or("", |(p, _)| p)
}

fn q(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}:{name}")
    }
}

/// Prefix bound to `uri` on a start tag, if any.
fn ns_prefix(tag: &str, uri: &str) -> Option<String> {
    let needle = format!("=\"{uri}\"");
    let at = tag.find(&needle)?;
    let before = &tag[..at];
    let decl = before.rsplit(char::is_whitespace).next()?;
    decl.strip_prefix("xmlns:").map(str::to_string)
}

/// Unique `w14:paraId` values (8 hex digits below 0x80000000).
pub struct ParaIds {
    used: HashSet<String>,
    state: u64,
}

impl ParaIds {
    pub fn new(parts: &[&str]) -> Self {
        let mut used = HashSet::new();
        for xml in parts {
            let mut rest = *xml;
            while let Some(i) = rest.find("paraId=\"") {
                let v = &rest[i + 8..];
                if let Some(end) = v.find('"') {
                    used.insert(v[..end].to_ascii_uppercase());
                }
                rest = &rest[i + 8..];
            }
        }
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Self {
            used,
            state: seed | 1,
        }
    }

    pub fn next(&mut self) -> String {
        loop {
            // xorshift64*
            self.state ^= self.state >> 12;
            self.state ^= self.state << 25;
            self.state ^= self.state >> 27;
            let v = (self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u32 & 0x7FFF_FFFF;
            let id = format!("{v:08X}");
            if v != 0 && self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

struct CommentTag {
    id: String,
    tag: Range<usize>,
    /// Start tag of the comment's last paragraph.
    last_p: Option<Range<usize>>,
    end: usize,
}

fn comment_tags(xml: &str) -> Result<Vec<CommentTag>> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut current: Option<CommentTag> = None;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|e| Error::xml(COMMENTS_PART, e))?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(e) if local(&e) == "comment" => {
                current = Some(CommentTag {
                    id: attr(&e, "id").unwrap_or_default(),
                    tag: start..end,
                    last_p: None,
                    end,
                });
            }
            Event::Start(e) | Event::Empty(e) if local(&e) == "p" => {
                if let Some(c) = current.as_mut() {
                    c.last_p = Some(start..end);
                }
            }
            Event::End(e) if e.local_name().as_ref() == "comment" => {
                if let Some(mut c) = current.take() {
                    c.end = start;
                    out.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// Gives the listed comments a `w14:paraId` on their last paragraph where
/// they lack one. Returns the new XML and the id → paraId assignments.
pub fn ensure_para_ids(
    xml: &str,
    ids: &HashSet<&str>,
    para_ids: &mut ParaIds,
) -> Result<(String, HashMap<String, String>)> {
    let (root_tag, _, _) = root(xml, COMMENTS_PART)?;
    let root_xml = &xml[root_tag.clone()];
    let (w14, declare) = match ns_prefix(root_xml, W14) {
        Some(p) => (p, false),
        None => ("w14".to_string(), true),
    };
    let key = format!("{w14}:paraId");
    let mut edits = Vec::new();
    let mut assigned = HashMap::new();
    for c in comment_tags(xml)? {
        if !ids.contains(c.id.as_str()) {
            continue;
        }
        let id = para_ids.next();
        match c.last_p {
            Some(p) => edits.push((
                p.clone(),
                set_attrs(&xml[p], &[(key.as_str(), Some(id.as_str()))])?,
            )),
            None => {
                let prefix = prefix_of(
                    xml[c.tag.clone()]
                        .trim_start_matches('<')
                        .split(char::is_whitespace)
                        .next()
                        .unwrap_or(""),
                )
                .to_string();
                edits.push((
                    c.end..c.end,
                    format!("<{} {key}=\"{id}\"/>", q(&prefix, "p")),
                ));
            }
        }
        assigned.insert(c.id, id);
    }
    if assigned.is_empty() {
        return Ok((xml.to_string(), assigned));
    }
    if declare {
        edits.push((
            root_tag.clone(),
            set_attrs(root_xml, &[("xmlns:w14", Some(W14))])?,
        ));
    }
    Ok((splice(xml, edits), assigned))
}

/// Sets author and initials on the listed comments.
pub fn set_authors(
    xml: &str,
    ids: &HashSet<&str>,
    author: &str,
    initials: Option<&str>,
) -> Result<String> {
    let mut edits = Vec::new();
    for c in comment_tags(xml)? {
        if ids.contains(c.id.as_str()) {
            let tag = &xml[c.tag.clone()];
            let new = match initials {
                Some(i) => set_attrs(tag, &[("w:author", Some(author)), ("w:initials", Some(i))])?,
                None => set_attrs(tag, &[("w:author", Some(author))])?,
            };
            edits.push((c.tag, new));
        }
    }
    Ok(splice(xml, edits))
}

pub struct NewComment<'a> {
    pub id: &'a str,
    pub author: &'a str,
    pub initials: Option<&'a str>,
    pub date: &'a str,
    /// Each line becomes a paragraph.
    pub text: &'a str,
    /// `w14:paraId` of the last paragraph, which commentsExtended refers to.
    pub para_id: &'a str,
}

/// Appends a comment to comments.xml.
pub fn append_comment(xml: &str, c: &NewComment<'_>, para_ids: &mut ParaIds) -> Result<String> {
    let NewComment {
        id,
        author,
        initials,
        date,
        text,
        para_id: last_para_id,
    } = *c;
    let (root_tag, root_name, end_tag) = root(xml, COMMENTS_PART)?;
    let root_xml = &xml[root_tag.clone()];
    let w = prefix_of(&root_name);
    let (w14, declare) = match ns_prefix(root_xml, W14) {
        Some(p) => (p, false),
        None => ("w14".to_string(), true),
    };
    let a = if w.is_empty() { "w" } else { w };
    let mut comment = format!(
        "<{} {a}:id=\"{id}\" {a}:author=\"{}\" {a}:date=\"{date}\"",
        q(w, "comment"),
        escape_attr(author)
    );
    if let Some(i) = initials {
        comment.push_str(&format!(" {a}:initials=\"{}\"", escape_attr(i)));
    }
    comment.push('>');
    let lines: Vec<&str> = text.lines().collect();
    let lines = if lines.is_empty() { vec![""] } else { lines };
    for (n, line) in lines.iter().enumerate() {
        let para_id = if n + 1 == lines.len() {
            last_para_id.to_string()
        } else {
            para_ids.next()
        };
        comment.push_str(&format!("<{} {w14}:paraId=\"{para_id}\">", q(w, "p")));
        if n == 0 {
            comment.push_str(&format!(
                "<{r}><{ann}/></{r}>",
                r = q(w, "r"),
                ann = q(w, "annotationRef")
            ));
        }
        if !line.is_empty() {
            comment.push_str(&format!(
                "<{r}><{t} xml:space=\"preserve\">{}</{t}></{r}>",
                escape_text(line),
                r = q(w, "r"),
                t = q(w, "t")
            ));
        }
        comment.push_str(&format!("</{}>", q(w, "p")));
    }
    comment.push_str(&format!("</{}>", q(w, "comment")));

    let mut edits = vec![(end_tag..end_tag, comment)];
    if declare {
        edits.push((root_tag, set_attrs(root_xml, &[("xmlns:w14", Some(W14))])?));
    }
    Ok(splice(xml, edits))
}

pub fn new_extended_part() -> String {
    concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n",
        "<w15:commentsEx xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" ",
        "xmlns:w15=\"http://schemas.microsoft.com/office/word/2012/wordml\" mc:Ignorable=\"w15\">",
        "</w15:commentsEx>"
    )
    .to_string()
}

/// Registers a newly created commentsExtended part in the content types and
/// the document relationships. Returns the updated parts.
pub fn register_extended(content_types: &str, rels: &str) -> Result<(String, String)> {
    let content_types = if content_types.contains("/word/commentsExtended.xml\"") {
        content_types.to_string()
    } else {
        let (_, _, end) = root(content_types, CONTENT_TYPES_PART)?;
        splice(
            content_types,
            vec![(
                end..end,
                format!(
                    "<Override PartName=\"/word/commentsExtended.xml\" ContentType=\"{EXTENDED_TYPE}\"/>"
                ),
            )],
        )
    };
    let rels = if rels.contains(EXTENDED_REL) {
        rels.to_string()
    } else {
        let (_, _, end) = root(rels, DOCUMENT_RELS_PART)?;
        let mut n = 1;
        while rels.contains(&format!("Id=\"rIdApd{n}\"")) {
            n += 1;
        }
        splice(
            rels,
            vec![(
                end..end,
                format!(
                    "<Relationship Id=\"rIdApd{n}\" Type=\"{EXTENDED_REL}\" Target=\"commentsExtended.xml\"/>"
                ),
            )],
        )
    };
    Ok((content_types, rels))
}

/// An entry of commentsExtended.xml to create or update.
pub struct ExtendedEntry<'a> {
    pub para_id: &'a str,
    /// Only used when the entry is created.
    pub parent_para_id: Option<&'a str>,
    pub done: bool,
}

pub fn set_extended(xml: &str, entries: &[ExtendedEntry<'_>]) -> Result<String> {
    let (_, root_name, end_tag) = root(xml, EXTENDED_PART)?;
    let p = prefix_of(&root_name).to_string();
    let mut edits = Vec::new();
    let mut found = HashSet::new();
    let mut reader = Reader::from_str(xml);
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|e| Error::xml(EXTENDED_PART, e))?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(e) | Event::Empty(e) if local(&e) == "commentEx" => {
                let Some(pid) = attr(&e, "paraId") else {
                    continue;
                };
                if let Some(entry) = entries
                    .iter()
                    .find(|en| en.para_id.eq_ignore_ascii_case(&pid))
                {
                    found.insert(entry.para_id);
                    let done_key = q(&p, "done");
                    edits.push((
                        start..end,
                        set_attrs(
                            &xml[start..end],
                            &[(done_key.as_str(), Some(if entry.done { "1" } else { "0" }))],
                        )?,
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut appended = String::new();
    for entry in entries.iter().filter(|e| !found.contains(e.para_id)) {
        appended.push_str(&format!(
            "<{} {}=\"{}\"",
            q(&p, "commentEx"),
            q(&p, "paraId"),
            entry.para_id
        ));
        if let Some(parent) = entry.parent_para_id {
            appended.push_str(&format!(" {}=\"{parent}\"", q(&p, "paraIdParent")));
        }
        appended.push_str(&format!(
            " {}=\"{}\"/>",
            q(&p, "done"),
            if entry.done { "1" } else { "0" }
        ));
    }
    if !appended.is_empty() {
        edits.push((end_tag..end_tag, appended));
    }
    Ok(splice(xml, edits))
}

/// Adds range and reference marks for reply `new_id` next to those of
/// comment `parent_id` in document.xml, so Word keeps the reply.
pub fn add_reply_marks(doc_xml: &str, parent_id: &str, new_id: &str) -> Result<String> {
    const PART: &str = crate::DOCUMENT_PART;
    let mut reader = Reader::from_str(doc_xml);
    let mut edits = Vec::new();
    let mut in_reference_run = false;
    let mut have_reference = false;
    let mut range_end: Option<usize> = None;
    let mut prefix = String::from("w");
    let tag = |name: &str, prefix: &str| {
        format!("<{} {}=\"{new_id}\"/>", q(prefix, name), q(prefix, "id"))
    };
    loop {
        let event = reader.read_event().map_err(|e| Error::xml(PART, e))?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Empty(e) if attr(&e, "id").as_deref() == Some(parent_id) => {
                prefix = prefix_of(e.name().as_ref()).to_string();
                match local(&e).as_str() {
                    "commentRangeStart" => {
                        edits.push((end..end, tag("commentRangeStart", &prefix)))
                    }
                    "commentRangeEnd" => {
                        edits.push((end..end, tag("commentRangeEnd", &prefix)));
                        range_end = Some(end);
                    }
                    "commentReference" => in_reference_run = true,
                    _ => {}
                }
            }
            Event::End(e) if in_reference_run && e.local_name().as_ref() == "r" => {
                in_reference_run = false;
                have_reference = true;
                edits.push((end..end, reference_run(&prefix, new_id)));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !have_reference {
        let at = range_end.ok_or_else(|| Error::Edit("找不到原批注在正文中的位置".into()))?;
        edits.push((at..at, reference_run(&prefix, new_id)));
    }
    Ok(splice(doc_xml, edits))
}

fn reference_run(prefix: &str, id: &str) -> String {
    format!(
        "<{r}><{rpr}><{rs} {val}=\"CommentReference\"/></{rpr}><{cr} {idk}=\"{id}\"/></{r}>",
        r = q(prefix, "r"),
        rpr = q(prefix, "rPr"),
        rs = q(prefix, "rStyle"),
        val = q(prefix, "val"),
        cr = q(prefix, "commentReference"),
        idk = q(prefix, "id"),
    )
}
