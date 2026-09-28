//! Native .docx engine for AutoPassDoc.
//!
//! Parses WordprocessingML into a lightweight model for display (paragraphs,
//! tables, comments, revisions, list labels) while keeping the original
//! package bytes, so saving copies everything that was not edited verbatim.

mod body;
mod comments;
mod error;
pub mod model;
mod numbering;
mod package;
mod styles;
pub mod testgen;
pub mod view;
mod xml;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

pub use error::{Error, Result};
pub use model::*;
pub use package::Package;

use numbering::Numbering;
use styles::Styles;

pub const DOCUMENT_PART: &str = "word/document.xml";

pub struct Document {
    package: Package,
    pub paragraphs: Vec<Paragraph>,
    pub blocks: Vec<Block>,
    pub comments: Vec<Comment>,
    styles: Styles,
    heading_levels: Vec<Option<u8>>,
    list_labels: Vec<Option<String>>,
    /// Top-level block index of each paragraph.
    paragraph_blocks: Vec<usize>,
    /// Comment ranges per paragraph: (comment index, start, end) in chars.
    comment_ranges: Vec<Vec<(usize, usize, usize)>>,
    relationships: HashMap<String, String>,
}

impl Document {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let package = Package::from_bytes(bytes)?;

        let document_bytes = package.required_part(DOCUMENT_PART)?;
        let body = body::parse(xml::to_str(&document_bytes, DOCUMENT_PART)?)?;

        let styles = match package.part("word/styles.xml")? {
            Some(b) => Styles::parse(xml::to_str(&b, "word/styles.xml")?)?,
            None => Styles::default(),
        };
        let numbering = match package.part("word/numbering.xml")? {
            Some(b) => Numbering::parse(xml::to_str(&b, "word/numbering.xml")?)?,
            None => Numbering::default(),
        };
        let mut comments = match package.part("word/comments.xml")? {
            Some(b) => comments::parse_comments(xml::to_str(&b, "word/comments.xml")?)?,
            None => Vec::new(),
        };
        if let Some(b) = package.part("word/commentsExtended.xml")? {
            comments::apply_extended(xml::to_str(&b, "word/commentsExtended.xml")?, &mut comments)?;
        }
        comments::resolve_anchors(&mut comments, &body.paragraphs);

        let relationships = match package.part("word/_rels/document.xml.rels")? {
            Some(b) => parse_relationships(xml::to_str(&b, "word/_rels/document.xml.rels")?)?,
            None => HashMap::new(),
        };

        let mut doc = Document {
            package,
            paragraphs: body.paragraphs,
            blocks: body.blocks,
            comments,
            styles,
            heading_levels: Vec::new(),
            list_labels: Vec::new(),
            paragraph_blocks: Vec::new(),
            comment_ranges: Vec::new(),
            relationships,
        };
        doc.index(&numbering);
        Ok(doc)
    }

    fn index(&mut self, numbering: &Numbering) {
        let mut counter = numbering.counter();
        self.heading_levels = Vec::with_capacity(self.paragraphs.len());
        self.list_labels = Vec::with_capacity(self.paragraphs.len());
        for p in &self.paragraphs {
            let level = match p.outline_level {
                Some(l) if l < 9 => Some(l),
                Some(_) => None,
                None => self.styles.outline_level(p.style_id.as_deref()),
            };
            self.heading_levels.push(level);
            let num = p
                .numbering
                .or_else(|| self.styles.numbering(p.style_id.as_deref()));
            self.list_labels
                .push(num.and_then(|n| counter.next_label(n.num_id, n.level)));
        }

        self.paragraph_blocks = vec![0; self.paragraphs.len()];
        for (block_index, block) in self.blocks.iter().enumerate() {
            for p in block.paragraph_indices() {
                self.paragraph_blocks[p] = block_index;
            }
        }

        self.comment_ranges = vec![Vec::new(); self.paragraphs.len()];
        for (ci, comment) in self.comments.iter().enumerate() {
            // Replies share the parent's highlight.
            if comment.parent_id.is_some() {
                continue;
            }
            let Some(a) = &comment.anchor else { continue };
            for pi in a.start_paragraph..=a.end_paragraph.min(self.paragraphs.len() - 1) {
                let len = self.paragraphs[pi].char_len();
                let start = if pi == a.start_paragraph {
                    a.start_offset.min(len)
                } else {
                    0
                };
                let mut end = if pi == a.end_paragraph {
                    a.end_offset.min(len)
                } else {
                    len
                };
                if start == end && a.start_paragraph == a.end_paragraph {
                    // Point comments highlight the character they sit on.
                    end = (start + 1).min(len);
                }
                if start < end {
                    self.comment_ranges[pi].push((ci, start, end));
                }
            }
        }
    }

    pub fn heading_level(&self, paragraph: usize) -> Option<u8> {
        self.heading_levels[paragraph]
    }

    pub fn list_label(&self, paragraph: usize) -> Option<&str> {
        self.list_labels[paragraph].as_deref()
    }

    pub fn block_of_paragraph(&self, paragraph: usize) -> usize {
        self.paragraph_blocks[paragraph]
    }

    pub fn style_name(&self, style_id: &str) -> Option<&str> {
        self.styles.name(style_id)
    }

    pub(crate) fn comment_ranges(&self, paragraph: usize) -> &[(usize, usize, usize)] {
        &self.comment_ranges[paragraph]
    }

    /// Characters in the document as if all revisions were accepted, excluding whitespace.
    pub fn char_count(&self) -> usize {
        self.paragraphs
            .iter()
            .map(|p| {
                p.accepted_text()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .count()
            })
            .sum()
    }

    /// Image bytes and MIME type for a relationship id from an [`Inline::Image`].
    pub fn image(&self, rel_id: &str) -> Result<Option<(Vec<u8>, &'static str)>> {
        let Some(target) = self.relationships.get(rel_id) else {
            return Ok(None);
        };
        let path = resolve_target("word", target);
        let mime = match path
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("bmp") => "image/bmp",
            Some("svg") => "image/svg+xml",
            Some("webp") => "image/webp",
            _ => "application/octet-stream",
        };
        Ok(self.package.part(&path)?.map(|b| (b, mime)))
    }

    pub fn is_modified(&self) -> bool {
        self.package.is_modified()
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.package.to_bytes()
    }

    /// Saves atomically: writes a temporary file next to the target, checks
    /// that it parses back, then moves it into place.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.to_bytes()?;
        Document::from_bytes(bytes.clone())
            .map_err(|e| Error::Invalid(format!("保存结果校验失败，未写入文件: {e}")))?;
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document.docx");
        let tmp = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(())
    }
}

fn parse_relationships(xml: &str) -> Result<HashMap<String, String>> {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let mut reader = Reader::from_str(xml);
    let mut map = HashMap::new();
    loop {
        match reader
            .read_event()
            .map_err(|e| Error::xml("word/_rels/document.xml.rels", e))?
        {
            Event::Start(e) | Event::Empty(e) if xml::local(&e) == "Relationship" => {
                if xml::attr(&e, "TargetMode").as_deref() == Some("External") {
                    continue;
                }
                if let (Some(id), Some(target)) = (xml::attr(&e, "Id"), xml::attr(&e, "Target")) {
                    map.insert(id, target);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// Resolves a relationship target relative to the source part's folder.
fn resolve_target(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}
