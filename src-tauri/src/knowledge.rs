//! Knowledge-base commands.

use std::path::PathBuf;
use std::sync::Arc;

use app_core::Core;
use app_core::knowledge::{HitView, ImportResult, StatsView};
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
            Ok(core.kb_import(&paths, |done, total, current| {
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
            }))
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
