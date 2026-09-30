//! Backups, the update check and what the About page shows.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use app_core::Core;
use app_core::backup::{BackupManifest, ExportSummary, ImportMode, ImportSummary};
use app_core::update::{self, ReleaseAsset, UpdateInfo};
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

/// Set to stop a running [`download_update`].
static CANCEL_DOWNLOAD: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgress {
    downloaded: u64,
    total: u64,
}

/// Downloads the release's installer for this machine into the temp
/// directory, emitting `update-progress`; returns where it landed.
#[tauri::command]
pub async fn download_update(installer: ReleaseAsset, app: AppHandle) -> Res<String> {
    CANCEL_DOWNLOAD.store(false, Ordering::Relaxed);
    let dir = std::env::temp_dir().join("AutoPassDoc-update");
    let last = AtomicU64::new(0);
    let path = update::download_installer(
        &installer,
        &dir,
        |downloaded, total| {
            // About every 256 KB, plus the end, so the UI isn't flooded.
            if downloaded == 0
                || downloaded >= total
                || downloaded - last.load(Ordering::Relaxed) >= 256 * 1024
            {
                last.store(downloaded, Ordering::Relaxed);
                let _ = app.emit("update-progress", UpdateProgress { downloaded, total });
            }
        },
        &CANCEL_DOWNLOAD,
    )
    .await
    .map_err(err)?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn cancel_update_download() {
    CANCEL_DOWNLOAD.store(true, Ordering::Relaxed);
}

/// Starts the downloaded installer and quits so it can replace the app.
#[tauri::command]
pub fn install_update(path: String, app: AppHandle) -> Res<()> {
    update::launch_installer(&PathBuf::from(path), std::process::id()).map_err(err)?;
    app.exit(0);
    Ok(())
}
