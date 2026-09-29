//! Knowledge-base glue: retrieval for fixes (keyword + vector search, then
//! the reranker), background embedding and the knowledge-base page.

use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::sync::atomic::Ordering;

use kb::{DocMeta, Hit, KbDocument, KnowledgeBase, SearchQuery};
use serde::Serialize;

use crate::core::{Core, RoleName, Target};
use crate::error::{Error, Result};

/// Chunks embedded per model call.
const EMBED_BATCH: usize = 32;
/// Fused results the reranker sees.
const RERANK_POOL: usize = 30;

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

/// A search result for the knowledge-base page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitView {
    #[serde(flatten)]
    pub hit: Hit,
    /// Set when a reranker is configured.
    pub rerank_score: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    pub documents: usize,
    pub chunks: usize,
    /// Chunks embedded with the configured embedding model.
    pub embedded: usize,
    pub embedding_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub file_name: String,
    pub doc_id: Option<i64>,
    pub chunks: usize,
    pub unchanged: bool,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

/// Search results plus notes on what was skipped (no vectors, reranker
/// failure...).
pub struct Found {
    pub hits: Vec<HitView>,
    pub warnings: Vec<String>,
}

impl Core {
    /// The knowledge base, opened on first use in `<data dir>/kb`.
    fn kb(&self) -> Result<MutexGuard<'_, Option<KnowledgeBase>>> {
        let mut guard = self.kb.lock().unwrap();
        if guard.is_none() {
            *guard = Some(KnowledgeBase::open(&self.data_dir().join("kb"))?);
        }
        Ok(guard)
    }

    /// Runs `f` on the knowledge base.
    pub fn with_kb<T>(&self, f: impl FnOnce(&mut KnowledgeBase) -> kb::Result<T>) -> Result<T> {
        let mut guard = self.kb()?;
        Ok(f(guard.as_mut().expect("opened"))?)
    }

    pub fn kb_documents(&self) -> Result<Vec<KbDocument>> {
        self.with_kb(|kb| kb.documents())
    }

    pub fn kb_stats(&self) -> Result<StatsView> {
        let model = self.embedding_model()?;
        let stats = self.with_kb(|kb| kb.stats())?;
        Ok(StatsView {
            documents: stats.documents,
            chunks: stats.chunks,
            embedded: model
                .as_ref()
                .and_then(|m| stats.embedded.get(m))
                .copied()
                .unwrap_or(0),
            embedding_model: model,
        })
    }

    /// Imports files one by one; a file that fails is reported and skipped.
    /// `progress(done, total, current file)`.
    pub fn kb_import(
        &self,
        paths: &[PathBuf],
        progress: impl Fn(usize, usize, &str),
    ) -> Vec<ImportResult> {
        let mut out = Vec::with_capacity(paths.len());
        for (i, path) in paths.iter().enumerate() {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            progress(i, paths.len(), &file_name);
            out.push(match self.with_kb(|kb| kb.import_file(path)) {
                Ok(r) => ImportResult {
                    file_name,
                    doc_id: Some(r.doc_id),
                    chunks: r.chunks,
                    unchanged: r.unchanged,
                    warnings: r.warnings,
                    error: None,
                },
                Err(e) => ImportResult {
                    file_name,
                    doc_id: None,
                    chunks: 0,
                    unchanged: false,
                    warnings: Vec::new(),
                    error: Some(e.to_string()),
                },
            });
        }
        progress(paths.len(), paths.len(), "");
        out
    }

    pub fn kb_remove(&self, doc_id: i64) -> Result<()> {
        self.with_kb(|kb| kb.remove_document(doc_id))
    }

    pub fn kb_update_meta(&self, doc_id: i64, meta: &DocMeta) -> Result<KbDocument> {
        self.with_kb(|kb| {
            kb.update_metadata(doc_id, meta)?;
            kb.document(doc_id)
        })?
        .ok_or_else(|| Error::Invalid("资料不存在".into()))
    }

    /// The model id vectors are stored under, when an embedding model is set.
    fn embedding_model(&self) -> Result<Option<String>> {
        let rm = self.settings()?.roles.embedding;
        Ok(rm.is_set().then_some(rm.model))
    }

    /// Embeds every chunk that has no vector from the configured embedding
    /// model. `progress(done, total)`. Returns the number embedded now.
    pub async fn kb_embed(&self, progress: impl Fn(usize, usize)) -> Result<usize> {
        let target = self.require(RoleName::Embedding)?;
        if self.embedding.swap(true, Ordering::AcqRel) {
            return Err(Error::Invalid("正在生成向量，请稍候".into()));
        }
        let result = self.embed_pending(&target, &progress).await;
        self.embedding.store(false, Ordering::Release);
        result
    }

    async fn embed_pending(
        &self,
        target: &Target,
        progress: &impl Fn(usize, usize),
    ) -> Result<usize> {
        let model = target.model.as_str();
        let mut embedded = 0;
        loop {
            let (batch, (done, total)) = self.with_kb(|kb| {
                Ok((
                    kb.pending_embeddings(model, EMBED_BATCH)?,
                    kb.embedding_progress(model)?,
                ))
            })?;
            progress(done, total);
            if batch.is_empty() {
                return Ok(embedded);
            }
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            let vectors = self
                .client()
                .embed(&target.provider, model, &texts)
                .await
                .map_err(|e| Error::Invalid(format!("向量模型调用失败：{e}")))?;
            let items: Vec<(i64, Vec<f32>)> =
                batch.iter().map(|(id, _)| *id).zip(vectors).collect();
            self.with_kb(|kb| kb.store_embeddings(model, &items))
                .map_err(|e| match e {
                    Error::Kb(kb::Error::DimensionMismatch { .. }) => Error::Invalid(format!(
                        "向量维度与之前用「{model}」生成的不一致（{e}）。换了同名的不同模型时，请先在知识库页点「清空向量」"
                    )),
                    e => e,
                })?;
            embedded += items.len();
        }
    }

    /// Deletes the vectors of the configured embedding model.
    pub fn kb_clear_embeddings(&self) -> Result<()> {
        if self.embedding.load(Ordering::Acquire) {
            return Err(Error::Invalid("正在生成向量，请等它结束后再清空".into()));
        }
        if let Some(model) = self.embedding_model()? {
            self.with_kb(|kb| kb.clear_embeddings(&model))?;
        }
        Ok(())
    }

    /// Hybrid search: keywords, plus vectors when an embedding model is set
    /// and the knowledge base has its vectors, then the reranker when set.
    pub async fn kb_search(&self, text: &str, limit: usize) -> Result<Found> {
        let mut warnings = Vec::new();
        let mut embedding = None;
        if let Some(target) = self.target(RoleName::Embedding)? {
            let (done, total) = self.with_kb(|kb| kb.embedding_progress(&target.model))?;
            if done == 0 && total > 0 {
                warnings.push("知识库还没有生成向量，本次只用关键词检索".into());
            } else if total > 0 {
                if done < total {
                    warnings.push(format!(
                        "还有 {} 个片段没有向量，它们只能被关键词检索到",
                        total - done
                    ));
                }
                match self
                    .client()
                    .embed(&target.provider, &target.model, &[text.to_string()])
                    .await
                {
                    Ok(mut v) => embedding = v.pop().map(|v| (target.model.clone(), v)),
                    Err(e) => warnings.push(format!("向量模型调用失败，本次只用关键词检索：{e}")),
                }
            }
        }
        let rerank = self.target(RoleName::Rerank)?;
        let pool = if rerank.is_some() {
            RERANK_POOL.max(limit)
        } else {
            limit
        };
        let query = SearchQuery {
            text: text.to_string(),
            embedding,
            limit: pool,
            doc_ids: None,
        };
        let hits = self.with_kb(|kb| kb.search(&query))?;
        let mut views: Vec<HitView> = hits
            .into_iter()
            .map(|hit| HitView {
                hit,
                rerank_score: None,
            })
            .collect();
        if let Some(target) = rerank.filter(|_| views.len() > 1) {
            let docs: Vec<String> = views
                .iter()
                .map(|v| {
                    let h = &v.hit;
                    if h.heading_path.is_empty() {
                        format!("《{}》{}", h.title, h.text)
                    } else {
                        format!("《{}》{}\n{}", h.title, h.heading_path.join(" > "), h.text)
                    }
                })
                .collect();
            match self
                .client()
                .rerank(&target.provider, &target.model, text, &docs)
                .await
            {
                Ok(scores) => {
                    for (v, s) in views.iter_mut().zip(scores) {
                        v.rerank_score = s.is_finite().then_some(s);
                    }
                    views.sort_by(|a, b| {
                        b.rerank_score
                            .unwrap_or(f32::NEG_INFINITY)
                            .total_cmp(&a.rerank_score.unwrap_or(f32::NEG_INFINITY))
                    });
                }
                Err(e) => warnings.push(format!("重排模型调用失败，按融合排序：{e}")),
            }
        }
        views.truncate(limit);
        Ok(Found {
            hits: views,
            warnings,
        })
    }
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Passages for a fix, best first, with notes on what was skipped.
pub async fn retrieve(
    core: &Core,
    query: &str,
    limit: usize,
) -> Result<(Vec<Citation>, Vec<String>)> {
    let empty = core.with_kb(|kb| Ok(kb.stats()?.chunks == 0))?;
    if empty {
        return Ok((Vec::new(), Vec::new()));
    }
    let found = core.kb_search(query, limit).await?;
    let mut seen_parents = std::collections::HashSet::new();
    let citations = found
        .hits
        .into_iter()
        .map(|v| v.hit)
        // Several children of one article carry the same parent text.
        .filter(|h| {
            let unit = if h.parent_text.is_empty() {
                &h.text
            } else {
                &h.parent_text
            };
            seen_parents.insert((h.doc_id, unit.clone()))
        })
        .enumerate()
        .map(|(i, h)| Citation {
            n: i + 1,
            doc_id: h.doc_id,
            chunk_id: h.chunk_id,
            stored_path: path_string(&h.stored_path),
            file_name: h.file_name,
            title: h.title,
            heading_path: h.heading_path,
            text: h.text,
            parent_text: h.parent_text,
            char_start: h.char_start,
            char_end: h.char_end,
        })
        .collect();
    Ok((citations, found.warnings))
}
