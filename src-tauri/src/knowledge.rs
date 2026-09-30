//! Knowledge-base commands.

use std::path::PathBuf;
use std::sync::Arc;

use app_core::Core;
use app_core::enhanced::{ParserInfo, ParserTest};
use app_core::knowledge::{HitView, ImportResult, StatsView};
use app_core::settings::ParserKind;
use kb::{DocMeta, KbDocument};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{Res, blocking, err};

type CoreState<'a> = State<'a, Arc<Core>>;

/// Fused results shown on the knowledge-base page.
const SEARCH_LIMIT: usize = 20;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KbProgress {
    stage: &'static str,
    done: usize,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn emit(app: &AppHandle, p: KbProgress) {
    let _ = app.emit("kb-progress", p);
}

#[tauri::command]
pub async fn kb_documents(core: CoreState<'_>) -> Res<Vec<KbDocument>> {
    let core = core.inner().clone();
    blocking(move || core.kb_documents().map_err(err)).await
}

#[tauri::command]
pub async fn kb_stats(core: CoreState<'_>) -> Res<StatsView> {
    let core = core.inner().clone();
    blocking(move || core.kb_stats().map_err(err)).await
}

/// Starts embedding in the background when an embedding model is set.
fn embed_later(app: AppHandle, core: Arc<Core>) {
    tauri::async_runtime::spawn(async move {
        let result = core
            .kb_embed(|done, total| {
                emit(
                    &app,
                    KbProgress {
                        stage: "embed",
                        done,
                        total,
                        current: None,
                        message: None,
                    },
                )
            })
            .await;
        if let Err(e) = result {
            emit(
                &app,
                KbProgress {
                    stage: "embed",
                    done: 0,
                    total: 0,
                    current: None,
                    message: Some(e.to_string()),
                },
            );
        }
    });
}

#[tauri::command]
pub async fn kb_import(
    paths: Vec<String>,
    app: AppHandle,
    core: CoreState<'_>,
) -> Res<Vec<ImportResult>> {
    let core = core.inner().clone();
    let reports = blocking({
        let (app, core) = (app.clone(), core.clone());
        move || {
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            // Parsing and indexing block; online parsing awaits the network.
            Ok(tauri::async_runtime::block_on(core.kb_import(
                &paths,
                |done, total, current| {
                    emit(
                        &app,
                        KbProgress {
                            stage: "import",
                            done,
                            total,
                            current: (!current.is_empty()).then(|| current.to_string()),
                            message: None,
                        },
                    )
                },
            )))
        }
    })
    .await?;
    if reports.iter().any(|r| r.doc_id.is_some() && !r.unchanged)
        && core
            .target(app_core::RoleName::Embedding)
            .ok()
            .flatten()
            .is_some()
    {
        embed_later(app, core);
    }
    Ok(reports)
}

/// Looks something up on the web (the chat model's search, else the search
/// engines with AI-written terms and AI screening, whitelisted sites first).
#[tauri::command]
pub async fn web_search(
    request: app_core::web::LookupRequest,
    core: CoreState<'_>,
) -> Res<app_core::web::SearchOutcome> {
    core.web_lookup(&request).await.map_err(err)
}

/// Downloads a file from the search results into the knowledge base.
#[tauri::command]
pub async fn web_download_to_kb(
    url: String,
    app: AppHandle,
    core: CoreState<'_>,
) -> Res<ImportResult> {
    let core = core.inner().clone();
    let report = core.web_download_to_kb(&url).await.map_err(err)?;
    if report.doc_id.is_some()
        && !report.unchanged
        && core
            .target(app_core::RoleName::Embedding)
            .ok()
            .flatten()
            .is_some()
    {
        embed_later(app, core);
    }
    Ok(report)
}

#[tauri::command]
pub async fn kb_remove(doc_id: i64, core: CoreState<'_>) -> Res<()> {
    let core = core.inner().clone();
    blocking(move || core.kb_remove(doc_id).map_err(err)).await
}

#[tauri::command]
pub async fn kb_update_meta(doc_id: i64, meta: DocMeta, core: CoreState<'_>) -> Res<KbDocument> {
    let core = core.inner().clone();
    blocking(move || core.kb_update_meta(doc_id, &meta).map_err(err)).await
}

#[tauri::command]
pub async fn kb_search(text: String, core: CoreState<'_>) -> Res<Vec<HitView>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(core.kb_search(&text, SEARCH_LIMIT).await.map_err(err)?.hits)
}

#[tauri::command]
pub async fn kb_embed(app: AppHandle, core: CoreState<'_>) -> Res<()> {
    let core = core.inner().clone();
    core.require(app_core::RoleName::Embedding).map_err(err)?;
    embed_later(app, core);
    Ok(())
}

#[tauri::command]
pub async fn kb_clear_embeddings(core: CoreState<'_>) -> Res<()> {
    let core = core.inner().clone();
    blocking(move || core.kb_clear_embeddings().map_err(err)).await
}

/// How many files an import of these paths would read (folders expanded).
#[tauri::command]
pub async fn kb_count_import(paths: Vec<String>, core: CoreState<'_>) -> Res<usize> {
    let core = core.inner().clone();
    blocking(move || {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        core.kb_count_import(&paths).map_err(err)
    })
    .await
}

/// A document's text, headings and chunk boundaries for the viewer.
#[tauri::command]
pub async fn kb_document_view(doc_id: i64, core: CoreState<'_>) -> Res<kb::DocumentView> {
    let core = core.inner().clone();
    blocking(move || core.kb_document_view(doc_id).map_err(err)).await
}

#[tauri::command]
pub async fn parser_infos(core: CoreState<'_>) -> Res<Vec<ParserInfo>> {
    let core = core.inner().clone();
    blocking(move || core.parser_infos().map_err(err)).await
}

#[tauri::command]
pub async fn set_parser_key(kind: ParserKind, key: String, core: CoreState<'_>) -> Res<ParserInfo> {
    let core = core.inner().clone();
    blocking(move || core.set_parser_key(kind, &key).map_err(err)).await
}

#[tauri::command]
pub async fn clear_parser_key(kind: ParserKind, core: CoreState<'_>) -> Res<()> {
    let core = core.inner().clone();
    blocking(move || core.clear_parser_key(kind).map_err(err)).await
}

/// Checks a key; tests `key` when given (before saving), else the saved one.
#[tauri::command]
pub async fn test_parser(
    kind: ParserKind,
    key: Option<String>,
    core: CoreState<'_>,
) -> Res<ParserTest> {
    core.test_parser(kind, key.as_deref().filter(|k| !k.trim().is_empty()))
        .await
        .map_err(err)
}
