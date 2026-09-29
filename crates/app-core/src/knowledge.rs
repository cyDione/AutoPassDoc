//! Knowledge-base glue: retrieval for fixes (keyword + vector search, then
//! the reranker), background embedding and the knowledge-base page.

use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::sync::atomic::Ordering;

use kb::{DocMeta, DocumentView, Hit, KbDocument, KnowledgeBase, SearchQuery};
use serde::Serialize;

use crate::core::{Core, RoleName, Target};
use crate::enhanced::{EnhancedError, EnhancedParser};
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
    /// What read the file: `builtin`, `mineru` or `paddleocr`. For a
    /// failed import, the parser that was tried.
    pub parser: String,
}

impl ImportResult {
    fn ok(file_name: String, r: kb::ImportReport, parser: &str) -> Self {
        Self {
            file_name,
            doc_id: Some(r.doc_id),
            chunks: r.chunks,
            unchanged: r.unchanged,
            warnings: r.warnings,
            error: None,
            parser: parser.to_string(),
        }
    }

    fn failed(file_name: String, error: String, parser: &str) -> Self {
        Self {
            file_name,
            doc_id: None,
            chunks: 0,
            unchanged: false,
            warnings: Vec::new(),
            error: Some(error),
            parser: parser.to_string(),
        }
    }
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

    /// The files an import of `paths` covers: folders expanded
    /// recursively (with images when enhanced parsing is on), see
    /// [`kb::expand_import_paths`].
    pub fn kb_expand_import(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let images = self.enhanced_parser()?.is_some();
        Ok(kb::expand_import_paths(paths, images))
    }

    /// Number of files an import of `paths` covers, to confirm before a
    /// folder import.
    pub fn kb_count_import(&self, paths: &[PathBuf]) -> Result<usize> {
        Ok(self.kb_expand_import(paths)?.len())
    }

    /// Imports files and folders one file at a time; a file that fails is
    /// reported and skipped. `progress(done, total, current)`, where
    /// `current` is the file name, followed during online parsing by what
    /// the service is doing, e.g. `扫描件.pdf（MinerU 解析中 3/10 页）`.
    ///
    /// With enhanced parsing on (an online parser chosen and its key
    /// saved), PDFs and images go to that service; a PDF it fails on is
    /// read by the built-in parser instead, with a warning.
    pub async fn kb_import(
        &self,
        paths: &[PathBuf],
        progress: impl Fn(usize, usize, &str) + Sync,
    ) -> Vec<ImportResult> {
        let (files, enhanced) = match self
            .enhanced_parser()
            .map(|p| (kb::expand_import_paths(paths, p.is_some()), p))
        {
            Ok(v) => v,
            Err(e) => {
                // Settings unreadable: import with the built-in parsers.
                let files = kb::expand_import_paths(paths, false);
                let mut out = self.import_builtin_all(&files, &progress);
                if let Some(first) = out.first_mut() {
                    first
                        .warnings
                        .push(format!("无法读取增强解析设置，已用普通模式：{e}"));
                }
                return out;
            }
        };
        let total = files.len();
        let mut out = Vec::with_capacity(total);
        for (i, path) in files.iter().enumerate() {
            let file_name = file_name_of(path);
            progress(i, total, &file_name);
            let online = enhanced.as_ref().filter(|_| wants_online(path));
            out.push(match online {
                Some(parser) => {
                    let report = |stage: &str, done: usize, pages: usize| {
                        let what = match stage {
                            "upload" => "上传中".to_string(),
                            "queued" => "排队中".to_string(),
                            "parsing" if pages > 0 => format!("解析中 {done}/{pages} 页"),
                            "parsing" => "解析中".to_string(),
                            "download" => "下载结果".to_string(),
                            other => other.to_string(),
                        };
                        progress(
                            i,
                            total,
                            &format!("{file_name}（{} {what}）", parser.kind().label()),
                        );
                    };
                    self.import_online(path, file_name.clone(), parser, &report)
                        .await
                }
                None => self.import_builtin(path, file_name),
            });
        }
        progress(total, total, "");
        out
    }

    fn import_builtin_all(
        &self,
        files: &[PathBuf],
        progress: &impl Fn(usize, usize, &str),
    ) -> Vec<ImportResult> {
        let mut out = Vec::with_capacity(files.len());
        for (i, path) in files.iter().enumerate() {
            let file_name = file_name_of(path);
            progress(i, files.len(), &file_name);
            out.push(self.import_builtin(path, file_name));
        }
        progress(files.len(), files.len(), "");
        out
    }

    fn import_builtin(&self, path: &Path, file_name: String) -> ImportResult {
        match self.with_kb(|kb| kb.import_file(path)) {
            Ok(r) => ImportResult::ok(file_name, r, kb::BUILTIN_PARSER),
            Err(e) => ImportResult::failed(file_name, e.to_string(), kb::BUILTIN_PARSER),
        }
    }

    async fn import_online(
        &self,
        path: &Path,
        file_name: String,
        parser: &EnhancedParser,
        progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
    ) -> ImportResult {
        let id = parser.kind().id();
        // Content this service already parsed is not sent again.
        match self.with_kb(|kb| kb.find_content(path)) {
            Ok(Some(doc)) if doc.parser == id => {
                let report = kb::ImportReport {
                    doc_id: doc.id,
                    chunks: doc.chunk_count,
                    unchanged: true,
                    warnings: Vec::new(),
                };
                return ImportResult::ok(file_name, report, id);
            }
            Err(e) => return ImportResult::failed(file_name, e.to_string(), id),
            _ => {}
        }
        let parsed = match std::fs::metadata(path) {
            Ok(m) if m.len() > parser.max_bytes() => {
                Err(EnhancedError::TooLarge(parser.max_bytes() / (1024 * 1024)).to_string())
            }
            Ok(_) => match std::fs::read(path) {
                Ok(bytes) => parser
                    .parse(&file_name, bytes, progress)
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => return ImportResult::failed(file_name, format!("无法读取文件：{e}"), id),
            },
            Err(e) => return ImportResult::failed(file_name, format!("无法读取文件：{e}"), id),
        };
        match parsed {
            Ok(markdown) => match self.with_kb(|kb| kb.import_parsed(path, &markdown, id)) {
                Ok(r) => ImportResult::ok(file_name, r, id),
                Err(e) => ImportResult::failed(file_name, e.to_string(), id),
            },
            Err(reason) if is_image(path) => {
                ImportResult::failed(file_name, format!("增强解析失败：{reason}"), id)
            }
            Err(reason) => {
                let mut result = self.import_builtin(path, file_name);
                match &mut result.error {
                    Some(error) => {
                        *error = format!("增强解析失败：{reason}；普通模式也无法读取：{error}")
                    }
                    None => result
                        .warnings
                        .insert(0, format!("增强解析失败，已用普通模式：{reason}")),
                }
                result
            }
        }
    }

    /// A document for the viewer: metadata, lines with heading levels and
    /// chunk spans.
    pub fn kb_document_view(&self, doc_id: i64) -> Result<DocumentView> {
        self.with_kb(|kb| kb.document_view(doc_id))
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

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn is_image(path: &Path) -> bool {
    kb::IMAGE_EXTENSIONS.contains(&extension(path).as_str())
}

/// PDFs and images go to the online parser; Word, TXT and Markdown are
/// read accurately by the built-in parsers.
fn wants_online(path: &Path) -> bool {
    extension(path) == "pdf" || is_image(path)
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
