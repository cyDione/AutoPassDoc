use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Outcome of [`KnowledgeBase::import_file`](crate::KnowledgeBase::import_file).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub doc_id: i64,
    /// Number of child chunks indexed for the document.
    pub chunks: usize,
    /// The same content was already in the knowledge base, so nothing changed.
    pub unchanged: bool,
    /// Problems the user should know about, e.g. a scanned PDF without a text layer.
    pub warnings: Vec<String>,
}

/// An imported document.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbDocument {
    pub id: i64,
    /// Display title: `meta.title`, or the file name without extension.
    pub title: String,
    pub file_name: String,
    /// The copy inside the knowledge base folder; open this one.
    pub stored_path: PathBuf,
    /// Where the file was imported from.
    pub original_path: PathBuf,
    /// `docx`, `pdf`, `txt`, `md`, or `image` (PNG/JPEG, online parsers only).
    pub format: String,
    /// What produced the text: `builtin`, or the online parser's name
    /// (`mineru`, `paddleocr`).
    pub parser: String,
    pub meta: DocMeta,
    pub chunk_count: usize,
    /// Characters in the document text, excluding whitespace.
    pub char_count: usize,
    /// Unix seconds.
    pub imported_at: i64,
    pub sha256: String,
    /// Warnings from the last import, e.g. a PDF that needs OCR.
    pub warnings: Vec<String>,
}

/// A document as the viewer shows it: its lines with detected headings and
/// where each chunk sits.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentView {
    pub document: KbDocument,
    /// The full text split at `\n`; line `i` starts after the `i` newlines
    /// before it, so chunk offsets can be mapped to lines.
    pub lines: Vec<ViewLine>,
    /// Child chunks in text order.
    pub chunks: Vec<ChunkSpan>,
}

/// One line of [`DocumentView::lines`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewLine {
    pub text: String,
    /// Rank of the heading the line starts with, smaller is higher:
    /// 0 附件, 1 编/部分, 2 章, 3 节, 4 条, 5 一、, 6 （一）, 7 1., 8 （1）, 9 ①.
    pub heading_level: Option<u8>,
}

/// Where a child chunk sits in the full text, in chars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkSpan {
    pub chunk_id: i64,
    pub char_start: usize,
    pub char_end: usize,
    pub heading_path: Vec<String>,
}

/// Document metadata, extracted on import and correctable by the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocMeta {
    pub title: Option<String>,
    /// 文号, stored with 〔〕 brackets, e.g. `国办发〔2024〕12号`.
    pub doc_number: Option<String>,
    /// 发文机关.
    pub issuer: Option<String>,
    /// 成文日期 as `YYYY-MM-DD`.
    pub date: Option<String>,
}

/// A hybrid search request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchQuery {
    pub text: String,
    /// Query embedding and the name of the model that produced it. Vector
    /// search runs only when chunks have embeddings from the same model.
    pub embedding: Option<(String, Vec<f32>)>,
    /// Number of fused results to return; 0 means the default of 30.
    pub limit: usize,
    /// Only search these documents.
    pub doc_ids: Option<Vec<i64>>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            embedding: None,
            limit: 30,
            doc_ids: None,
        }
    }
}

/// A search result: a child chunk plus the parent unit (条/节) it belongs to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub file_name: String,
    /// Display title of the document.
    pub title: String,
    pub stored_path: PathBuf,
    /// Headings enclosing the chunk, outermost first, e.g. `["第三章 监督管理", "第二十条"]`.
    pub heading_path: Vec<String>,
    /// The child chunk.
    pub text: String,
    /// The enclosing article or section, for LLM context.
    pub parent_text: String,
    /// Offsets of `text` in the document's full text, in chars (Unicode
    /// scalar values). Line `i` of the full text is paragraph `i` of a .docx.
    pub char_start: usize,
    pub char_end: usize,
    /// 1-based rank in the keyword results.
    pub keyword_rank: Option<usize>,
    /// 1-based rank in the vector results.
    pub vector_rank: Option<usize>,
    /// Reciprocal rank fusion score, Σ 1/(60 + rank).
    pub score: f32,
}

/// Size of the knowledge base.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbStats {
    pub documents: usize,
    pub chunks: usize,
    /// Embedded chunks per embedding model.
    pub embedded: BTreeMap<String, usize>,
}

/// Outcome of [`KnowledgeBase::merge_from`](crate::KnowledgeBase::merge_from).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeStats {
    /// Documents copied in.
    pub documents_added: usize,
    /// Documents whose content (SHA-256) was already here.
    pub documents_skipped: usize,
    pub chunks_added: usize,
    pub embeddings_added: usize,
    /// Files of added documents that were missing from the source folder;
    /// their text is searchable, but the original cannot be opened.
    pub missing_files: Vec<String>,
    /// Embedding models whose vectors were not copied because their
    /// dimension differs from the vectors stored here under the same name.
    pub skipped_models: Vec<String>,
}
