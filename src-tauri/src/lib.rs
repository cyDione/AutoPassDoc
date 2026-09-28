//! Tauri shell: exposes the docx engine to the UI through IPC commands and
//! serves document images through the `apd://` protocol.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use docx_engine::Document;
use docx_engine::view::{BlockView, DocumentSummary};
use serde::Serialize;
use tauri::http::{Response, StatusCode};
use tauri::{Manager, State};

#[derive(Default)]
struct OpenDocuments {
    next_id: AtomicU64,
    docs: RwLock<HashMap<u64, Arc<OpenDocument>>>,
}

struct OpenDocument {
    path: PathBuf,
    doc: Document,
}

impl OpenDocuments {
    fn get(&self, id: u64) -> Result<Arc<OpenDocument>, String> {
        self.docs
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| "文档已关闭".to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Opened {
    doc_id: u64,
    path: String,
    file_name: String,
    summary: DocumentSummary,
}

#[tauri::command]
async fn open_document(
    path: String,
    state: State<'_, Arc<OpenDocuments>>,
) -> Result<Opened, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
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
        let doc = Document::open(&path).map_err(|e| e.to_string())?;
        let summary = doc.summary();
        let doc_id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let file_name = file_name(&path);
        state.docs.write().unwrap().insert(
            doc_id,
            Arc::new(OpenDocument {
                path: path.clone(),
                doc,
            }),
        );
        Ok(Opened {
            doc_id,
            path: path.to_string_lossy().into_owned(),
            file_name,
            summary,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_blocks(
    doc_id: u64,
    start: usize,
    end: usize,
    state: State<'_, Arc<OpenDocuments>>,
) -> Result<Vec<BlockView>, String> {
    Ok(state.get(doc_id)?.doc.blocks_view(start, end))
}

/// Default "save as" path: next to the original, with a suffix, so the
/// reviewer's original file is never overwritten by accident.
#[tauri::command]
fn suggested_save_path(
    doc_id: u64,
    state: State<'_, Arc<OpenDocuments>>,
) -> Result<String, String> {
    let open = state.get(doc_id)?;
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
async fn save_document_as(
    doc_id: u64,
    path: String,
    state: State<'_, Arc<OpenDocuments>>,
) -> Result<(), String> {
    let open = state.get(doc_id)?;
    tauri::async_runtime::spawn_blocking(move || open.doc.save(&path).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// A .docx passed on the command line, e.g. when the app is opened through
/// "Open with" in Explorer or Finder.
#[tauri::command]
fn initial_file() -> Option<String> {
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
fn close_document(doc_id: u64, state: State<'_, Arc<OpenDocuments>>) {
    state.docs.write().unwrap().remove(&doc_id);
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `apd://localhost/<docId>/<relId>` → image bytes from the document package.
fn serve_image(state: &OpenDocuments, uri_path: &str) -> Response<Vec<u8>> {
    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };
    let decoded = percent_encoding::percent_decode_str(uri_path.trim_start_matches('/'))
        .decode_utf8_lossy()
        .into_owned();
    let Some((doc_id, rel_id)) = decoded.split_once('/') else {
        return not_found();
    };
    let Some(open) = doc_id.parse().ok().and_then(|id| state.get(id).ok()) else {
        return not_found();
    };
    match open.doc.image(rel_id) {
        Ok(Some((bytes, mime))) => Response::builder()
            .header("Content-Type", mime)
            .header("Cache-Control", "max-age=31536000, immutable")
            .body(bytes)
            .unwrap(),
        _ => not_found(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let documents = Arc::new(OpenDocuments::default());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(documents)
        .register_uri_scheme_protocol("apd", |ctx, request| {
            let state = ctx.app_handle().state::<Arc<OpenDocuments>>();
            serve_image(&state, request.uri().path())
        })
        .invoke_handler(tauri::generate_handler![
            open_document,
            get_blocks,
            suggested_save_path,
            save_document_as,
            initial_file,
            close_document
        ])
        .run(tauri::generate_context!())
        .expect("error while running AutoPassDoc");
}
