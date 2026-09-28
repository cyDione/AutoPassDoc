//! Native .docx engine for AutoPassDoc.
//!
//! Parses WordprocessingML into a lightweight model for display (paragraphs,
//! tables, comments, revisions, list labels) while keeping the original
//! package bytes, so saving copies everything that was not edited verbatim.

mod annotate;
mod body;
mod comments;
pub mod diff;
pub mod edit;
mod error;
mod history;
pub mod model;
mod numbering;
mod package;
mod styles;
pub mod testgen;
pub mod view;
mod xml;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

pub use edit::{EditMode, EditOptions, EditReport, now_iso};
pub use error::{Error, Result};
pub use model::*;
pub use package::Package;

use annotate::{COMMENTS_PART, CONTENT_TYPES_PART, DOCUMENT_RELS_PART, EXTENDED_PART};
use history::History;
use numbering::Numbering;
use styles::Styles;

const BOM: &[u8] = b"\xEF\xBB\xBF";

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
    history: History,
}

/// Everything read from the package parts.
struct Parsed {
    paragraphs: Vec<Paragraph>,
    blocks: Vec<Block>,
    comments: Vec<Comment>,
    styles: Styles,
    numbering: Numbering,
    relationships: HashMap<String, String>,
}

fn parse_package(package: &Package) -> Result<Parsed> {
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
    let mut comments = match package.part(COMMENTS_PART)? {
        Some(b) => comments::parse_comments(xml::to_str(&b, COMMENTS_PART)?)?,
        None => Vec::new(),
    };
    if let Some(b) = package.part(EXTENDED_PART)? {
        comments::apply_extended(xml::to_str(&b, EXTENDED_PART)?, &mut comments)?;
    }
    comments::resolve_anchors(&mut comments, &body.paragraphs);

    let relationships = match package.part(DOCUMENT_RELS_PART)? {
        Some(b) => parse_relationships(xml::to_str(&b, DOCUMENT_RELS_PART)?)?,
        None => HashMap::new(),
    };
    Ok(Parsed {
        paragraphs: body.paragraphs,
        blocks: body.blocks,
        comments,
        styles,
        numbering,
        relationships,
    })
}

impl Document {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let package = Package::from_bytes(bytes)?;
        let parsed = parse_package(&package)?;
        let mut doc = Document {
            package,
            paragraphs: Vec::new(),
            blocks: Vec::new(),
            comments: Vec::new(),
            styles: Styles::default(),
            heading_levels: Vec::new(),
            list_labels: Vec::new(),
            paragraph_blocks: Vec::new(),
            comment_ranges: Vec::new(),
            relationships: HashMap::new(),
            history: History::default(),
        };
        doc.install(parsed);
        Ok(doc)
    }

    fn install(&mut self, parsed: Parsed) {
        self.paragraphs = parsed.paragraphs;
        self.blocks = parsed.blocks;
        self.comments = parsed.comments;
        self.styles = parsed.styles;
        self.relationships = parsed.relationships;
        self.index(&parsed.numbering);
    }

    fn reload(&mut self) -> Result<()> {
        let parsed = parse_package(&self.package)?;
        self.install(parsed);
        Ok(())
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

    /// Whether any part differs from the file that was opened.
    pub fn is_modified(&self) -> bool {
        self.package.is_modified()
    }

    /// Whether there are edits since the document was opened or last saved.
    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    /// Undoes the last edit and returns its label.
    pub fn undo(&mut self) -> Result<Option<String>> {
        let Some(step) = self.history.take_undo() else {
            return Ok(None);
        };
        step.revert(&mut self.package)?;
        self.reload()?;
        let label = step.label().to_string();
        self.history.push_undone(step);
        Ok(Some(label))
    }

    /// Redoes the last undone edit and returns its label.
    pub fn redo(&mut self) -> Result<Option<String>> {
        let Some(step) = self.history.take_redo() else {
            return Ok(None);
        };
        step.apply(&mut self.package)?;
        self.reload()?;
        let label = step.label().to_string();
        self.history.push_redone(step);
        Ok(Some(label))
    }

    /// Writes new part contents as one undoable step and re-reads the
    /// document. If the result does not parse, nothing changes.
    fn commit(&mut self, label: &str, parts: Vec<(&str, String)>) -> Result<()> {
        let parts = parts
            .into_iter()
            .map(|(name, xml)| {
                let bom = self
                    .package
                    .part(name)
                    .ok()
                    .flatten()
                    .is_some_and(|b| b.starts_with(BOM));
                let mut bytes = if bom { BOM.to_vec() } else { Vec::new() };
                bytes.extend_from_slice(xml.as_bytes());
                (name.to_string(), bytes)
            })
            .collect();
        let step = self.history.record(&mut self.package, label, parts)?;
        if let Err(e) = self.reload() {
            step.revert(&mut self.package)?;
            self.reload()?;
            return Err(Error::Edit(format!("修改后的文档无法解析，已撤回：{e}")));
        }
        self.history.push(step);
        Ok(())
    }

    fn part_string(&self, name: &str) -> Result<Option<String>> {
        self.package
            .part(name)?
            .map(|b| xml::to_str(&b, name).map(str::to_string))
            .transpose()
    }

    fn required_string(&self, name: &str) -> Result<String> {
        self.part_string(name)?
            .ok_or_else(|| Error::MissingPart(name.to_string()))
    }

    pub fn comment(&self, id: &str) -> Option<&Comment> {
        self.comments.iter().find(|c| c.id == id)
    }

    /// The paragraph's text as an AI edit sees it: tracked deletions left
    /// out, images, notes and equations as placeholders like `⟦图⟧` that
    /// must come back unchanged.
    pub fn editable_text(&self, paragraph: usize) -> String {
        edit::EditMap::new(&self.paragraphs[paragraph]).text
    }

    /// Replaces the editable text of each listed paragraph, as one undoable
    /// step. Only the changed words are rewritten.
    pub fn replace_paragraphs(
        &mut self,
        changes: &[(usize, String)],
        opts: &EditOptions,
        label: &str,
    ) -> Result<EditReport> {
        let doc_xml = self.required_string(DOCUMENT_PART)?;
        let comments_xml = self.part_string(COMMENTS_PART)?.unwrap_or_default();
        let mut ids = edit::Ids::above(&[&doc_xml, &comments_xml]);
        let mut report = EditReport::default();
        let mut replacements = Vec::new();
        let mut seen = HashSet::new();
        for (index, text) in changes {
            let p = self
                .paragraphs
                .get(*index)
                .ok_or_else(|| Error::Edit(format!("段落 {index} 不存在")))?;
            if !seen.insert(*index) {
                return Err(Error::Edit(format!("段落 {index} 重复修改")));
            }
            let ops = edit::ops_for(p, text)?;
            if ops.is_empty() {
                continue;
            }
            for op in &ops {
                match op {
                    edit::Op::Delete(r) => report.deleted_chars += r.len(),
                    edit::Op::Insert(_, t) => report.inserted_chars += t.chars().count(),
                }
            }
            report.paragraphs += 1;
            let xml = edit::rewrite_paragraph(&doc_xml, p, &ops, opts, &mut ids)?;
            replacements.push((p.span.start..p.span.end, xml));
        }
        if replacements.is_empty() {
            return Ok(report);
        }
        let new_xml = annotate::splice(&doc_xml, replacements);
        self.commit(label, vec![(DOCUMENT_PART, new_xml)])?;
        Ok(report)
    }

    /// Comments `ids` plus their replies.
    fn threads(&self, ids: &[&str]) -> Result<Vec<&Comment>> {
        for id in ids {
            if self.comment(id).is_none() {
                return Err(Error::Edit(format!("找不到批注 {id}")));
            }
        }
        Ok(self
            .comments
            .iter()
            .filter(|c| {
                ids.contains(&c.id.as_str())
                    || c.parent_id.as_deref().is_some_and(|p| ids.contains(&p))
            })
            .collect())
    }

    /// Returns the commentsExtended.xml content, creating the part (and its
    /// content type and relationship, pushed onto `parts`) when missing.
    fn extended_or_new(&self, parts: &mut Vec<(&'static str, String)>) -> Result<String> {
        if let Some(xml) = self.part_string(EXTENDED_PART)? {
            return Ok(xml);
        }
        let (content_types, rels) = annotate::register_extended(
            &self.required_string(CONTENT_TYPES_PART)?,
            &self.required_string(DOCUMENT_RELS_PART)?,
        )?;
        parts.push((CONTENT_TYPES_PART, content_types));
        parts.push((DOCUMENT_RELS_PART, rels));
        Ok(annotate::new_extended_part())
    }

    /// Marks comment threads resolved (or open again), as Word's "Resolve".
    pub fn set_comments_done(&mut self, ids: &[&str], done: bool) -> Result<()> {
        let targets = self.threads(ids)?;
        let doc_xml = self.required_string(DOCUMENT_PART)?;
        let comments_xml = self.required_string(COMMENTS_PART)?;
        let existing = self.part_string(EXTENDED_PART)?;
        if existing.is_none() && !done {
            return Ok(());
        }
        let mut para_ids =
            annotate::ParaIds::new(&[&doc_xml, &comments_xml, existing.as_deref().unwrap_or("")]);
        let missing: HashSet<&str> = targets
            .iter()
            .filter(|c| c.para_id.is_none())
            .map(|c| c.id.as_str())
            .collect();
        let (comments_new, assigned) =
            annotate::ensure_para_ids(&comments_xml, &missing, &mut para_ids)?;
        let para_of: Vec<String> = targets
            .iter()
            .filter_map(|c| c.para_id.clone().or_else(|| assigned.get(&c.id).cloned()))
            .collect();
        let entries: Vec<annotate::ExtendedEntry> = para_of
            .iter()
            .map(|p| annotate::ExtendedEntry {
                para_id: p,
                parent_para_id: None,
                done,
            })
            .collect();
        let mut parts = Vec::new();
        let extended = self.extended_or_new(&mut parts)?;
        parts.push((EXTENDED_PART, annotate::set_extended(&extended, &entries)?));
        if !assigned.is_empty() {
            parts.push((COMMENTS_PART, comments_new));
        }
        let label = if done {
            "标记批注已解决"
        } else {
            "重新打开批注"
        };
        self.commit(label, parts)
    }

    /// Changes the author (and optionally initials) of comments.
    pub fn set_comment_authors(
        &mut self,
        ids: &[&str],
        author: &str,
        initials: Option<&str>,
    ) -> Result<()> {
        self.threads(ids)?;
        let comments_xml = self.required_string(COMMENTS_PART)?;
        let ids: HashSet<&str> = ids.iter().copied().collect();
        let new = annotate::set_authors(&comments_xml, &ids, author, initials)?;
        self.commit("修改批注作者", vec![(COMMENTS_PART, new)])
    }

    /// Adds a reply to a comment thread and returns the new comment id.
    pub fn add_reply(
        &mut self,
        comment_id: &str,
        author: &str,
        initials: Option<&str>,
        text: &str,
    ) -> Result<String> {
        let comment = self
            .comment(comment_id)
            .ok_or_else(|| Error::Edit(format!("找不到批注 {comment_id}")))?;
        // Word threads are flat: a reply to a reply belongs to the thread root.
        let root_id = comment.parent_id.clone().unwrap_or(comment.id.clone());
        let root = self.comment(&root_id).unwrap_or(comment);
        let (root_para, root_done) = (root.para_id.clone(), root.done);

        let doc_xml = self.required_string(DOCUMENT_PART)?;
        let comments_xml = self.required_string(COMMENTS_PART)?;
        let existing = self.part_string(EXTENDED_PART)?;
        let mut para_ids =
            annotate::ParaIds::new(&[&doc_xml, &comments_xml, existing.as_deref().unwrap_or("")]);
        let missing: HashSet<&str> = if root_para.is_none() {
            HashSet::from([root_id.as_str()])
        } else {
            HashSet::new()
        };
        let (comments_xml, assigned) =
            annotate::ensure_para_ids(&comments_xml, &missing, &mut para_ids)?;
        let root_para = root_para
            .or_else(|| assigned.get(&root_id).cloned())
            .ok_or_else(|| Error::Edit("无法定位原批注".into()))?;

        let new_id = (self
            .comments
            .iter()
            .filter_map(|c| c.id.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1)
        .to_string();
        let new_para = para_ids.next();
        let comments_xml = annotate::append_comment(
            &comments_xml,
            &annotate::NewComment {
                id: &new_id,
                author,
                initials,
                date: &now_iso(),
                text: &edit::sanitize(text),
                para_id: &new_para,
            },
            &mut para_ids,
        )?;
        let doc_xml = annotate::add_reply_marks(&doc_xml, &root_id, &new_id)?;

        let mut parts = Vec::new();
        let extended = self.extended_or_new(&mut parts)?;
        let entries = [
            annotate::ExtendedEntry {
                para_id: &root_para,
                parent_para_id: None,
                done: root_done,
            },
            annotate::ExtendedEntry {
                para_id: &new_para,
                parent_para_id: Some(&root_para),
                done: false,
            },
        ];
        parts.push((EXTENDED_PART, annotate::set_extended(&extended, &entries)?));
        parts.push((COMMENTS_PART, comments_xml));
        parts.push((DOCUMENT_PART, doc_xml));
        self.commit("回复批注", parts)?;
        Ok(new_id)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.package.to_bytes()
    }

    /// Saves atomically: writes a temporary file next to the target, checks
    /// that it parses back, then moves it into place.
    pub fn save(&mut self, path: impl AsRef<Path>) -> Result<()> {
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
        self.history.mark_saved();
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
