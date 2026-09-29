//! Knowledge base: import, structure-aware chunking and hybrid search.
//!
//! A knowledge base is one folder: `kb.sqlite` holds the documents, their
//! parent and child chunks, an FTS5 keyword index over jieba tokens and the
//! embeddings; `files/` holds a copy of every imported file so citations
//! still open after the original moves.
//!
//! Embeddings are computed by the caller, which keeps this crate independent
//! of any model: fetch [`KnowledgeBase::pending_embeddings`], embed the texts
//! and hand the vectors back with [`KnowledgeBase::store_embeddings`].
//!
//! Char offsets ([`Hit::char_start`], [`Hit::char_end`]) count Unicode scalar
//! values in the document's full text ([`KnowledgeBase::full_text`]), which
//! is the document's lines joined with `\n`; for .docx, a paragraph is one
//! line and a table is one line per row, each value labelled with its column.

pub mod chunking;
mod db;
mod error;
pub mod metadata;
mod parse;
mod search;
mod text;
mod tokenize;
mod types;
mod vector;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use sha2::{Digest, Sha256};

pub use error::{Error, Result};
pub use types::*;

use vector::VectorIndex;

const DB_FILE: &str = "kb.sqlite";
const FILES_DIR: &str = "files";
const NO_TEXT_WARNING: &str = "文档中没有可检索的文字";

/// A knowledge base stored in one folder.
///
/// Not `Sync`; share it behind a `Mutex`.
pub struct KnowledgeBase {
    conn: Connection,
    root: PathBuf,
    /// Embeddings per model id, loaded on the first vector search and kept
    /// in sync by every write through this instance afterwards.
    vectors: RefCell<HashMap<i64, VectorIndex>>,
    /// `PRAGMA data_version` when the vectors were loaded; it changes when
    /// another connection (another instance on the same folder) commits.
    vectors_version: Cell<i64>,
}

struct ExistingDoc {
    id: i64,
    stored_name: String,
    meta_edited: bool,
    meta: DocMeta,
}

impl KnowledgeBase {
    /// Opens the knowledge base in `dir`, creating the folder, `kb.sqlite`
    /// and `files/` as needed, and upgrades the schema.
    pub fn open(dir: &Path) -> Result<Self> {
        let root = std::path::absolute(dir)?;
        std::fs::create_dir_all(root.join(FILES_DIR))?;
        let conn = db::open(&root.join(DB_FILE))?;
        tokenize::warm_up();
        Ok(Self {
            conn,
            root,
            vectors: RefCell::default(),
            vectors_version: Cell::new(0),
        })
    }

    /// The knowledge base folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Imports a .docx, .pdf, .txt or .md file: copies it into `files/`,
    /// extracts metadata, chunks and indexes it.
    ///
    /// A file whose content (SHA-256) is already imported is skipped and
    /// reported as `unchanged`. A changed file at an already imported path
    /// replaces the old version and keeps its id; metadata the user corrected
    /// is kept, and its embeddings have to be computed again.
    pub fn import_file(&mut self, path: &Path) -> Result<ImportReport> {
        let format = parse::format_of(path)?;
        let bytes = std::fs::read(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let sha256: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let known = self
            .conn
            .query_row(
                "SELECT id, (SELECT COUNT(*) FROM chunks WHERE doc_id = documents.id)
                 FROM documents WHERE sha256 = ?1 ORDER BY id LIMIT 1",
                [&sha256],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        if let Some((doc_id, chunks)) = known {
            return Ok(ImportReport {
                doc_id,
                chunks: chunks as usize,
                unchanged: true,
                warnings: Vec::new(),
            });
        }

        let parsed = parse::parse(format, &bytes)?;
        let mut chunked = chunking::chunk_lines(&parsed.lines);
        let mut warnings = parsed.warnings;
        if parsed.no_text {
            chunked.parents.clear();
        } else if chunked.parents.is_empty() {
            warnings.push(NO_TEXT_WARNING.to_string());
        }
        let meta_lines: Vec<&str> = parsed
            .lines
            .iter()
            .map(|l| l.display.as_deref().unwrap_or(&l.text))
            .collect();
        let mut meta = metadata::extract_metadata(&meta_lines);
        if parsed.title_hint.is_some() {
            meta.title = parsed.title_hint;
        }

        let original_path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let original = original_path.to_string_lossy().into_owned();
        let file_name = path
            .file_name()
            .map_or_else(|| "document".into(), |n| n.to_string_lossy().into_owned());
        let existing = self.existing_by_path(&original)?;
        if let Some(old) = existing.as_ref().filter(|old| old.meta_edited) {
            meta = old.meta.clone();
        }
        let char_count = chunked
            .full_text
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);

        let files_dir = self.files_dir();
        let tx = self.conn.transaction()?;
        let doc_id = match &existing {
            Some(old) => {
                delete_chunk_rows(&tx, old.id)?;
                old.id
            }
            None => {
                tx.execute(
                    "INSERT INTO documents (file_name, original_path, stored_name, format,
                     sha256, char_count, full_text, imported_at)
                     VALUES (?1, ?2, '', ?3, '', 0, '', 0)",
                    params![file_name, original, format],
                )?;
                tx.last_insert_rowid()
            }
        };
        let stored_name = format!("{doc_id}-{}", sanitize_file_name(&file_name));
        tx.execute(
            "UPDATE documents SET file_name = ?2, stored_name = ?3, format = ?4, sha256 = ?5,
             title = ?6, doc_number = ?7, issuer = ?8, date = ?9, char_count = ?10,
             full_text = ?11, warnings = ?12, imported_at = ?13 WHERE id = ?1",
            params![
                doc_id,
                file_name,
                stored_name,
                format,
                sha256,
                meta.title,
                meta.doc_number,
                meta.issuer,
                meta.date,
                char_count as i64,
                chunked.full_text,
                warnings.join("\n"),
                now,
            ],
        )?;
        let doc_tokens = doc_index_text(&meta, &file_name);
        let chunks = insert_chunks(&tx, doc_id, &chunked, &doc_tokens)?;
        let stored_path = files_dir.join(&stored_name);
        std::fs::create_dir_all(&files_dir)?;
        std::fs::write(&stored_path, &bytes)?;
        if let Err(e) = tx.commit() {
            if existing.is_none() {
                let _ = std::fs::remove_file(&stored_path);
            }
            return Err(e.into());
        }
        if let Some(old) = existing.filter(|old| old.stored_name != stored_name) {
            let _ = std::fs::remove_file(files_dir.join(old.stored_name));
        }
        self.forget_vectors_of(doc_id);
        Ok(ImportReport {
            doc_id,
            chunks,
            unchanged: false,
            warnings,
        })
    }

    fn existing_by_path(&self, original: &str) -> Result<Option<ExistingDoc>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, stored_name, meta_edited, title, doc_number, issuer, date
                 FROM documents WHERE original_path = ?1 ORDER BY id LIMIT 1",
                [original],
                |r| {
                    Ok(ExistingDoc {
                        id: r.get(0)?,
                        stored_name: r.get(1)?,
                        meta_edited: r.get(2)?,
                        meta: DocMeta {
                            title: r.get(3)?,
                            doc_number: r.get(4)?,
                            issuer: r.get(5)?,
                            date: r.get(6)?,
                        },
                    })
                },
            )
            .optional()?)
    }

    /// All documents, oldest first.
    pub fn documents(&self) -> Result<Vec<KbDocument>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{DOCUMENT_SQL} ORDER BY d.id"))?;
        let docs = stmt
            .query_map([], |r| self.document_from_row(r))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(docs)
    }

    /// One document by id.
    pub fn document(&self, id: i64) -> Result<Option<KbDocument>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{DOCUMENT_SQL} WHERE d.id = ?1"))?;
        Ok(stmt
            .query_row([id], |r| self.document_from_row(r))
            .optional()?)
    }

    /// The document's full text, which chunk offsets point into.
    pub fn full_text(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT full_text FROM documents WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    fn document_from_row(&self, r: &Row) -> rusqlite::Result<KbDocument> {
        let file_name: String = r.get(1)?;
        let meta = DocMeta {
            title: r.get(6)?,
            doc_number: r.get(7)?,
            issuer: r.get(8)?,
            date: r.get(9)?,
        };
        let warnings: String = r.get(11)?;
        Ok(KbDocument {
            id: r.get(0)?,
            title: display_title(meta.title.as_deref(), &file_name),
            stored_path: self.files_dir().join(r.get::<_, String>(3)?),
            original_path: PathBuf::from(r.get::<_, String>(2)?),
            format: r.get(4)?,
            sha256: r.get(5)?,
            char_count: r.get::<_, i64>(10)? as usize,
            warnings: warnings
                .lines()
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect(),
            imported_at: r.get(12)?,
            chunk_count: r.get::<_, i64>(13)? as usize,
            file_name,
            meta,
        })
    }

    /// Replaces a document's metadata with the user's corrections. Empty
    /// strings clear a field; 文号 brackets and dates are normalised.
    /// Re-importing the file later keeps these values.
    pub fn update_metadata(&mut self, id: i64, meta: &DocMeta) -> Result<()> {
        let clean = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let meta = DocMeta {
            title: clean(&meta.title),
            doc_number: clean(&meta.doc_number)
                .map(|n| metadata::normalize_doc_number(&n).unwrap_or(n)),
            issuer: clean(&meta.issuer),
            date: clean(&meta.date).map(|d| normalize_date(&d).unwrap_or(d)),
        };
        let file_name: String = self
            .conn
            .query_row("SELECT file_name FROM documents WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or(Error::DocumentNotFound(id))?;
        let doc_tokens = doc_index_text(&meta, &file_name);
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE documents SET title = ?2, doc_number = ?3, issuer = ?4, date = ?5,
             meta_edited = 1 WHERE id = ?1",
            params![id, meta.title, meta.doc_number, meta.issuer, meta.date],
        )?;
        // The FTS table stores no content, so rows are rewritten whole.
        {
            let mut chunks =
                tx.prepare("SELECT id, heading_path, text FROM chunks WHERE doc_id = ?1")?;
            let mut delete = tx.prepare("DELETE FROM chunks_fts WHERE rowid = ?1")?;
            let mut insert = tx.prepare(
                "INSERT INTO chunks_fts (rowid, body, head, doc) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let rows = chunks
                .query_map([id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (chunk_id, path, text) in rows {
                delete.execute([chunk_id])?;
                insert.execute(params![
                    chunk_id,
                    tokenize::index_text(&text),
                    tokenize::index_text(&db::split_path(&path).join(" ")),
                    doc_tokens
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes a document with its chunks, index entries, embeddings and stored file.
    pub fn remove_document(&mut self, id: i64) -> Result<()> {
        let stored_name: String = self
            .conn
            .query_row(
                "SELECT stored_name FROM documents WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::DocumentNotFound(id))?;
        // The file goes first: if it is locked (open in a viewer on Windows),
        // nothing is removed and the user can retry.
        match std::fs::remove_file(self.files_dir().join(stored_name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let tx = self.conn.transaction()?;
        delete_chunk_rows(&tx, id)?;
        tx.execute("DELETE FROM documents WHERE id = ?1", [id])?;
        tx.commit()?;
        self.forget_vectors_of(id);
        Ok(())
    }

    /// Up to `limit` chunks without an embedding from `model`, as
    /// `(chunk_id, text to embed)`. The text is the heading path followed by
    /// the chunk, e.g. `第三章 监督管理 > 第二十条\n第二十条　县级以上…`.
    pub fn pending_embeddings(&self, model: &str, limit: usize) -> Result<Vec<(i64, String)>> {
        let model_id = self.model(model)?.map_or(-1, |m| m.0);
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id, c.heading_path, c.text FROM chunks c
             WHERE NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.chunk_id = c.id AND e.model_id = ?1)
             ORDER BY c.id LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![model_id, limit as i64], |r| {
                let path = db::split_path(&r.get::<_, String>(1)?);
                let text: String = r.get(2)?;
                Ok((
                    r.get(0)?,
                    if path.is_empty() {
                        text
                    } else {
                        format!("{}\n{text}", path.join(" > "))
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Stores embeddings from `model`, replacing earlier ones for the same
    /// chunks. Every vector of a model must have the same dimension; chunks
    /// that no longer exist are skipped.
    pub fn store_embeddings(&mut self, model: &str, items: &[(i64, Vec<f32>)]) -> Result<()> {
        let Some(first) = items.first() else {
            return Ok(());
        };
        if first.1.is_empty() {
            return Err(Error::EmptyVector);
        }
        let tx = self.conn.transaction()?;
        let (model_id, dim) = match tx
            .query_row("SELECT id, dim FROM models WHERE name = ?1", [model], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as usize))
            })
            .optional()?
        {
            Some(m) => m,
            None => {
                tx.execute(
                    "INSERT INTO models (name, dim) VALUES (?1, ?2)",
                    params![model, first.1.len() as i64],
                )?;
                (tx.last_insert_rowid(), first.1.len())
            }
        };
        let mut stored = Vec::with_capacity(items.len());
        {
            let mut doc_of = tx.prepare_cached("SELECT doc_id FROM chunks WHERE id = ?1")?;
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO embeddings (chunk_id, model_id, vector) VALUES (?1, ?2, ?3)",
            )?;
            for (i, (chunk_id, vector)) in items.iter().enumerate() {
                if vector.len() != dim {
                    return Err(Error::DimensionMismatch {
                        model: model.to_string(),
                        expected: dim,
                        actual: vector.len(),
                    });
                }
                let Some(doc_id) = doc_of
                    .query_row([chunk_id], |r| r.get::<_, i64>(0))
                    .optional()?
                else {
                    continue;
                };
                insert.execute(params![chunk_id, model_id, db::vector_to_blob(vector)])?;
                stored.push((*chunk_id, doc_id, i));
            }
        }
        tx.commit()?;
        if let Some(index) = self.vectors.get_mut().get_mut(&model_id) {
            for (chunk_id, doc_id, i) in stored {
                index.upsert(chunk_id, doc_id, &items[i].1);
            }
        }
        Ok(())
    }

    /// Deletes all embeddings of `model`, e.g. after switching to a model
    /// with another dimension under the same name.
    pub fn clear_embeddings(&mut self, model: &str) -> Result<()> {
        if let Some((model_id, _)) = self.model(model)? {
            self.conn
                .execute("DELETE FROM models WHERE id = ?1", [model_id])?;
            self.vectors.get_mut().remove(&model_id);
        }
        Ok(())
    }

    /// `(chunks embedded with model, all chunks)`.
    pub fn embedding_progress(&self, model: &str) -> Result<(usize, usize)> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let done: i64 = match self.model(model)? {
            Some((model_id, _)) => self.conn.query_row(
                "SELECT COUNT(*) FROM embeddings WHERE model_id = ?1",
                [model_id],
                |r| r.get(0),
            )?,
            None => 0,
        };
        Ok((done as usize, total as usize))
    }

    /// One chunk as a [`Hit`] without ranks, e.g. to resolve a stored citation.
    pub fn chunk(&self, chunk_id: i64) -> Result<Option<Hit>> {
        self.hit(chunk_id, None, None, 0.0)
    }

    /// Counts of documents, chunks and embedded chunks per model.
    pub fn stats(&self) -> Result<KbStats> {
        let count = |sql: &str| -> Result<usize> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.name, COUNT(e.chunk_id) FROM models m
             LEFT JOIN embeddings e ON e.model_id = m.id GROUP BY m.id",
        )?;
        let embedded = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(KbStats {
            documents: count("SELECT COUNT(*) FROM documents")?,
            chunks: count("SELECT COUNT(*) FROM chunks")?,
            embedded,
        })
    }

    /// Writes a consistent copy of the database to `path` (`VACUUM INTO`),
    /// e.g. for a backup. An existing file at `path` is replaced.
    pub fn snapshot_to(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let target = path.to_str().ok_or_else(|| {
            Error::Io(std::io::Error::other(format!(
                "路径含有无法识别的字符：{}",
                path.display()
            )))
        })?;
        self.conn.execute("VACUUM INTO ?1", [target])?;
        Ok(())
    }

    /// Copies in the documents of the knowledge base in `other_dir` whose
    /// content (SHA-256) is not here yet: the stored original, metadata,
    /// chunks, keyword index rows and the embeddings of every model whose
    /// dimension matches the vectors stored here under the same name.
    /// `other_dir` is opened (and its schema upgraded) but otherwise left as is.
    pub fn merge_from(&mut self, other_dir: &Path) -> Result<MergeStats> {
        let other = KnowledgeBase::open(other_dir)?;
        let mut stats = MergeStats::default();

        let mut known: HashSet<String> = {
            let mut stmt = self.conn.prepare("SELECT sha256 FROM documents")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut docs = Vec::new();
        {
            let mut stmt = other
                .conn
                .prepare("SELECT id, sha256 FROM documents ORDER BY id")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, sha) in rows {
                if known.insert(sha) {
                    docs.push(id);
                } else {
                    stats.documents_skipped += 1;
                }
            }
        }
        if docs.is_empty() {
            return Ok(stats);
        }

        // Embedding models of the other side mapped to ours: (id here, dim).
        let mut models: HashMap<i64, (i64, usize)> = HashMap::new();
        {
            let mut stmt = other.conn.prepare("SELECT id, name, dim FROM models")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)? as usize,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (other_id, name, dim) in rows {
                match self.model(&name)? {
                    Some((id, d)) if d == dim => {
                        models.insert(other_id, (id, dim));
                    }
                    Some(_) => stats.skipped_models.push(name),
                    None => {
                        self.conn.execute(
                            "INSERT INTO models (name, dim) VALUES (?1, ?2)",
                            params![name, dim as i64],
                        )?;
                        models.insert(other_id, (self.conn.last_insert_rowid(), dim));
                    }
                }
            }
        }

        let files_dir = self.files_dir();
        std::fs::create_dir_all(&files_dir)?;
        let other_files = other.files_dir();
        for other_id in docs {
            let added = merge_document(&mut self.conn, &other.conn, other_id, &models)?;
            let source = other_files.join(&added.other_stored_name);
            match std::fs::copy(&source, files_dir.join(&added.stored_name)) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    stats.missing_files.push(added.file_name.clone());
                }
                Err(e) => {
                    // No document row without its file.
                    let tx = self.conn.transaction()?;
                    delete_chunk_rows(&tx, added.doc_id)?;
                    tx.execute("DELETE FROM documents WHERE id = ?1", [added.doc_id])?;
                    tx.commit()?;
                    self.vectors.get_mut().clear();
                    return Err(e.into());
                }
            }
            stats.documents_added += 1;
            stats.chunks_added += added.chunks;
            stats.embeddings_added += added.embeddings;
        }
        // Loaded again on the next vector search.
        self.vectors.get_mut().clear();
        Ok(stats)
    }

    fn files_dir(&self) -> PathBuf {
        self.root.join(FILES_DIR)
    }

    /// `(id, dim)` of an embedding model.
    fn model(&self, name: &str) -> Result<Option<(i64, usize)>> {
        Ok(self
            .conn
            .query_row("SELECT id, dim FROM models WHERE name = ?1", [name], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? as usize))
            })
            .optional()?)
    }

    fn forget_vectors_of(&mut self, doc_id: i64) {
        for index in self.vectors.get_mut().values_mut() {
            index.remove_doc(doc_id);
        }
    }
}

const DOCUMENT_SQL: &str = "SELECT d.id, d.file_name, d.original_path, d.stored_name, d.format,
    d.sha256, d.title, d.doc_number, d.issuer, d.date, d.char_count, d.warnings, d.imported_at,
    (SELECT COUNT(*) FROM chunks c WHERE c.doc_id = d.id)
    FROM documents d";

struct MergedDocument {
    doc_id: i64,
    file_name: String,
    stored_name: String,
    other_stored_name: String,
    chunks: usize,
    embeddings: usize,
}

/// Copies one document's rows from `other` in one transaction. Keyword
/// index rows are rebuilt, since the FTS table stores no content.
fn merge_document(
    conn: &mut Connection,
    other: &Connection,
    other_id: i64,
    models: &HashMap<i64, (i64, usize)>,
) -> Result<MergedDocument> {
    let (file_name, other_stored_name, meta) = other.query_row(
        "SELECT file_name, stored_name, title, doc_number, issuer, date FROM documents WHERE id = ?1",
        [other_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                DocMeta {
                    title: r.get(2)?,
                    doc_number: r.get(3)?,
                    issuer: r.get(4)?,
                    date: r.get(5)?,
                },
            ))
        },
    )?;

    // Every document column both sides know, so columns added later travel too.
    let ours = table_columns(conn, "documents")?;
    let columns: Vec<String> = table_columns(other, "documents")?
        .into_iter()
        .filter(|c| c != "id" && ours.contains(c))
        .collect();
    let values: Vec<rusqlite::types::Value> = other.query_row(
        &format!("SELECT {} FROM documents WHERE id = ?1", columns.join(", ")),
        [other_id],
        |r| (0..columns.len()).map(|i| r.get(i)).collect(),
    )?;

    let tx = conn.transaction()?;
    tx.execute(
        &format!(
            "INSERT INTO documents ({}) VALUES ({})",
            columns.join(", "),
            vec!["?"; columns.len()].join(", ")
        ),
        rusqlite::params_from_iter(values),
    )?;
    let doc_id = tx.last_insert_rowid();
    let stored_name = format!("{doc_id}-{}", sanitize_file_name(&file_name));
    tx.execute(
        "UPDATE documents SET stored_name = ?2 WHERE id = ?1",
        params![doc_id, stored_name],
    )?;

    let mut parent_ids: HashMap<i64, i64> = HashMap::new();
    {
        let mut select = other.prepare(
            "SELECT id, char_start, char_end, text FROM parents WHERE doc_id = ?1 ORDER BY id",
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO parents (doc_id, char_start, char_end, text) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut rows = select.query([other_id])?;
        while let Some(r) = rows.next()? {
            insert.execute(params![
                doc_id,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?
            ])?;
            parent_ids.insert(r.get(0)?, tx.last_insert_rowid());
        }
    }

    let doc_tokens = doc_index_text(&meta, &file_name);
    let mut chunk_ids: HashMap<i64, i64> = HashMap::new();
    {
        let mut select = other.prepare(
            "SELECT id, parent_id, heading_path, char_start, char_end, text
             FROM chunks WHERE doc_id = ?1 ORDER BY id",
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO chunks (doc_id, parent_id, heading_path, char_start, char_end, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut fts = tx.prepare_cached(
            "INSERT INTO chunks_fts (rowid, body, head, doc) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut last_head: Option<(String, String)> = None;
        let mut rows = select.query([other_id])?;
        while let Some(r) = rows.next()? {
            let Some(&parent_id) = parent_ids.get(&r.get::<_, i64>(1)?) else {
                continue;
            };
            let path: String = r.get(2)?;
            let text: String = r.get(5)?;
            insert.execute(params![
                doc_id,
                parent_id,
                path,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                text
            ])?;
            let chunk_id = tx.last_insert_rowid();
            chunk_ids.insert(r.get(0)?, chunk_id);
            if last_head.as_ref().is_none_or(|(p, _)| *p != path) {
                let head = tokenize::index_text(&db::split_path(&path).join(" "));
                last_head = Some((path, head));
            }
            let head = &last_head.as_ref().expect("set above").1;
            fts.execute(params![
                chunk_id,
                tokenize::index_text(&text),
                head,
                doc_tokens
            ])?;
        }
    }

    let mut embeddings = 0;
    {
        let mut select = other.prepare(
            "SELECT e.chunk_id, e.model_id, e.vector FROM embeddings e
             JOIN chunks c ON c.id = e.chunk_id WHERE c.doc_id = ?1",
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT OR REPLACE INTO embeddings (chunk_id, model_id, vector) VALUES (?1, ?2, ?3)",
        )?;
        let mut rows = select.query([other_id])?;
        while let Some(r) = rows.next()? {
            let (Some(&chunk_id), Some(&(model_id, dim))) = (
                chunk_ids.get(&r.get::<_, i64>(0)?),
                models.get(&r.get::<_, i64>(1)?),
            ) else {
                continue;
            };
            let vector: Vec<u8> = r.get(2)?;
            if vector.len() != dim * 4 {
                continue;
            }
            insert.execute(params![chunk_id, model_id, vector])?;
            embeddings += 1;
        }
    }
    tx.commit()?;
    Ok(MergedDocument {
        doc_id,
        file_name,
        stored_name,
        other_stored_name,
        chunks: chunk_ids.len(),
        embeddings,
    })
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(columns)
}

fn delete_chunk_rows(tx: &Transaction, doc_id: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE doc_id = ?1)",
        [doc_id],
    )?;
    // Embeddings go with their chunks (ON DELETE CASCADE).
    tx.execute("DELETE FROM chunks WHERE doc_id = ?1", [doc_id])?;
    tx.execute("DELETE FROM parents WHERE doc_id = ?1", [doc_id])?;
    Ok(())
}

fn insert_chunks(
    tx: &Transaction,
    doc_id: i64,
    chunked: &chunking::Chunked,
    doc_tokens: &str,
) -> Result<usize> {
    let mut parent = tx.prepare_cached(
        "INSERT INTO parents (doc_id, char_start, char_end, text) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut chunk = tx.prepare_cached(
        "INSERT INTO chunks (doc_id, parent_id, heading_path, char_start, char_end, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut fts = tx.prepare_cached(
        "INSERT INTO chunks_fts (rowid, body, head, doc) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut count = 0;
    let mut last_path: Option<(&[String], String)> = None;
    for p in &chunked.parents {
        parent.execute(params![
            doc_id,
            p.char_start as i64,
            p.char_end as i64,
            p.text
        ])?;
        let parent_id = tx.last_insert_rowid();
        for c in &p.children {
            chunk.execute(params![
                doc_id,
                parent_id,
                db::join_path(&c.heading_path),
                c.char_start as i64,
                c.char_end as i64,
                c.text
            ])?;
            let chunk_id = tx.last_insert_rowid();
            if last_path
                .as_ref()
                .is_none_or(|(p, _)| *p != c.heading_path.as_slice())
            {
                last_path = Some((
                    &c.heading_path,
                    tokenize::index_text(&c.heading_path.join(" ")),
                ));
            }
            let head = &last_path.as_ref().unwrap().1;
            fts.execute(params![
                chunk_id,
                tokenize::index_text(&c.text),
                head,
                doc_tokens
            ])?;
            count += 1;
        }
    }
    Ok(count)
}

/// Tokens of the document-level fields shared by all of its chunks.
fn doc_index_text(meta: &DocMeta, file_name: &str) -> String {
    let title = display_title(meta.title.as_deref(), file_name);
    let fields = [
        Some(title.as_str()),
        meta.issuer.as_deref(),
        meta.doc_number.as_deref(),
    ];
    tokenize::index_text(&fields.into_iter().flatten().collect::<Vec<_>>().join(" "))
}

fn display_title(title: Option<&str>, file_name: &str) -> String {
    title.map(str::to_string).unwrap_or_else(|| {
        Path::new(file_name).file_stem().map_or_else(
            || file_name.to_string(),
            |s| s.to_string_lossy().into_owned(),
        )
    })
}

/// A file name safe on Windows and macOS, at most 100 chars.
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.chars().count() <= 10 => (s, Some(e)),
        _ => (cleaned, None),
    };
    let stem: String = stem.chars().take(90).collect();
    let stem = if stem.is_empty() { "document" } else { &stem };
    match ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem.to_string(),
    }
}

/// `2024-3-5`, `2024/03/05`, `2024年3月5日` → `2024-03-05`.
fn normalize_date(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split(['-', '/', '.']).map(str::trim).collect();
    if let [y, m, d] = parts.as_slice()
        && let (Ok(y), Ok(m), Ok(d)) = (y.parse::<u32>(), m.parse::<u32>(), d.parse::<u32>())
        && (1..=12).contains(&m)
        && (1..=31).contains(&d)
    {
        return Some(format!("{y:04}-{m:02}-{d:02}"));
    }
    metadata::parse_date(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("a:b/c?.docx"), "a_b_c_.docx");
        assert_eq!(sanitize_file_name("关于印发.docx"), "关于印发.docx");
        assert_eq!(sanitize_file_name("..."), "document");
    }

    #[test]
    fn normalizes_dates() {
        assert_eq!(normalize_date("2024-3-5").as_deref(), Some("2024-03-05"));
        assert_eq!(normalize_date("2024/03/05").as_deref(), Some("2024-03-05"));
        assert_eq!(
            normalize_date("2024年3月5日").as_deref(),
            Some("2024-03-05")
        );
        assert_eq!(normalize_date("去年"), None);
    }
}
