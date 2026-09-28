//! Parser for the body of `word/document.xml`.
//!
//! Streams the XML once and builds paragraphs and tables, remembering the
//! byte span of every paragraph and top-level table so that edits can later
//! be spliced into the original XML without re-serialising anything else.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::{Error, Result};
use crate::model::{
    Block, ByteSpan, Inline, Marker, MarkerKind, NumberingRef, Paragraph, Revision, Run, RunFormat,
    Table, TableCell, XmlRun,
};
use crate::xml::{attr, local, read_text, skip, toggle};

const PART: &str = "word/document.xml";

/// Elements whose whole subtree carries nothing we display.
const SKIPPED: &[&str] = &[
    "sectPr",
    "sdtPr",
    "sdtEndPr",
    "tblPr",
    "tblGrid",
    "tblPrEx",
    "trPr",
    "rPrChange",
    "pPrChange",
    "instrText",
    "delInstrText",
    "Fallback",
    "txbxContent",
    "customXmlPr",
    "smartTagPr",
];

pub struct Body {
    pub blocks: Vec<Block>,
    pub paragraphs: Vec<Paragraph>,
}

pub fn parse(xml: &str) -> Result<Body> {
    let mut parser = Parser {
        reader: Reader::from_str(xml),
        paragraphs: Vec::new(),
        pending_starts: Vec::new(),
    };
    let mut blocks = Vec::new();
    loop {
        match parser.next()? {
            Event::Start(e) if local(&e) == "body" => {
                parser.container("body", &mut ContainerSink::Blocks(&mut blocks))?;
                break;
            }
            Event::Eof => return Err(Error::xml(PART, "missing <w:body>")),
            _ => {}
        }
    }
    parser.flush_pending_starts();
    Ok(Body {
        blocks,
        paragraphs: parser.paragraphs,
    })
}

/// Number of paragraph characters a fragment of run content (for example
/// `<w:tab/>` or a whole `<mc:AlternateContent>`) produces, counted exactly
/// as when the paragraph is read.
pub fn fragment_chars(xml: &str) -> Result<usize> {
    let mut parser = Parser {
        reader: Reader::from_str(xml),
        paragraphs: Vec::new(),
        pending_starts: Vec::new(),
    };
    let mut p = ParagraphBuilder::new(0);
    parser.paragraph_content(&mut p, true)?;
    Ok(p.len)
}

enum ContainerSink<'a> {
    Blocks(&'a mut Vec<Block>),
    Paragraphs(&'a mut Vec<usize>),
}

impl ContainerSink<'_> {
    fn paragraph(&mut self, index: usize) {
        match self {
            ContainerSink::Blocks(b) => b.push(Block::Paragraph(index)),
            ContainerSink::Paragraphs(p) => p.push(index),
        }
    }

    fn table(&mut self, table: Table) {
        match self {
            ContainerSink::Blocks(b) => b.push(Block::Table(table)),
            // Nested tables are flattened into the enclosing cell.
            ContainerSink::Paragraphs(p) => p.extend(Block::Table(table).paragraph_indices()),
        }
    }
}

struct Parser<'a> {
    reader: Reader<&'a [u8]>,
    paragraphs: Vec<Paragraph>,
    /// Comment ranges that start between paragraphs; they attach to the next one.
    pending_starts: Vec<String>,
}

impl<'a> Parser<'a> {
    fn pos(&self) -> usize {
        self.reader.buffer_position() as usize
    }

    fn next(&mut self) -> Result<Event<'a>> {
        self.reader.read_event().map_err(|e| Error::xml(PART, e))
    }

    fn skip(&mut self, e: &BytesStart<'_>) -> Result<()> {
        skip(&mut self.reader, e, PART)
    }

    /// Handles comment range marks that sit between block-level elements.
    fn block_level_marker(&mut self, e: &BytesStart<'_>) {
        let Some(id) = attr(e, "id") else { return };
        match local(e).as_str() {
            "commentRangeStart" => self.pending_starts.push(id),
            "commentRangeEnd" => {
                if let Some(p) = self.paragraphs.last_mut() {
                    let offset = p.char_len();
                    p.markers.push(Marker {
                        offset,
                        kind: MarkerKind::CommentEnd,
                        id,
                    });
                }
            }
            _ => {}
        }
    }

    fn flush_pending_starts(&mut self) {
        if self.pending_starts.is_empty() {
            return;
        }
        if let Some(p) = self.paragraphs.last_mut() {
            let offset = p.char_len();
            for id in self.pending_starts.drain(..) {
                p.markers.push(Marker {
                    offset,
                    kind: MarkerKind::CommentStart,
                    id,
                });
            }
        }
    }

    /// Parses block content (paragraphs, tables, transparent wrappers such as
    /// content controls) until the end tag named `end`.
    fn container(&mut self, end: &str, sink: &mut ContainerSink<'_>) -> Result<()> {
        loop {
            let start = self.pos();
            match self.next()? {
                Event::Start(e) => match local(&e).as_str() {
                    "p" => {
                        let index = self.paragraph(start, false)?;
                        sink.paragraph(index);
                    }
                    "tbl" => {
                        let table = self.table(start)?;
                        sink.table(table);
                    }
                    name if SKIPPED.contains(&name) => self.skip(&e)?,
                    name => {
                        let name = name.to_string();
                        self.container(&name, sink)?;
                    }
                },
                Event::Empty(e) => match local(&e).as_str() {
                    "p" => {
                        let index = self.paragraph(start, true)?;
                        sink.paragraph(index);
                    }
                    _ => self.block_level_marker(&e),
                },
                Event::End(e) if e.local_name().as_ref() == end => return Ok(()),
                Event::Eof => {
                    return Err(Error::xml(PART, format!("unexpected end inside <{end}>")));
                }
                _ => {}
            }
        }
    }

    fn table(&mut self, start: usize) -> Result<Table> {
        let mut rows = Vec::new();
        self.table_rows("tbl", &mut rows)?;
        Ok(Table {
            span: ByteSpan {
                start,
                end: self.pos(),
            },
            rows,
        })
    }

    fn table_rows(&mut self, end: &str, rows: &mut Vec<Vec<TableCell>>) -> Result<()> {
        loop {
            match self.next()? {
                Event::Start(e) => match local(&e).as_str() {
                    "tr" => {
                        let mut cells = Vec::new();
                        self.table_cells("tr", &mut cells)?;
                        rows.push(cells);
                    }
                    name if SKIPPED.contains(&name) => self.skip(&e)?,
                    name => {
                        let name = name.to_string();
                        self.table_rows(&name, rows)?;
                    }
                },
                Event::Empty(e) => self.block_level_marker(&e),
                Event::End(e) if e.local_name().as_ref() == end => return Ok(()),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside table")),
                _ => {}
            }
        }
    }

    fn table_cells(&mut self, end: &str, cells: &mut Vec<TableCell>) -> Result<()> {
        loop {
            match self.next()? {
                Event::Start(e) => match local(&e).as_str() {
                    "tc" => {
                        let cell = self.table_cell()?;
                        cells.push(cell);
                    }
                    name if SKIPPED.contains(&name) => self.skip(&e)?,
                    name => {
                        let name = name.to_string();
                        self.table_cells(&name, cells)?;
                    }
                },
                Event::Empty(e) => self.block_level_marker(&e),
                Event::End(e) if e.local_name().as_ref() == end => return Ok(()),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside table row")),
                _ => {}
            }
        }
    }

    fn table_cell(&mut self) -> Result<TableCell> {
        let mut cell = TableCell {
            grid_span: 1,
            v_merge: None,
            paragraphs: Vec::new(),
        };
        loop {
            let start = self.pos();
            match self.next()? {
                Event::Start(e) => match local(&e).as_str() {
                    "tcPr" => self.cell_properties(&mut cell)?,
                    "p" => {
                        let index = self.paragraph(start, false)?;
                        cell.paragraphs.push(index);
                    }
                    "tbl" => {
                        let table = self.table(start)?;
                        ContainerSink::Paragraphs(&mut cell.paragraphs).table(table);
                    }
                    name if SKIPPED.contains(&name) => self.skip(&e)?,
                    name => {
                        let name = name.to_string();
                        self.container(
                            &name,
                            &mut ContainerSink::Paragraphs(&mut cell.paragraphs),
                        )?;
                    }
                },
                Event::Empty(e) => match local(&e).as_str() {
                    "p" => {
                        let index = self.paragraph(start, true)?;
                        cell.paragraphs.push(index);
                    }
                    _ => self.block_level_marker(&e),
                },
                Event::End(e) if e.local_name().as_ref() == "tc" => return Ok(cell),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside table cell")),
                _ => {}
            }
        }
    }

    fn cell_properties(&mut self, cell: &mut TableCell) -> Result<()> {
        loop {
            match self.next()? {
                Event::Start(e) | Event::Empty(e) => match local(&e).as_str() {
                    "gridSpan" => {
                        cell.grid_span = attr(&e, "val").and_then(|v| v.parse().ok()).unwrap_or(1)
                    }
                    "vMerge" => cell.v_merge = Some(attr(&e, "val").as_deref() == Some("restart")),
                    _ => {}
                },
                Event::End(e) if e.local_name().as_ref() == "tcPr" => return Ok(()),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside tcPr")),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, start: usize, empty: bool) -> Result<usize> {
        let index = self.paragraphs.len();
        let mut p = ParagraphBuilder::new(index);
        for id in self.pending_starts.drain(..) {
            p.marker(MarkerKind::CommentStart, id);
        }
        if !empty {
            self.paragraph_content(&mut p, false)?;
        }
        let span = ByteSpan {
            start,
            end: self.pos(),
        };
        self.paragraphs.push(p.finish(span));
        Ok(index)
    }

    /// Reads paragraph content up to `</w:p>`, or to the end of input when
    /// `fragment` is set.
    fn paragraph_content(&mut self, p: &mut ParagraphBuilder, fragment: bool) -> Result<()> {
        loop {
            let start = self.pos();
            match self.next()? {
                Event::Start(e) => match local(&e).as_str() {
                    "pPr" => self.paragraph_properties(p)?,
                    "r" => {
                        p.format = RunFormat::default();
                        p.open_run = Some((start, p.len));
                    }
                    "rPr" => p.format = self.run_properties()?,
                    "t" | "delText" => {
                        let text = read_text(&mut self.reader, &e, PART)?;
                        p.push(&text, Inline::Text);
                    }
                    "ins" | "moveTo" => p.revisions.push((Revision::Insert, attr(&e, "author"))),
                    "del" | "moveFrom" => p.revisions.push((Revision::Delete, attr(&e, "author"))),
                    "drawing" | "pict" | "object" => {
                        let rel_id = image_rel_id(&mut self.reader, &e)?;
                        p.push("\u{FFFC}", Inline::Image { rel_id });
                    }
                    "oMath" | "oMathPara" => {
                        self.skip(&e)?;
                        p.push("〔公式〕", Inline::Math);
                    }
                    name if SKIPPED.contains(&name) => self.skip(&e)?,
                    // hyperlink, smartTag, sdt, sdtContent, fldSimple, AlternateContent, Choice, ...
                    _ => {}
                },
                Event::Empty(e) => match local(&e).as_str() {
                    "tab" | "ptab" => p.push("\t", Inline::Text),
                    "br" | "cr" => {
                        if attr(&e, "type").as_deref() != Some("page") {
                            p.push("\n", Inline::Text);
                        }
                    }
                    "noBreakHyphen" => p.push("\u{2011}", Inline::Text),
                    "sym" => {
                        let c = attr(&e, "char")
                            .and_then(|v| u32::from_str_radix(&v, 16).ok())
                            .and_then(char::from_u32)
                            .filter(|c| !('\u{E000}'..='\u{F8FF}').contains(c))
                            .unwrap_or('□');
                        p.push(&c.to_string(), Inline::Text);
                    }
                    "footnoteReference" | "endnoteReference" => {
                        let id = attr(&e, "id").unwrap_or_default();
                        p.push(&id.clone(), Inline::Note { id });
                    }
                    "commentRangeStart" => p.marker_attr(&e, MarkerKind::CommentStart),
                    "commentRangeEnd" => p.marker_attr(&e, MarkerKind::CommentEnd),
                    "commentReference" => p.marker_attr(&e, MarkerKind::CommentReference),
                    "rPr" => p.format = RunFormat::default(),
                    _ => {}
                },
                Event::End(e) => match e.local_name().as_ref() {
                    "p" if !fragment => return Ok(()),
                    "r" => p.close_run(self.pos()),
                    "ins" | "moveTo" | "del" | "moveFrom" => {
                        p.revisions.pop();
                    }
                    _ => {}
                },
                Event::Eof if fragment => return Ok(()),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside paragraph")),
                _ => {}
            }
        }
    }

    fn paragraph_properties(&mut self, p: &mut ParagraphBuilder) -> Result<()> {
        loop {
            let (e, is_start) = match self.next()? {
                Event::Start(e) => (e, true),
                Event::Empty(e) => (e, false),
                Event::End(e) if e.local_name().as_ref() == "pPr" => return Ok(()),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside pPr")),
                _ => continue,
            };
            match local(&e).as_str() {
                "pStyle" => p.style_id = attr(&e, "val"),
                "outlineLvl" => p.outline_level = attr(&e, "val").and_then(|v| v.parse().ok()),
                "numId" => p.num_id = attr(&e, "val").and_then(|v| v.parse().ok()),
                "ilvl" => p.num_level = attr(&e, "val").and_then(|v| v.parse().ok()),
                "jc" => p.align = attr(&e, "val"),
                // Paragraph-mark run properties, property revisions and section breaks.
                "rPr" | "pPrChange" | "sectPr" if is_start => self.skip(&e)?,
                _ => {}
            }
        }
    }

    fn run_properties(&mut self) -> Result<RunFormat> {
        let mut f = RunFormat::default();
        loop {
            match self.next()? {
                Event::Start(e) if local(&e) == "rPrChange" => self.skip(&e)?,
                Event::Start(e) | Event::Empty(e) => match local(&e).as_str() {
                    "b" => f.bold = toggle(&e),
                    "i" => f.italic = toggle(&e),
                    "u" => f.underline = toggle(&e),
                    "strike" | "dstrike" => f.strike = f.strike || toggle(&e),
                    "vertAlign" => {
                        let v = attr(&e, "val");
                        f.superscript = v.as_deref() == Some("superscript");
                        f.subscript = v.as_deref() == Some("subscript");
                    }
                    _ => {}
                },
                Event::End(e) if e.local_name().as_ref() == "rPr" => return Ok(f),
                Event::Eof => return Err(Error::xml(PART, "unexpected end inside rPr")),
                _ => {}
            }
        }
    }
}

/// Reads a drawing subtree and returns the relationship id of its picture.
fn image_rel_id(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<Option<String>> {
    let end = start.name().as_ref().to_string();
    let mut depth = 0usize;
    let mut rel_id = None;
    let is_picture = |e: &BytesStart<'_>| matches!(local(e).as_str(), "blip" | "imagedata");
    loop {
        match reader.read_event().map_err(|e| Error::xml(PART, e))? {
            Event::Start(e) => {
                if e.name().as_ref() == end {
                    depth += 1;
                } else if rel_id.is_none() && is_picture(&e) {
                    rel_id = attr(&e, "embed").or_else(|| attr(&e, "id"));
                }
            }
            Event::Empty(e) if rel_id.is_none() && is_picture(&e) => {
                rel_id = attr(&e, "embed").or_else(|| attr(&e, "id"));
            }
            Event::End(e) if e.name().as_ref() == end => {
                if depth == 0 {
                    return Ok(rel_id);
                }
                depth -= 1;
            }
            Event::Eof => return Err(Error::xml(PART, "unexpected end inside drawing")),
            _ => {}
        }
    }
}

struct ParagraphBuilder {
    index: usize,
    style_id: Option<String>,
    outline_level: Option<u8>,
    num_id: Option<u32>,
    num_level: Option<u8>,
    align: Option<String>,
    text: String,
    len: usize,
    runs: Vec<Run>,
    markers: Vec<Marker>,
    format: RunFormat,
    revisions: Vec<(Revision, Option<String>)>,
    xml_runs: Vec<XmlRun>,
    /// Byte and char offset where the current `<w:r>` started.
    open_run: Option<(usize, usize)>,
}

impl ParagraphBuilder {
    fn new(index: usize) -> Self {
        Self {
            index,
            style_id: None,
            outline_level: None,
            num_id: None,
            num_level: None,
            align: None,
            text: String::new(),
            len: 0,
            runs: Vec::new(),
            markers: Vec::new(),
            format: RunFormat::default(),
            revisions: Vec::new(),
            xml_runs: Vec::new(),
            open_run: None,
        }
    }

    fn close_run(&mut self, end_byte: usize) {
        if let Some((start_byte, start)) = self.open_run.take() {
            self.xml_runs.push(XmlRun {
                span: ByteSpan {
                    start: start_byte,
                    end: end_byte,
                },
                start,
                end: self.len,
                revision: self.revisions.last().map_or(Revision::None, |r| r.0),
            });
        }
    }

    fn push(&mut self, text: &str, inline: Inline) {
        let n = text.chars().count();
        if n == 0 {
            return;
        }
        let (revision, author) = self
            .revisions
            .last()
            .cloned()
            .unwrap_or((Revision::None, None));
        self.text.push_str(text);
        let start = self.len;
        self.len += n;
        if let Some(last) = self.runs.last_mut()
            && inline == Inline::Text
            && last.inline == Inline::Text
            && last.format == self.format
            && last.revision == revision
            && last.revision_author == author
        {
            last.end = self.len;
            return;
        }
        self.runs.push(Run {
            start,
            end: self.len,
            format: self.format,
            revision,
            revision_author: author,
            inline,
        });
    }

    fn marker(&mut self, kind: MarkerKind, id: String) {
        self.markers.push(Marker {
            offset: self.len,
            kind,
            id,
        });
    }

    fn marker_attr(&mut self, e: &BytesStart<'_>, kind: MarkerKind) {
        if let Some(id) = attr(e, "id") {
            self.marker(kind, id);
        }
    }

    fn finish(self, span: ByteSpan) -> Paragraph {
        Paragraph {
            index: self.index,
            span,
            style_id: self.style_id,
            outline_level: self.outline_level,
            numbering: self.num_id.map(|num_id| NumberingRef {
                num_id,
                level: self.num_level.unwrap_or(0),
            }),
            align: self.align,
            text: self.text,
            runs: self.runs,
            markers: self.markers,
            xml_runs: self.xml_runs,
        }
    }
}
