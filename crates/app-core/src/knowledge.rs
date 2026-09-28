//! Knowledge-base glue: retrieval for fixes (keyword + vector search, then
//! the reranker), background embedding and the knowledge-base page.

use serde::Serialize;

use crate::core::Core;
use crate::error::Result;

/// A knowledge-base passage cited by a fix.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub n: usize,
    pub doc_id: i64,
    pub chunk_id: i64,
    pub file_name: String,
    pub title: String,
    pub heading_path: Vec<String>,
    pub text: String,
    /// The enclosing article or section, given to the model.
    #[serde(skip)]
    pub parent_text: String,
    pub stored_path: String,
    pub char_start: usize,
    pub char_end: usize,
}

/// Passages for a fix, best first.
pub async fn retrieve(
    _core: &Core,
    _query: &str,
    _limit: usize,
) -> Result<(Vec<Citation>, Vec<String>)> {
    Ok((Vec::new(), Vec::new()))
}
