//! Backups, the update check and what the About page shows.

use std::path::PathBuf;
use std::sync::Arc;

use app_core::Core;
use app_core::backup::{BackupManifest, ExportSummary, ImportMode, ImportSummary};
use app_core::update::{self, UpdateInfo};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{Res, blocking, err};

type CoreState<'a> = State<'a, Arc<Core>>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupProgress {
    done: usize,
    total: usize,
    step: String,
}

fn progress(app: &AppHandle) -> impl Fn(usize, usize, &str) + use<> {
    let app = app.clone();
    move |done, total, step| {
        let _ = app.emit(
            "backup-progress",
            BackupProgress {
                done,
                total,
                step: step.to_string(),
            },
        );
    }
}

#[tauri::command]
pub async fn export_backup(
    path: String,
    app: AppHandle,
    core: CoreState<'_>,
) -> Res<ExportSummary> {
    let core = core.inner().clone();
    let report = progress(&app);
    blocking(move || {
        core.export_backup(&PathBuf::from(path), report)
            .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn inspect_backup(path: String, core: CoreState<'_>) -> Res<BackupManifest> {
    let core = core.inner().clone();
    blocking(move || core.inspect_backup(&PathBuf::from(path)).map_err(err)).await
}

#[tauri::command]
pub async fn import_backup(
    path: String,
    mode: ImportMode,
    app: AppHandle,
    core: CoreState<'_>,
) -> Res<ImportSummary> {
    let core = core.inner().clone();
    let report = progress(&app);
    blocking(move || {
        core.import_backup(&PathBuf::from(path), mode, report)
            .map_err(err)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    data_dir: String,
}

#[tauri::command]
pub fn app_info(app: AppHandle, core: CoreState<'_>) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        data_dir: core.data_dir().to_string_lossy().into_owned(),
    }
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Res<UpdateInfo> {
    let current = app.package_info().version.to_string();
    update::check_update(&current, update::RELEASES_URL)
        .await
        .map_err(err)
}
