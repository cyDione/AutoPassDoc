//! In-memory document model.
//!
//! Offsets inside a paragraph are counted in Unicode scalar values (`char`s)
//! of the paragraph's display text, which includes deleted revision text.

use serde::Serialize;

/// Byte range of an element inside `word/document.xml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RunFormat {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub superscript: bool,
    pub subscript: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Revision {
    #[default]
    None,
    Insert,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Inline {
    Text,
    /// A drawing; `rel_id` points into `word/_rels/document.xml.rels`.
    Image {
        rel_id: Option<String>,
    },
    /// Footnote or endnote reference mark.
    Note {
        id: String,
    },
    /// An equation, shown as a placeholder.
    Math,
}

/// A contiguous piece of paragraph text with one format and revision state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub format: RunFormat,
    pub revision: Revision,
    pub revision_author: Option<String>,
    pub inline: Inline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    CommentStart,
    CommentEnd,
    CommentReference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub offset: usize,
    pub kind: MarkerKind,
    pub id: String,
}

/// A `<w:r>` element: its bytes in `word/document.xml` and the paragraph
/// characters it produces. Edits rewrite runs through these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlRun {
    pub span: ByteSpan,
    pub start: usize,
    pub end: usize,
    pub revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberingRef {
    pub num_id: u32,
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    /// Index among all paragraphs of the document, in document order,
    /// including paragraphs inside tables.
    pub index: usize,
    pub span: ByteSpan,
    pub style_id: Option<String>,
    /// Outline level set directly on the paragraph (0 = heading 1).
    pub outline_level: Option<u8>,
    pub numbering: Option<NumberingRef>,
    pub align: Option<String>,
    pub text: String,
    pub runs: Vec<Run>,
    pub markers: Vec<Marker>,
    pub xml_runs: Vec<XmlRun>,
}

impl Paragraph {
    pub fn char_len(&self) -> usize {
        self.runs.last().map_or(0, |r| r.end)
    }

    /// Text with deletions removed, i.e. the text as if all revisions were accepted.
    pub fn accepted_text(&self) -> String {
        let chars: Vec<char> = self.text.chars().collect();
        self.runs
            .iter()
            .filter(|r| r.revision != Revision::Delete && r.inline == Inline::Text)
            .flat_map(|r| chars[r.start..r.end].iter())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    pub grid_span: u32,
    /// `Some(true)` starts a vertical merge, `Some(false)` continues one.
    pub v_merge: Option<bool>,
    /// Indices into [`Document::paragraphs`].
    pub paragraphs: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub span: ByteSpan,
    pub rows: Vec<Vec<TableCell>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(usize),
    Table(Table),
}

impl Block {
    /// All paragraph indices contained in this block.
    pub fn paragraph_indices(&self) -> Vec<usize> {
        match self {
            Block::Paragraph(i) => vec![*i],
            Block::Table(t) => t
                .rows
                .iter()
                .flatten()
                .flat_map(|c| c.paragraphs.iter().copied())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommentAnchor {
    pub start_paragraph: usize,
    pub start_offset: usize,
    pub end_paragraph: usize,
    pub end_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub initials: Option<String>,
    pub date: Option<String>,
    pub text: String,
    /// `w14:paraId` of the comment's last paragraph; links to commentsExtended.xml.
    #[serde(skip)]
    pub para_id: Option<String>,
    pub parent_id: Option<String>,
    pub done: bool,
    pub anchor: Option<CommentAnchor>,
}
