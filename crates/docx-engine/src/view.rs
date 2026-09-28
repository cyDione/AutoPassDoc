//! Serializable views sent to the UI. The UI only ever asks for a window of
//! blocks at a time, so even very large documents stay cheap to display.

use serde::Serialize;

use crate::Document;
use crate::model::{Block, Inline, Revision, RunFormat};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub block_count: usize,
    pub paragraph_count: usize,
    pub char_count: usize,
    pub outline: Vec<OutlineItem>,
    pub comments: Vec<CommentView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    pub block_index: usize,
    pub paragraph_index: usize,
    pub level: u8,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    pub id: String,
    pub author: String,
    pub initials: Option<String>,
    pub date: Option<String>,
    pub text: String,
    pub parent_id: Option<String>,
    pub done: bool,
    /// Top-level block to scroll to; `None` when the comment has no anchor.
    pub block_index: Option<usize>,
    pub paragraph_index: Option<usize>,
    /// The commented text, shortened for display.
    pub quote: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BlockView {
    #[serde(rename_all = "camelCase")]
    Paragraph {
        index: usize,
        paragraph: ParagraphView,
    },
    #[serde(rename_all = "camelCase")]
    Table {
        index: usize,
        rows: Vec<Vec<CellView>>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellView {
    pub grid_span: u32,
    pub v_merge: Option<bool>,
    pub paragraphs: Vec<ParagraphView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphView {
    pub index: usize,
    pub heading_level: Option<u8>,
    pub list_label: Option<String>,
    pub align: Option<String>,
    pub spans: Vec<SpanView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanView {
    pub text: String,
    #[serde(flatten)]
    pub format: RunFormat,
    pub revision: Revision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_author: Option<String>,
    pub inline: Inline,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comment_ids: Vec<String>,
}

const QUOTE_LIMIT: usize = 120;

impl Document {
    pub fn summary(&self) -> DocumentSummary {
        let outline = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(block_index, block)| match block {
                Block::Paragraph(pi) => self.heading_level(*pi).map(|level| {
                    let p = &self.paragraphs[*pi];
                    let label = self.list_label(*pi).unwrap_or_default();
                    OutlineItem {
                        block_index,
                        paragraph_index: *pi,
                        level,
                        text: format!("{label}{}", p.accepted_text().trim()),
                    }
                }),
                Block::Table(_) => None,
            })
            .filter(|item| !item.text.is_empty())
            .collect();

        let comments = self
            .comments
            .iter()
            .map(|c| CommentView {
                id: c.id.clone(),
                author: c.author.clone(),
                initials: c.initials.clone(),
                date: c.date.clone(),
                text: c.text.clone(),
                parent_id: c.parent_id.clone(),
                done: c.done,
                block_index: c
                    .anchor
                    .as_ref()
                    .map(|a| self.block_of_paragraph(a.start_paragraph)),
                paragraph_index: c.anchor.as_ref().map(|a| a.start_paragraph),
                quote: c.anchor.as_ref().map(|a| self.quote(a)).unwrap_or_default(),
            })
            .collect();

        DocumentSummary {
            block_count: self.blocks.len(),
            paragraph_count: self.paragraphs.len(),
            char_count: self.char_count(),
            outline,
            comments,
        }
    }

    fn quote(&self, a: &crate::CommentAnchor) -> String {
        let mut out = String::new();
        for pi in a.start_paragraph..=a.end_paragraph {
            let p = &self.paragraphs[pi];
            let chars: Vec<char> = p.text.chars().collect();
            let start = if pi == a.start_paragraph {
                a.start_offset.min(chars.len())
            } else {
                0
            };
            let end = if pi == a.end_paragraph {
                a.end_offset.min(chars.len())
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
            if out.chars().count() > QUOTE_LIMIT {
                break;
            }
            if pi != a.end_paragraph {
                out.push(' ');
            }
        }
        let trimmed = out.trim();
        if trimmed.chars().count() > QUOTE_LIMIT {
            let short: String = trimmed.chars().take(QUOTE_LIMIT).collect();
            format!("{short}…")
        } else {
            trimmed.to_string()
        }
    }

    /// Views for blocks `start..end` (clamped to the document).
    pub fn blocks_view(&self, start: usize, end: usize) -> Vec<BlockView> {
        let end = end.min(self.blocks.len());
        (start.min(end)..end)
            .map(|index| match &self.blocks[index] {
                Block::Paragraph(pi) => BlockView::Paragraph {
                    index,
                    paragraph: self.paragraph_view(*pi),
                },
                Block::Table(t) => BlockView::Table {
                    index,
                    rows: t
                        .rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|cell| CellView {
                                    grid_span: cell.grid_span,
                                    v_merge: cell.v_merge,
                                    paragraphs: cell
                                        .paragraphs
                                        .iter()
                                        .map(|pi| self.paragraph_view(*pi))
                                        .collect(),
                                })
                                .collect()
                        })
                        .collect(),
                },
            })
            .collect()
    }

    pub fn paragraph_view(&self, index: usize) -> ParagraphView {
        let p = &self.paragraphs[index];
        let ranges = self.comment_ranges(index);
        let chars: Vec<char> = p.text.chars().collect();
        let mut spans = Vec::new();
        for run in &p.runs {
            // Split each run where comment ranges begin or end.
            let mut cuts = vec![run.start, run.end];
            for &(_, s, e) in ranges {
                for c in [s, e] {
                    if c > run.start && c < run.end {
                        cuts.push(c);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            for w in cuts.windows(2) {
                let (s, e) = (w[0], w[1]);
                let comment_ids = ranges
                    .iter()
                    .filter(|&&(_, cs, ce)| cs < e && ce > s)
                    .map(|&(ci, _, _)| self.comments[ci].id.clone())
                    .collect();
                spans.push(SpanView {
                    text: chars[s..e].iter().collect(),
                    format: run.format,
                    revision: run.revision,
                    revision_author: run.revision_author.clone(),
                    inline: run.inline.clone(),
                    comment_ids,
                });
            }
        }
        ParagraphView {
            index,
            heading_level: self.heading_level(index),
            list_label: self.list_label(index).map(str::to_string),
            align: p.align.clone(),
            spans,
        }
    }
}
