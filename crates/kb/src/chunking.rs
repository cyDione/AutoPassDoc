//! Structure detection and parent/child chunking for Chinese official documents.
//!
//! A document is a list of lines (paragraphs). Headings are recognised from
//! the file's own markup (Word heading styles, Markdown `#`) and from the
//! numbering conventions of 公文 and regulations:
//!
//! | rank | pattern |
//! |---|---|
//! | 0 | `附件1` |
//! | 1 | `第X编` / `第X部分` / `第X篇` |
//! | 2 | `第X章` |
//! | 3 | `第X节` |
//! | 4 | `第X条` (the article, the key legal unit) |
//! | 5 | `一、` |
//! | 6 | `（一）` |
//! | 7 | `1.` / `1．` / `1、` |
//! | 8 | `（1）` / `⑴` |
//! | 9 | `①` |
//!
//! Numbered items inside an article are its 款/项 and stay in the article.
//!
//! Every line belongs to exactly one *segment*: a heading line and the body
//! below it up to the next heading. A heading directly followed by a deeper
//! heading joins that heading's segment, so no text is ever dropped. Segments
//! are the *parents* (split further when longer than [`PARENT_MAX_CHARS`]),
//! and *children* of at most [`CHILD_MAX_CHARS`] are cut from each parent at
//! paragraph and then sentence boundaries. A document that fits in one parent
//! stays one parent, with children still cut per segment.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::text::{normalize_char, trim_span};

/// Maximum length of a child chunk, in chars.
pub const CHILD_MAX_CHARS: usize = 400;
/// Parents longer than this are split at paragraph boundaries. A document no
/// longer than this is a single parent (its children still follow headings).
pub const PARENT_MAX_CHARS: usize = 1500;
/// Longest heading text kept in a heading path.
const HEADING_TEXT_MAX: usize = 60;

/// One line (paragraph) of a document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLine {
    /// The line as it appears in the document's full text.
    pub text: String,
    /// Heading level from the file's markup (0 = top), e.g. a Word heading
    /// style or Markdown `#`.
    pub style_level: Option<u8>,
    /// What to run heading detection on when it differs from `text`: the
    /// Markdown heading without `#`, or a Word paragraph with its automatic
    /// numbering label in front.
    pub display: Option<String>,
}

impl SourceLine {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// The kind of numbering that makes a line a heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HeadingKind {
    /// `附件1`
    Attachment,
    /// `第X编`, `第X部分`, `第X篇`
    Part,
    /// `第X章`
    Chapter,
    /// `第X节`
    Section,
    /// `第X条`
    Article,
    /// `一、`
    Outline1,
    /// `（一）`
    Outline2,
    /// `1.` `1．` `1、`
    Outline3,
    /// `（1）` `⑴`
    Outline4,
    /// `①`
    Outline5,
    /// A heading only by its style (Word heading style, Markdown `#`).
    Styled,
}

impl HeadingKind {
    /// Position in the hierarchy, smaller is higher; `None` for [`HeadingKind::Styled`].
    pub fn rank(self) -> Option<u8> {
        Some(match self {
            Self::Attachment => 0,
            Self::Part => 1,
            Self::Chapter => 2,
            Self::Section => 3,
            Self::Article => 4,
            Self::Outline1 => 5,
            Self::Outline2 => 6,
            Self::Outline3 => 7,
            Self::Outline4 => 8,
            Self::Outline5 => 9,
            Self::Styled => return None,
        })
    }
}

/// A recognised heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heading {
    pub kind: HeadingKind,
    /// Text for the heading path: the whole line, or just the label of an
    /// article (`第二十条`) or the lead phrase of a run-in heading.
    pub text: String,
    /// The line continues with body text after the heading.
    pub has_body: bool,
}

/// A document split into parents and children.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunked {
    /// The lines joined with `\n`; all offsets point into this text.
    pub full_text: String,
    pub parents: Vec<ParentChunk>,
}

/// The smallest structural unit: an article or a leaf section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentChunk {
    pub heading_path: Vec<String>,
    pub text: String,
    /// Char offsets in [`Chunked::full_text`].
    pub char_start: usize,
    pub char_end: usize,
    pub children: Vec<ChildChunk>,
}

/// A retrieval unit of at most [`CHILD_MAX_CHARS`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildChunk {
    pub heading_path: Vec<String>,
    pub text: String,
    /// Char offsets in [`Chunked::full_text`].
    pub char_start: usize,
    pub char_end: usize,
}

const NUM: &str = "[0-9一二三四五六七八九十百千零〇○两]+";
const CN_NUM: &str = "[一二三四五六七八九十百零〇]+";

static ORDINAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("^第({NUM})(部分|编|篇|章|节|条)")).unwrap());
static OUTLINE1: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("^{CN_NUM}、")).unwrap());
static OUTLINE2: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^\({CN_NUM}\)")).unwrap());
static OUTLINE3: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]{1,3}[.、]").unwrap());
static OUTLINE4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\([0-9]{1,3}\)|[⑴-⒇])").unwrap());
static OUTLINE5: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[①-⑳]").unwrap());
static ATTACHMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^附件\s*[0-9一二三四五六七八九十]*\s*:?$").unwrap());

/// Characters that, right after `第X条`, mean the line cites an article
/// ("第二十条规定的…") instead of starting one.
const CITATION_FOLLOWERS: &[char] = &[
    '规', '所', '的', '中', '之', '至', '和', '及', '或', '到', '款', '项', '与', '第', '、', ',',
    '。', ';',
];

/// Recognises a heading from its numbering. Long lines are body text: a
/// `1.` line longer than 60 chars is a list item, and an article line keeps
/// only its label (`第二十条`) as the heading while the rest stays body.
pub fn detect_heading(line: &str) -> Option<Heading> {
    let original: Vec<char> = line.trim().chars().collect();
    if original.is_empty() {
        return None;
    }
    let chars: Vec<char> = original.iter().map(|&c| normalize_char(c)).collect();
    let norm: String = chars.iter().collect();
    let len = chars.len();
    let heading = |kind, end: usize, has_body| {
        Some(Heading {
            kind,
            text: heading_text(&original[..end]),
            has_body,
        })
    };

    if let Some(c) = ORDINAL.captures(&norm) {
        let label = c[0].chars().count();
        if chars
            .get(label)
            .is_some_and(|c| CITATION_FOLLOWERS.contains(c))
        {
            return None;
        }
        if &c[2] == "条" {
            let mut end = label;
            let next = (label..len).find(|&i| !chars[i].is_whitespace());
            if let Some(open) = next.filter(|&i| matches!(chars[i], '【' | '[' | '(')) {
                let close =
                    (open + 1..len.min(open + 22)).find(|&i| matches!(chars[i], '】' | ']' | ')'));
                if let Some(close) = close {
                    end = close + 1;
                }
            }
            let has_body = chars[end..].iter().any(|c| !c.is_whitespace());
            return heading(HeadingKind::Article, end, has_body);
        }
        if len > 40 || chars.contains(&'。') {
            return None;
        }
        let kind = match &c[2] {
            "章" => HeadingKind::Chapter,
            "节" => HeadingKind::Section,
            _ => HeadingKind::Part,
        };
        return heading(kind, len, false);
    }

    if ATTACHMENT.is_match(&norm) {
        return heading(HeadingKind::Attachment, len, false);
    }

    let (kind, label) = if let Some(m) = OUTLINE1.find(&norm) {
        (HeadingKind::Outline1, m.as_str())
    } else if let Some(m) = OUTLINE2.find(&norm) {
        (HeadingKind::Outline2, m.as_str())
    } else if let Some(m) = OUTLINE3.find(&norm) {
        (HeadingKind::Outline3, m.as_str())
    } else if let Some(m) = OUTLINE4.find(&norm) {
        (HeadingKind::Outline4, m.as_str())
    } else if let Some(m) = OUTLINE5.find(&norm) {
        (HeadingKind::Outline5, m.as_str())
    } else {
        return None;
    };
    let label = label.chars().count();
    // "1.5亿元" is a number, not a heading.
    if kind == HeadingKind::Outline3 && chars.get(label).is_some_and(char::is_ascii_digit) {
        return None;
    }
    // Items of an enumeration end with a comma or semicolon.
    if matches!(chars[len - 1], ',' | ';' | '、') {
        return None;
    }
    // End of the run-in lead phrase: "（一）加强组织领导。各地区要……"
    let lead = (label..len - 1).find(|&i| matches!(chars[i], '。' | ':' | ';' | '!' | '?'));
    match kind {
        HeadingKind::Outline4 | HeadingKind::Outline5 => {
            if len <= 40 && lead.is_none() {
                heading(kind, len, false)
            } else {
                None
            }
        }
        _ => {
            if len <= 60 && lead.is_none() {
                heading(kind, len, false)
            } else if let Some(lead) =
                lead.filter(|&e| e <= 30 && (kind != HeadingKind::Outline3 || len <= 60))
            {
                heading(kind, lead, true)
            } else if len <= 60 {
                heading(kind, len, false)
            } else {
                None
            }
        }
    }
}

/// Cleans a heading for display: collapses whitespace, closes up letter-spaced
/// titles (`总　则` → `总则`), drops trailing punctuation and caps the length.
fn heading_text(chars: &[char]) -> String {
    let s: String = chars.iter().collect();
    let words: Vec<&str> = s.split_whitespace().collect();
    let text = if words.len() > 2 && words[1..].iter().all(|w| w.chars().count() == 1) {
        format!("{} {}", words[0], words[1..].concat())
    } else if words.len() > 1 && words.iter().all(|w| w.chars().count() == 1) {
        words.concat()
    } else {
        words.join(" ")
    };
    finish_heading(text)
}

fn finish_heading(text: String) -> String {
    let text = text.trim_end_matches(['。', '：', ':', '；', ';', '，', ',']);
    if text.chars().count() > HEADING_TEXT_MAX {
        let mut t: String = text.chars().take(HEADING_TEXT_MAX).collect();
        t.push('…');
        t
    } else {
        text.to_string()
    }
}

/// An entry of the open-heading stack.
struct Frame {
    rank: Option<u8>,
    style: Option<u8>,
    text: String,
}

impl Frame {
    /// Whether `self` can contain `other`. Style levels decide between styled
    /// headings, numbering ranks between numbered ones, and a styled heading
    /// contains unstyled numbered headings.
    fn contains(&self, other: &Frame) -> bool {
        match (self.style, other.style, self.rank, other.rank) {
            (Some(a), Some(b), _, _) => a < b,
            (_, _, Some(a), Some(b)) => a < b,
            _ => self.style.is_some(),
        }
    }
}

fn line_heading(line: &SourceLine) -> Option<(Heading, Option<u8>)> {
    let source = line.display.as_deref().unwrap_or(&line.text).trim();
    match line.style_level {
        Some(level) if !source.is_empty() && source.chars().count() <= 100 => {
            let heading = detect_heading(source).unwrap_or_else(|| Heading {
                kind: HeadingKind::Styled,
                text: heading_text(&source.chars().collect::<Vec<_>>()),
                has_body: false,
            });
            Some((heading, Some(level)))
        }
        Some(_) => None,
        None => detect_heading(source).map(|h| (h, None)),
    }
}

struct Segment {
    first_line: usize,
    end_line: usize,
    path: Vec<String>,
}

fn segments(lines: &[SourceLine]) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    // The open segment: first line, heading path, whether it has body text,
    // and the stack depth of the heading that opened it.
    let mut open: Option<(usize, Vec<String>, bool, Option<usize>)> = None;
    for (i, line) in lines.iter().enumerate() {
        let is_empty = line.text.trim().is_empty()
            && line.display.as_deref().is_none_or(|d| d.trim().is_empty());
        if is_empty {
            continue;
        }
        let in_article = stack.iter().any(|f| f.rank == Some(4));
        let heading = line_heading(line).filter(|(h, style)| {
            // Numbered items inside an article are its 款/项, not new units.
            !(in_article && style.is_none() && h.kind.rank().is_some_and(|r| r > 4))
        });
        let Some((heading, style)) = heading else {
            match &mut open {
                Some((_, _, has_body, _)) => *has_body = true,
                None => open = Some((i, path_of(&stack), true, None)),
            }
            continue;
        };
        let frame = Frame {
            rank: heading.kind.rank(),
            style,
            text: heading.text,
        };
        while stack.last().is_some_and(|top| !top.contains(&frame)) {
            stack.pop();
        }
        // A heading without body joins the segment of a heading nested in it.
        let merge = matches!(&open, Some((_, _, false, Some(depth))) if stack.len() >= *depth);
        stack.push(frame);
        let first_line = match open.take() {
            Some((first, _, _, _)) if merge => first,
            Some((first, path, _, _)) => {
                out.push(Segment {
                    first_line: first,
                    end_line: i,
                    path,
                });
                i
            }
            None => i,
        };
        open = Some((
            first_line,
            path_of(&stack),
            heading.has_body,
            Some(stack.len()),
        ));
    }
    if let Some((first_line, path, _, _)) = open {
        out.push(Segment {
            first_line,
            end_line: lines.len(),
            path,
        });
    }
    out
}

fn path_of(stack: &[Frame]) -> Vec<String> {
    stack.iter().map(|f| f.text.clone()).collect()
}

/// Chunks plain text (lines separated by `\n`), detecting headings from numbering only.
pub fn chunk_text(text: &str) -> Chunked {
    let lines: Vec<SourceLine> = text
        .split('\n')
        .map(|l| SourceLine::new(l.strip_suffix('\r').unwrap_or(l)))
        .collect();
    chunk_lines(&lines)
}

/// Splits a document into parents and children.
pub fn chunk_lines(lines: &[SourceLine]) -> Chunked {
    let mut full_text = String::new();
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut pos = 0;
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            full_text.push('\n');
            pos += 1;
        }
        line_starts.push(pos);
        full_text.push_str(&line.text);
        pos += line.text.chars().count();
    }
    let chars: Vec<char> = full_text.chars().collect();
    let slice = |s: usize, e: usize| chars[s..e].iter().collect::<String>();
    let line_end = |i: usize| line_starts[i] + lines[i].text.chars().count();

    let mut parents = Vec::new();
    let Some((doc_start, doc_end)) = trim_span(&chars, 0, chars.len()) else {
        return Chunked { full_text, parents };
    };
    let spans: Vec<((usize, usize), Vec<String>)> = segments(lines)
        .into_iter()
        .filter_map(|seg| {
            let span = trim_span(
                &chars,
                line_starts[seg.first_line],
                line_end(seg.end_line - 1),
            )?;
            Some((span, seg.path))
        })
        .collect();
    let children_of = |(s, e): (usize, usize), path: &[String]| -> Vec<ChildChunk> {
        split_span(&chars, s, e, CHILD_MAX_CHARS)
            .into_iter()
            .map(|(cs, ce)| ChildChunk {
                heading_path: path.to_vec(),
                text: slice(cs, ce),
                char_start: cs,
                char_end: ce,
            })
            .collect()
    };

    if doc_end - doc_start <= PARENT_MAX_CHARS {
        // A short document is one parent; its children still follow the structure.
        let children: Vec<ChildChunk> = spans
            .iter()
            .flat_map(|(span, path)| children_of(*span, path))
            .collect();
        let mut common = spans.first().map(|s| s.1.clone()).unwrap_or_default();
        for (_, path) in &spans {
            let n = common.iter().zip(path).take_while(|(a, b)| a == b).count();
            common.truncate(n);
        }
        parents.push(ParentChunk {
            heading_path: common,
            text: slice(doc_start, doc_end),
            char_start: doc_start,
            char_end: doc_end,
            children,
        });
        return Chunked { full_text, parents };
    }

    for ((s, e), path) in &spans {
        for (ps, pe) in split_span(&chars, *s, *e, PARENT_MAX_CHARS) {
            parents.push(ParentChunk {
                heading_path: path.clone(),
                text: slice(ps, pe),
                char_start: ps,
                char_end: pe,
                children: children_of((ps, pe), path),
            });
        }
    }
    Chunked { full_text, parents }
}

fn is_sentence_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '；' | '!' | '?' | ';')
}

fn is_closing(c: char) -> bool {
    matches!(
        c,
        '”' | '’' | '"' | '\'' | '」' | '』' | '）' | ')' | '】' | '〕' | '》'
    )
}

fn is_clause_end(c: char) -> bool {
    matches!(c, '，' | ',' | '、' | '：' | ':')
}

/// Splits `[start, end)` into trimmed spans of at most `max` chars: whole
/// paragraphs where they fit, otherwise sentences, then clauses, and only
/// as a last resort a hard cut.
fn split_span(chars: &[char], start: usize, end: usize, max: usize) -> Vec<(usize, usize)> {
    let Some((start, end)) = trim_span(chars, start, end) else {
        return Vec::new();
    };
    if end - start <= max {
        return vec![(start, end)];
    }
    let mut pieces = Vec::new();
    let mut line_start = start;
    for i in start..=end {
        if i == end || chars[i] == '\n' {
            if let Some(span) = trim_span(chars, line_start, i) {
                split_long(chars, span, max, &mut pieces);
            }
            line_start = i + 1;
        }
    }
    // Pack greedily, then again aiming at equal sizes so the last chunk is
    // not a small remainder, unless that needs more chunks.
    let greedy = pack(&pieces, max, usize::MAX);
    if greedy.len() < 2 {
        return greedy;
    }
    let target = (end - start).div_ceil(greedy.len());
    let balanced = pack(&pieces, max, target);
    if balanced.len() <= greedy.len() {
        balanced
    } else {
        greedy
    }
}

/// Joins consecutive pieces while the group is shorter than `target` and
/// stays within `max`.
fn pack(pieces: &[(usize, usize)], max: usize, target: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &(s, e) in pieces {
        match out.last_mut() {
            Some(last) if last.1 - last.0 < target && e - last.0 <= max => last.1 = e,
            _ => out.push((s, e)),
        }
    }
    out
}

/// Pushes `span` to `out`, cut at sentence ends (then clause ends, then
/// hard) when it is longer than `max`.
fn split_long(chars: &[char], span: (usize, usize), max: usize, out: &mut Vec<(usize, usize)>) {
    if span.1 - span.0 <= max {
        out.push(span);
        return;
    }
    for (s, e) in cut_after(chars, span, is_sentence_end) {
        if e - s <= max {
            out.push((s, e));
            continue;
        }
        for (cs, ce) in cut_after(chars, (s, e), is_clause_end) {
            let mut i = cs;
            while ce - i > max {
                out.push((i, i + max));
                i += max;
            }
            out.push((i, ce));
        }
    }
}

/// Cuts a span after every char matching `is_end` (plus closing quotes and
/// brackets that follow it), trimming each piece.
fn cut_after(
    chars: &[char],
    (start, end): (usize, usize),
    is_end: fn(char) -> bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut piece_start = start;
    let mut i = start;
    while i < end {
        if is_end(chars[i]) {
            let mut j = i + 1;
            while j < end && (is_closing(chars[j]) || is_end(chars[j])) {
                j += 1;
            }
            if let Some(span) = trim_span(chars, piece_start, j) {
                out.push(span);
            }
            piece_start = j;
            i = j;
        } else {
            i += 1;
        }
    }
    if let Some(span) = trim_span(chars, piece_start, end) {
        out.push(span);
    }
    out
}
