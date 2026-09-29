//! Open documents and the commands that read, edit and save them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use app_core::Core;
use app_core::reviewers::{self, AuthorView};
use docx_engine::Document;
use docx_engine::view::{BlockView, DocumentSummary};
use serde::Serialize;
use tauri::State;

use crate::{Res, blocking, err};

#[derive(Default)]
pub struct OpenDocuments {
    next_id: AtomicU64,
    pub(crate) docs: RwLock<HashMap<u64, Arc<OpenDocument>>>,
}

pub struct OpenDocument {
    pub path: PathBuf,
    /// Identifies the document for per-document author mappings.
    pub key: String,
    /// Where the document was last saved in this session.
    pub saved_path: Mutex<Option<PathBuf>>,
    pub doc: RwLock<Document>,
}

impl OpenDocument {
    pub fn new(path: PathBuf, doc: Document) -> Self {
        let absolute = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let mut key = absolute.to_string_lossy().into_owned();
        if cfg!(windows) {
            key = key.to_lowercase();
        }
        Self {
            path,
            key,
            saved_path: Mutex::new(None),
            doc: RwLock::new(doc),
        }
    }

    pub fn file_name(&self) -> String {
        file_name(&self.path)
    }

    pub fn state(&self, doc: &Document) -> DocState {
        DocState {
            dirty: doc.is_dirty(),
            undo_label: doc.undo_label().map(str::to_string),
            redo_label: doc.redo_label().map(str::to_string),
            saved_path: self
                .saved_path
                .lock()
                .unwrap()
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }

    pub fn outcome(&self, doc: &Document) -> EditOutcome {
        EditOutcome {
            state: self.state(doc),
            summary: doc.summary(),
        }
    }
}

impl OpenDocuments {
    pub fn get(&self, id: u64) -> Res<Arc<OpenDocument>> {
        self.docs
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| "文档已关闭".to_string())
    }
}

type Docs<'a> = State<'a, Arc<OpenDocuments>>;
type CoreState<'a> = State<'a, Arc<Core>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocState {
    dirty: bool,
    undo_label: Option<String>,
    redo_label: Option<String>,
    saved_path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    state: DocState,
    summary: DocumentSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    doc_id: u64,
    path: String,
    file_name: String,
    summary: DocumentSummary,
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn open_document(path: String, state: Docs<'_>) -> Res<Opened> {
    let state = state.inner().clone();
    blocking(move || {
        let path = PathBuf::from(path);
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
        {
            return Err(
                "目前只支持 .docx 文件。.doc 或 .wps 文件请先在 Word/WPS 中另存为 .docx。"
                    .to_string(),
            );
        }
        let doc = Document::open(&path).map_err(err)?;
        let summary = doc.summary();
        let doc_id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let open = OpenDocument::new(path.clone(), doc);
        let file_name = open.file_name();
        state.docs.write().unwrap().insert(doc_id, Arc::new(open));
        Ok(Opened {
            doc_id,
            path: path.to_string_lossy().into_owned(),
            file_name,
            summary,
        })
    })
    .await
}

#[tauri::command]
pub fn get_blocks(doc_id: u64, start: usize, end: usize, state: Docs<'_>) -> Res<Vec<BlockView>> {
    Ok(state
        .get(doc_id)?
        .doc
        .read()
        .unwrap()
        .blocks_view(start, end))
}

/// Default "save as" path: next to the original, with a suffix, so the
/// reviewer's original file is never overwritten by accident.
#[tauri::command]
pub fn suggested_save_path(doc_id: u64, state: Docs<'_>) -> Res<String> {
    let open = state.get(doc_id)?;
    if let Some(p) = open.saved_path.lock().unwrap().as_ref() {
        return Ok(p.to_string_lossy().into_owned());
    }
    let stem = open
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("文档");
    Ok(open
        .path
        .with_file_name(format!("{stem}_AutoPassDoc.docx"))
        .to_string_lossy()
        .into_owned())
}

#[tauri::command]
pub async fn save_document_as(doc_id: u64, path: String, state: Docs<'_>) -> Res<DocState> {
    let open = state.get(doc_id)?;
    blocking(move || {
        let path = PathBuf::from(path);
        let mut doc = open.doc.write().unwrap();
        doc.save(&path).map_err(err)?;
        *open.saved_path.lock().unwrap() = Some(path);
        Ok(open.state(&doc))
    })
    .await
}

/// Saves to the path of the last "save as".
#[tauri::command]
pub async fn save_document(doc_id: u64, state: Docs<'_>) -> Res<DocState> {
    let open = state.get(doc_id)?;
    blocking(move || {
        let path = open
            .saved_path
            .lock()
            .unwrap()
            .clone()
            .ok_or("这份文档还没有另存过，请先选择保存位置")?;
        let mut doc = open.doc.write().unwrap();
        doc.save(&path).map_err(err)?;
        Ok(open.state(&doc))
    })
    .await
}

/// A .docx passed on the command line, e.g. when the app is opened through
/// "Open with" in Explorer or Finder.
#[tauri::command]
pub fn initial_file() -> Option<String> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
        })
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn close_document(doc_id: u64, state: Docs<'_>) {
    state.docs.write().unwrap().remove(&doc_id);
}

#[tauri::command]
pub fn doc_state(doc_id: u64, state: Docs<'_>) -> Res<DocState> {
    let open = state.get(doc_id)?;
    let doc = open.doc.read().unwrap();
    Ok(open.state(&doc))
}

#[tauri::command]
pub async fn document_summary(doc_id: u64, state: Docs<'_>) -> Res<DocumentSummary> {
    let open = state.get(doc_id)?;
    blocking(move || Ok(open.doc.read().unwrap().summary())).await
}

/// Runs an edit on the document off the UI thread and reports the new state.
async fn edit(
    open: Arc<OpenDocument>,
    f: impl FnOnce(&mut Document) -> Res<()> + Send + 'static,
) -> Res<EditOutcome> {
    blocking(move || {
        let mut doc = open.doc.write().unwrap();
        f(&mut doc)?;
        Ok(open.outcome(&doc))
    })
    .await
}

#[tauri::command]
pub async fn undo(doc_id: u64, state: Docs<'_>) -> Res<EditOutcome> {
    edit(state.get(doc_id)?, |doc| doc.undo().map(drop).map_err(err)).await
}

#[tauri::command]
pub async fn redo(doc_id: u64, state: Docs<'_>) -> Res<EditOutcome> {
    edit(state.get(doc_id)?, |doc| doc.redo().map(drop).map_err(err)).await
}

#[tauri::command]
pub async fn set_comment_done(
    doc_id: u64,
    comment_id: String,
    done: bool,
    state: Docs<'_>,
) -> Res<EditOutcome> {
    edit(state.get(doc_id)?, move |doc| {
        doc.set_comments_done(&[&comment_id], done).map_err(err)
    })
    .await
}

/// Replies to a comment thread as the revision author from the settings.
#[tauri::command]
pub async fn add_comment_reply(
    doc_id: u64,
    comment_id: String,
    text: String,
    state: Docs<'_>,
    core: CoreState<'_>,
) -> Res<EditOutcome> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("回复内容不能为空".into());
    }
    let author = core.settings().map_err(err)?.fix.author;
    edit(state.get(doc_id)?, move |doc| {
        doc.add_reply(&comment_id, &author, None, &text)
            .map(|_| ())
            .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn document_authors(
    doc_id: u64,
    state: Docs<'_>,
    core: CoreState<'_>,
) -> Res<Vec<AuthorView>> {
    let open = state.get(doc_id)?;
    let core = core.inner().clone();
    blocking(move || {
        let doc = open.doc.read().unwrap();
        reviewers::authors(&core.store(), &doc, &open.key).map_err(err)
    })
    .await
}

#[derive(Serialize)]
pub struct Assigned {
    authors: Vec<AuthorView>,
    outcome: Option<EditOutcome>,
}

/// Maps a comment signature to a reviewer (an existing one, a new one, or
/// none). `write_back` also renames the comments' author in the document.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn assign_author(
    doc_id: u64,
    author: String,
    initials: String,
    reviewer_id: Option<i64>,
    new_reviewer: Option<String>,
    write_back: bool,
    state: Docs<'_>,
    core: CoreState<'_>,
) -> Res<Assigned> {
    let open = state.get(doc_id)?;
    let core = core.inner().clone();
    blocking(move || {
        let reviewer = {
            let store = core.store();
            let reviewer = match (new_reviewer.map(|n| n.trim().to_string()), reviewer_id) {
                (Some(name), _) if !name.is_empty() => {
                    Some(match store.reviewer_by_name(&name).map_err(err)? {
                        Some(r) => r,
                        None => store.create_reviewer(&name, "").map_err(err)?,
                    })
                }
                (_, Some(id)) => Some(store.reviewer(id).map_err(err)?.ok_or("审稿人不存在")?),
                _ => None,
            };
            reviewers::assign(
                &store,
                &author,
                &initials,
                &open.key,
                reviewer.as_ref().map(|r| r.id),
            )
            .map_err(err)?;
            reviewer
        };
        let mut doc = open.doc.write().unwrap();
        let outcome = match reviewer {
            Some(r) if write_back && r.name != author => {
                let ids: Vec<String> = doc
                    .comments
                    .iter()
                    .filter(|c| {
                        c.author == author && c.initials.as_deref().unwrap_or("") == initials
                    })
                    .map(|c| c.id.clone())
                    .collect();
                let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                doc.set_comment_authors(&ids, &r.name, None).map_err(err)?;
                Some(open.outcome(&doc))
            }
            _ => None,
        };
        let authors = reviewers::authors(&core.store(), &doc, &open.key).map_err(err)?;
        Ok(Assigned { authors, outcome })
    })
    .await
}
