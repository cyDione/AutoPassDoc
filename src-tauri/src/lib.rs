//! Tauri shell: exposes the docx engine and the application core to the UI
//! through IPC commands and serves document images through the `apd://`
//! protocol.

mod ai;
mod data;
mod documents;
mod knowledge;
mod proofread;

use std::fmt::Display;
use std::sync::Arc;

use app_core::Core;
use documents::OpenDocuments;
use tauri::Manager;
use tauri::http::{Response, StatusCode};

/// Command result; errors are shown to the user as they are.
pub(crate) type Res<T> = Result<T, String>;

pub(crate) fn err(e: impl Display) -> String {
    e.to_string()
}

/// Runs blocking work (file and database access) off the async runtime.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Res<T> + Send + 'static,
) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
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
    let image = open.doc.read().unwrap().image(rel_id);
    match image {
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
        .plugin(tauri_plugin_opener::init())
        .manage(documents)
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let core =
                Core::open(&dir).map_err(|e| format!("无法打开数据目录 {}：{e}", dir.display()))?;
            app.manage(Arc::new(core));
            Ok(())
        })
        .register_uri_scheme_protocol("apd", |ctx, request| {
            let state = ctx.app_handle().state::<Arc<OpenDocuments>>();
            serve_image(&state, request.uri().path())
        })
        .invoke_handler(tauri::generate_handler![
            documents::open_document,
            documents::get_blocks,
            documents::suggested_save_path,
            documents::save_document_as,
            documents::save_document,
            documents::initial_file,
            documents::close_document,
            documents::doc_state,
            documents::document_summary,
            documents::undo,
            documents::redo,
            documents::set_comment_done,
            documents::add_comment_reply,
            documents::document_authors,
            documents::assign_author,
            ai::fix_comment,
            ai::fix_batch,
            ai::apply_fix,
            ai::reject_fix,
            ai::list_reviewers,
            ai::create_reviewer,
            ai::update_reviewer,
            ai::delete_reviewer,
            ai::merge_reviewers,
            ai::reviewer_profile,
            ai::distill_profile,
            ai::reviewer_cases,
            ai::pre_review,
            ai::export_dataset,
            ai::get_settings,
            ai::save_settings,
            ai::list_providers,
            ai::save_provider,
            ai::delete_provider,
            ai::fetch_models,
            ai::provider_models,
            ai::set_model_profile,
            ai::test_role,
            knowledge::kb_documents,
            knowledge::kb_stats,
            knowledge::kb_import,
            knowledge::kb_remove,
            knowledge::kb_update_meta,
            knowledge::kb_search,
            knowledge::kb_embed,
            knowledge::kb_clear_embeddings,
            data::export_backup,
            data::inspect_backup,
            data::import_backup,
            data::app_info,
            data::check_update,
            data::download_update,
            data::cancel_update_download,
            data::install_update,
            proofread::proofread,
            proofread::cancel_proofread,
            proofread::apply_proof_issues,
            proofread::clear_proofread_cache,
            knowledge::web_search,
            knowledge::web_download_to_kb,
            knowledge::kb_count_import,
            knowledge::kb_document_view,
            knowledge::parser_infos,
            knowledge::set_parser_key,
            knowledge::clear_parser_key,
            knowledge::test_parser,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AutoPassDoc");
}

#[cfg(test)]
mod tests {
    use super::*;
    use documents::OpenDocument;
    use docx_engine::Document;
    use std::path::PathBuf;

    #[test]
    fn serves_images_from_open_documents() {
        let state = OpenDocuments::default();
        let doc = Document::from_bytes(docx_engine::testgen::generate(
            &docx_engine::testgen::Spec {
                target_chars: 2_000,
                comments: 3,
                seed: 1,
            },
        ))
        .unwrap();
        state
            .docs
            .write()
            .unwrap()
            .insert(7, Arc::new(OpenDocument::new(PathBuf::from("a.docx"), doc)));

        let ok = serve_image(&state, "/7/rIdImage1");
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()["Content-Type"], "image/png");
        assert!(ok.body().starts_with(b"\x89PNG"));

        for missing in ["/7/rIdNope", "/8/rIdImage1", "/garbage", ""] {
            assert_eq!(
                serve_image(&state, missing).status(),
                StatusCode::NOT_FOUND,
                "{missing}"
            );
        }
    }
}
