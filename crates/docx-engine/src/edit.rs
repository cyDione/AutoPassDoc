//! Text edits written back into `word/document.xml`.
//!
//! An edit replaces a paragraph's *editable text* (its text with tracked
//! deletions left out and images, notes and equations shown as placeholders
//! such as `⟦图⟧`). The old and new text are diffed, and only the `<w:r>`
//! elements that the changes touch are rewritten: split where a change starts
//! or ends, with deleted text wrapped in `<w:del>` and new text in `<w:ins>`
//! (tracked mode) or simply removed and added (direct mode). Everything else
//! in the paragraph, including comment marks and bookmarks, keeps its bytes.

use std::collections::BTreeMap;
use std::ops::Range;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

use crate::DOCUMENT_PART;
use crate::body::fragment_chars;
use crate::diff::{hunks, tokenize};
use crate::error::{Error, Result};
use crate::model::{Inline, Paragraph, Revision, XmlRun};
use crate::xml::{local, read_text};

pub const IMAGE_PLACEHOLDER: &str = "⟦图⟧";
pub const MATH_PLACEHOLDER: &str = "⟦公式⟧";

pub fn note_placeholder(id: &str) -> String {
    format!("⟦注{id}⟧")
}

fn is_placeholder(token: &str) -> bool {
    token.starts_with('⟦') && token.ends_with('⟧') && token.chars().count() > 2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    /// Word tracked changes (`w:ins` / `w:del`), the default.
    Tracked,
    /// Replace the text without revision marks.
    Direct,
}

#[derive(Debug, Clone)]
pub struct EditOptions {
    pub mode: EditMode,
    /// Revision author shown in Word.
    pub author: String,
    /// ISO 8601 UTC timestamp, e.g. from [`now_iso`].
    pub date: String,
}

impl EditOptions {
    pub fn new(mode: EditMode, author: impl Into<String>) -> Self {
        Self {
            mode,
            author: author.into(),
            date: now_iso(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EditReport {
    pub paragraphs: usize,
    pub deleted_chars: usize,
    pub inserted_chars: usize,
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// One unit of editable text: a character, or a placeholder for an image,
/// note or equation, with the paragraph characters it stands for.
struct Unit {
    display: Range<usize>,
    placeholder: bool,
}

pub(crate) struct EditMap {
    pub text: String,
    units: Vec<Unit>,
    /// Unit of each editable character.
    unit_of_char: Vec<usize>,
}

impl EditMap {
    pub fn new(p: &Paragraph) -> Self {
        let chars: Vec<char> = p.text.chars().collect();
        let mut map = EditMap {
            text: String::new(),
            units: Vec::new(),
            unit_of_char: Vec::new(),
        };
        for run in p.runs.iter().filter(|r| r.revision != Revision::Delete) {
            let placeholder = match &run.inline {
                Inline::Text => {
                    for (d, c) in chars.iter().enumerate().take(run.end).skip(run.start) {
                        map.push(c.encode_utf8(&mut [0; 4]), d..d + 1, false);
                    }
                    continue;
                }
                Inline::Image { .. } => IMAGE_PLACEHOLDER.to_string(),
                Inline::Note { id } => note_placeholder(id),
                Inline::Math => MATH_PLACEHOLDER.to_string(),
            };
            map.push(&placeholder, run.start..run.end, true);
        }
        map
    }

    fn push(&mut self, text: &str, display: Range<usize>, placeholder: bool) {
        self.text.push_str(text);
        self.unit_of_char
            .extend(std::iter::repeat_n(self.units.len(), text.chars().count()));
        self.units.push(Unit {
            display,
            placeholder,
        });
    }
}

/// A change in paragraph (display) character offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    Delete(Range<usize>),
    Insert(usize, String),
}

/// Removes characters that XML 1.0 cannot carry and normalises line ends.
pub(crate) fn sanitize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|&c| matches!(c, '\t' | '\n') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}'))
        .collect()
}

pub(crate) fn ops_for(p: &Paragraph, new_text: &str) -> Result<Vec<Op>> {
    let map = EditMap::new(p);
    let new_text = sanitize(new_text);
    let old_tokens = tokenize(&map.text);
    let new_tokens = tokenize(&new_text);
    let mut token_chars = Vec::with_capacity(old_tokens.len());
    let mut at = 0;
    for t in &old_tokens {
        let n = t.chars().count();
        token_chars.push(at..at + n);
        at += n;
    }

    let mut ops = Vec::new();
    for h in hunks(&old_tokens, &new_tokens) {
        let inserted = &new_tokens[h.new.clone()];
        if inserted.iter().any(|t| is_placeholder(t)) {
            return Err(Error::Edit(
                "修改移动或新增了图片、脚注或公式，请手动处理".into(),
            ));
        }
        let mut units: Vec<usize> = Vec::new();
        if !h.old.is_empty() {
            let chars = token_chars[h.old.start].start..token_chars[h.old.end - 1].end;
            for c in chars {
                let u = map.unit_of_char[c];
                if units.last() != Some(&u) {
                    units.push(u);
                }
            }
        }
        if units.iter().any(|&u| map.units[u].placeholder) {
            return Err(Error::Edit("修改删除了图片、脚注或公式，请手动处理".into()));
        }
        let at = if let Some(&last) = units.last() {
            map.units[last].display.end
        } else if h.old.start > 0 {
            let c = token_chars[h.old.start - 1].end - 1;
            map.units[map.unit_of_char[c]].display.end
        } else {
            map.units.first().map_or(p.char_len(), |u| u.display.start)
        };
        let mut deletes: Vec<Range<usize>> = Vec::new();
        for u in units {
            let d = map.units[u].display.clone();
            match deletes.last_mut() {
                Some(last) if last.end == d.start => last.end = d.end,
                _ => deletes.push(d),
            }
        }
        ops.extend(deletes.into_iter().map(Op::Delete));
        let text = inserted.concat();
        if !text.is_empty() {
            ops.push(Op::Insert(at, text));
        }
    }
    Ok(ops)
}

#[derive(Default)]
struct RunEdits {
    deletes: Vec<Range<usize>>,
    inserts: Vec<(usize, String)>,
}

/// Allocates `w:id` values for new revision marks.
pub(crate) struct Ids(pub u64);

impl Ids {
    /// Starts above every numeric `:id="…"` in the given parts.
    pub fn above(parts: &[&str]) -> Self {
        let mut max = 0;
        for xml in parts {
            let mut rest = *xml;
            while let Some(i) = rest.find(":id=\"") {
                let digits = &rest[i + 5..];
                let n = digits.bytes().take_while(u8::is_ascii_digit).count();
                if n > 0 && digits.as_bytes().get(n) == Some(&b'"') {
                    max = max.max(digits[..n].parse().unwrap_or(0));
                }
                rest = &rest[i + 5..];
            }
        }
        Ids(max + 1)
    }

    fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0 - 1
    }
}

/// New XML for paragraph `p` of `doc_xml` with `ops` applied.
pub(crate) fn rewrite_paragraph(
    doc_xml: &str,
    p: &Paragraph,
    ops: &[Op],
    opts: &EditOptions,
    ids: &mut Ids,
) -> Result<String> {
    let para_xml = &doc_xml[p.span.start..p.span.end];
    let editable = |r: &XmlRun| r.revision == Revision::None && r.end > r.start;
    let overlap = || Error::Edit("要修改的文字里已有修订，请先在 Word 中接受或拒绝这些修订".into());

    let mut edits: BTreeMap<usize, RunEdits> = BTreeMap::new();
    let mut tail = Vec::new();
    for op in ops {
        match op {
            Op::Delete(range) => {
                let mut covered = 0;
                for (i, r) in p.xml_runs.iter().enumerate() {
                    let (s, e) = (range.start.max(r.start), range.end.min(r.end));
                    if s >= e {
                        continue;
                    }
                    if r.revision != Revision::None {
                        return Err(overlap());
                    }
                    edits
                        .entry(i)
                        .or_default()
                        .deletes
                        .push(s - r.start..e - r.start);
                    covered += e - s;
                }
                if covered != range.len() {
                    return Err(Error::Edit("要修改的位置包含无法编辑的内容".into()));
                }
            }
            Op::Insert(at, text) => {
                let runs = &p.xml_runs;
                let (host, local) = if let Some(i) =
                    runs.iter().position(|r| r.start < *at && *at < r.end)
                {
                    if !editable(&runs[i]) {
                        return Err(overlap());
                    }
                    (Some(i), at - runs[i].start)
                } else if let Some(i) = runs.iter().rposition(|r| r.end == *at && editable(r)) {
                    (Some(i), runs[i].end - runs[i].start)
                } else if let Some(i) = runs.iter().position(|r| r.start == *at && editable(r)) {
                    (Some(i), 0)
                } else if *at >= p.char_len() || !runs.iter().any(editable) {
                    (None, 0)
                } else {
                    return Err(overlap());
                };
                match host {
                    Some(i) => edits
                        .entry(i)
                        .or_default()
                        .inserts
                        .push((local, text.clone())),
                    None => tail.push(text.as_str()),
                }
            }
        }
    }

    let mut out = String::with_capacity(para_xml.len() + 256);
    let mut copied = p.span.start;
    for (i, run_edits) in edits {
        let r = &p.xml_runs[i];
        out.push_str(&doc_xml[copied..r.span.start]);
        let run_xml = &doc_xml[r.span.start..r.span.end];
        out.push_str(&rewrite_run(
            run_xml,
            r.end - r.start,
            run_edits,
            opts,
            ids,
        )?);
        copied = r.span.end;
    }
    let rest = &doc_xml[copied..p.span.end];
    if tail.is_empty() {
        out.push_str(rest);
        return Ok(out);
    }

    // Text appended at the paragraph end gets a fresh run before `</w:p>`.
    let (prefix, _) = element_prefix(para_xml);
    let w = |name: &str| qualified(prefix, name);
    let mut run = format!("<{}>", w("r"));
    for text in &tail {
        run.push_str(&text_xml(prefix, text, false));
    }
    run.push_str(&format!("</{}>", w("r")));
    let run = wrap(prefix, run, false, opts, ids);
    if let Some(open) = rest.strip_suffix("/>").filter(|_| copied == p.span.start) {
        out.push_str(open);
        out.push('>');
        out.push_str(&run);
        out.push_str(&format!("</{}>", w("p")));
    } else {
        let close = rest
            .rfind("</")
            .ok_or_else(|| Error::Edit("段落结构异常".into()))?;
        out.push_str(&rest[..close]);
        out.push_str(&run);
        out.push_str(&rest[close..]);
    }
    Ok(out)
}

/// Prefix and qualified name of the first element in `xml`, e.g. ("w", "w:p").
fn element_prefix(xml: &str) -> (&str, &str) {
    let name = xml
        .trim_start_matches('<')
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or("");
    (name.split_once(':').map_or("", |(p, _)| p), name)
}

fn qualified(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}:{name}")
    }
}

pub(crate) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

/// Run content for `text`: `<w:t>` (or `<w:delText>`) pieces, with tabs and
/// line breaks as their own elements.
fn text_xml(prefix: &str, text: &str, deleted: bool) -> String {
    let w = |name: &str| qualified(prefix, name);
    let t = w(if deleted { "delText" } else { "t" });
    let mut out = String::new();
    let mut piece = String::new();
    let flush = |piece: &mut String, out: &mut String| {
        if !piece.is_empty() {
            out.push_str(&format!(
                "<{t} xml:space=\"preserve\">{}</{t}>",
                escape_text(piece)
            ));
            piece.clear();
        }
    };
    for c in text.chars() {
        match c {
            '\t' => {
                flush(&mut piece, &mut out);
                out.push_str(&format!("<{}/>", w("tab")));
            }
            '\n' => {
                flush(&mut piece, &mut out);
                out.push_str(&format!("<{}/>", w("br")));
            }
            c => piece.push(c),
        }
    }
    flush(&mut piece, &mut out);
    out
}

/// Wraps a run in `<w:ins>` or `<w:del>` in tracked mode.
fn wrap(prefix: &str, run: String, deleted: bool, opts: &EditOptions, ids: &mut Ids) -> String {
    if opts.mode == EditMode::Direct {
        return run;
    }
    let a = if prefix.is_empty() { "w" } else { prefix };
    let tag = qualified(prefix, if deleted { "del" } else { "ins" });
    format!(
        "<{tag} {a}:id=\"{}\" {a}:author=\"{}\" {a}:date=\"{}\">{run}</{tag}>",
        ids.next(),
        escape_attr(&opts.author),
        escape_attr(&opts.date)
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keep,
    Delete,
    Insert,
}

enum Piece<'a> {
    Text(String),
    Raw(&'a str),
}

struct Item<'a> {
    raw: &'a str,
    /// Set for `<w:t>`, whose text can be split.
    text: Option<String>,
    chars: usize,
}

/// Rewrites one `<w:r>` whose content is `len` characters long.
fn rewrite_run(
    run_xml: &str,
    len: usize,
    mut edits: RunEdits,
    opts: &EditOptions,
    ids: &mut Ids,
) -> Result<String> {
    let bad = |what: &str| Error::Edit(format!("无法改写这段文字（{what}）"));
    let mut reader = Reader::from_str(run_xml);
    let pos = |r: &Reader<&[u8]>| r.buffer_position() as usize;
    fn read<'a>(r: &mut Reader<&'a [u8]>) -> Result<Event<'a>> {
        r.read_event().map_err(|e| Error::xml(DOCUMENT_PART, e))
    }

    let open_tag = match read(&mut reader)? {
        Event::Start(_) => &run_xml[..pos(&reader)],
        _ => return Err(bad("run 结构")),
    };
    let (prefix, qname) = element_prefix(open_tag);
    let mut rpr = "";
    let mut items = Vec::new();
    loop {
        let start = pos(&reader);
        match read(&mut reader)? {
            Event::Start(e) => {
                let name = local(&e);
                if name == "t" {
                    let text = read_text(&mut reader, &e, DOCUMENT_PART)?;
                    items.push(Item {
                        raw: &run_xml[start..pos(&reader)],
                        chars: text.chars().count(),
                        text: Some(text),
                    });
                    continue;
                }
                reader
                    .read_to_end(e.name())
                    .map_err(|err| Error::xml(DOCUMENT_PART, err))?;
                let raw = &run_xml[start..pos(&reader)];
                if name == "rPr" {
                    rpr = raw;
                } else {
                    items.push(Item {
                        raw,
                        text: None,
                        chars: fragment_chars(raw)?,
                    });
                }
            }
            Event::Empty(e) => {
                let raw = &run_xml[start..pos(&reader)];
                if local(&e) == "rPr" {
                    rpr = raw;
                } else {
                    items.push(Item {
                        raw,
                        text: None,
                        chars: fragment_chars(raw)?,
                    });
                }
            }
            Event::End(_) => break,
            Event::Eof => return Err(bad("run 未结束")),
            _ => {}
        }
    }
    if items.iter().map(|i| i.chars).sum::<usize>() != len {
        return Err(bad("字符数不一致"));
    }

    edits.inserts.sort_by_key(|(at, _)| *at);
    let deleted = |at: usize| edits.deletes.iter().any(|r| r.contains(&at));
    fn push<'a>(kind: Kind, piece: Piece<'a>, groups: &mut Vec<(Kind, Vec<Piece<'a>>)>) {
        match groups.last_mut() {
            Some((k, pieces)) if *k == kind => pieces.push(piece),
            _ => groups.push((kind, vec![piece])),
        }
    }
    let mut groups: Vec<(Kind, Vec<Piece>)> = Vec::new();
    let mut inserts = edits.inserts.iter().peekable();
    let mut at = 0;
    for item in &items {
        let end = at + item.chars;
        match &item.text {
            Some(text) => {
                let chars: Vec<char> = text.chars().collect();
                let mut cuts: Vec<usize> = vec![at, end];
                for r in &edits.deletes {
                    cuts.extend([r.start, r.end].into_iter().filter(|&c| c > at && c < end));
                }
                cuts.extend(
                    edits
                        .inserts
                        .iter()
                        .map(|(c, _)| *c)
                        .filter(|&c| c > at && c < end),
                );
                cuts.sort_unstable();
                cuts.dedup();
                for w in cuts.windows(2) {
                    while let Some((_, t)) = inserts.next_if(|(c, _)| *c <= w[0]) {
                        push(Kind::Insert, Piece::Text(t.clone()), &mut groups);
                    }
                    let kind = if deleted(w[0]) {
                        Kind::Delete
                    } else {
                        Kind::Keep
                    };
                    let piece: String = chars[w[0] - at..w[1] - at].iter().collect();
                    push(kind, Piece::Text(piece), &mut groups);
                }
            }
            None if item.chars == 0 => push(Kind::Keep, Piece::Raw(item.raw), &mut groups),
            None => {
                while let Some((_, t)) = inserts.next_if(|(c, _)| *c <= at) {
                    push(Kind::Insert, Piece::Text(t.clone()), &mut groups);
                }
                let covered = (at..end).filter(|&c| deleted(c)).count();
                if (covered != 0 && covered != item.chars)
                    || edits.inserts.iter().any(|(c, _)| *c > at && *c < end)
                {
                    return Err(bad("修改落在图片或符号内部"));
                }
                let kind = if covered == 0 {
                    Kind::Keep
                } else {
                    Kind::Delete
                };
                push(kind, Piece::Raw(item.raw), &mut groups);
            }
        }
        at = end;
    }
    for (_, t) in inserts {
        push(Kind::Insert, Piece::Text(t.clone()), &mut groups);
    }

    if opts.mode == EditMode::Direct {
        let mut merged: Vec<(Kind, Vec<Piece>)> = Vec::new();
        for (kind, pieces) in groups {
            if kind == Kind::Delete {
                continue;
            }
            match merged.last_mut() {
                Some((_, last)) => last.extend(pieces),
                None => merged.push((Kind::Keep, pieces)),
            }
        }
        groups = merged;
    }

    let mut out = String::new();
    for (kind, pieces) in groups {
        let mut run = format!("{open_tag}{rpr}");
        for piece in pieces {
            match piece {
                Piece::Text(t) => run.push_str(&text_xml(prefix, &t, kind == Kind::Delete)),
                Piece::Raw(r) => run.push_str(r),
            }
        }
        run.push_str(&format!("</{qname}>"));
        out.push_str(&match kind {
            Kind::Keep => run,
            Kind::Delete => wrap(prefix, run, true, opts, ids),
            Kind::Insert => wrap(prefix, run, false, opts, ids),
        });
    }
    Ok(out)
}
