//! AI fixes, reviewers, pre-review, settings and model providers.

use std::path::PathBuf;
use std::sync::Arc;

use app_core::fix::context::{self, FixRequest};
use app_core::fix::{FixJob, Proposal, Stage};
use app_core::prereview::PreReviewItem;
use app_core::profiles::{self, ProfileView};
use app_core::providers::{self, ModelView, ProbeResult, ProfileOverride, ProviderView};
use app_core::settings::Settings;
use app_core::store::{Case, ProviderRecord, Reviewer};
use app_core::{Core, RoleName};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Semaphore;

use crate::documents::{EditOutcome, OpenDocument, OpenDocuments};
use crate::{Res, blocking, err};

type Docs<'a> = State<'a, Arc<OpenDocuments>>;
type CoreState<'a> = State<'a, Arc<Core>>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FixProgress {
    doc_id: u64,
    comment_id: String,
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal: Option<Proposal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn stage_text(stage: Stage) -> (&'static str, &'static str) {
    match stage {
        Stage::Context => ("context", "正在读取批注和上下文"),
        Stage::Retrieve => ("retrieve", "正在检索知识库和审稿人习惯"),
        Stage::Web => ("web", "正在联网查找公开资料"),
        Stage::Generate => ("generate", "正在生成修改"),
        Stage::Judge => ("judge", "正在评估置信度"),
    }
}

/// Proposes a fix for one comment, reporting each stage as a `fix-progress` event.
async fn run_fix(
    app: &AppHandle,
    core: &Core,
    open: &Arc<OpenDocument>,
    doc_id: u64,
    comment_id: &str,
    request: &FixRequest,
) -> Res<Proposal> {
    let emit = |stage, message, proposal, error| {
        let _ = app.emit(
            "fix-progress",
            FixProgress {
                doc_id,
                comment_id: comment_id.to_string(),
                stage,
                message,
                proposal,
                error,
            },
        );
    };
    let result = async {
        let job = {
            let doc = open.doc.read().unwrap();
            FixJob {
                doc_key: open.key.clone(),
                doc_name: open.file_name(),
                input: context::gather_request(&doc, comment_id, request).map_err(err)?,
            }
        };
        let check = |changes: &[(usize, String)]| -> app_core::Result<()> {
            Ok(open.doc.read().unwrap().check_edits(changes)?)
        };
        core.propose(job, check, |stage| {
            let (name, text) = stage_text(stage);
            emit(name, Some(text), None, None);
        })
        .await
        .map_err(err)
    }
    .await;
    match result {
        Ok(mut p) => {
            p.doc_id = doc_id;
            emit("done", None, Some(p.clone()), None);
            Ok(p)
        }
        Err(e) => {
            emit("error", None, None, Some(e.clone()));
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn fix_comment(
    doc_id: u64,
    comment_id: String,
    request: Option<FixRequest>,
    app: AppHandle,
    docs: Docs<'_>,
    core: CoreState<'_>,
) -> Res<Proposal> {
    let open = docs.get(doc_id)?;
    let request = request.unwrap_or_default();
    run_fix(&app, &core, &open, doc_id, &comment_id, &request).await
}

/// Starts fixes for several comments, a few at a time; results arrive as
/// `fix-progress` events.
#[tauri::command]
pub async fn fix_batch(
    doc_id: u64,
    comment_ids: Vec<String>,
    app: AppHandle,
    docs: Docs<'_>,
    core: CoreState<'_>,
) -> Res<()> {
    let open = docs.get(doc_id)?;
    let core = core.inner().clone();
    core.require(RoleName::Chat).map_err(err)?;
    let limit = core.settings().map_err(err)?.fix.concurrency.max(1);
    let permits = Arc::new(Semaphore::new(limit));
    for comment_id in comment_ids {
        let (app, core, open, permits) = (app.clone(), core.clone(), open.clone(), permits.clone());
        tauri::async_runtime::spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            let request = FixRequest::default();
            let _ = run_fix(&app, &core, &open, doc_id, &comment_id, &request).await;
        });
    }
    Ok(())
}

/// Refreshes a reviewer's profile in the background once enough new cases
/// have been decided.
fn distill_later(core: Arc<Core>, reviewer_id: Option<i64>) {
    if let Some(id) = reviewer_id {
        tauri::async_runtime::spawn(async move {
            if let Err(e) = core.distill_profile(id).await {
                eprintln!("审稿人画像提炼失败：{e}");
            }
        });
    }
}

#[tauri::command]
pub async fn apply_fix(
    doc_id: u64,
    proposal_id: String,
    edited: Option<Vec<String>>,
    force: bool,
    docs: Docs<'_>,
    core: CoreState<'_>,
) -> Res<EditOutcome> {
    let open = docs.get(doc_id)?;
    let core = core.inner().clone();
    let (outcome, due) = blocking({
        let core = core.clone();
        move || {
            let mut doc = open.doc.write().unwrap();
            let due = core
                .apply_fix(&mut doc, &proposal_id, edited, force)
                .map_err(err)?;
            Ok((open.outcome(&doc), due))
        }
    })
    .await?;
    distill_later(core, due);
    Ok(outcome)
}

#[tauri::command]
pub async fn reject_fix(proposal_id: String, core: CoreState<'_>) -> Res<()> {
    let core = core.inner().clone();
    let due = blocking({
        let core = core.clone();
        move || core.reject_fix(&proposal_id).map_err(err)
    })
    .await?;
    distill_later(core, due);
    Ok(())
}

// Reviewers

/// Runs a store operation off the UI thread.
async fn with_core<T: Send + 'static>(
    core: &CoreState<'_>,
    f: impl FnOnce(&Core) -> app_core::Result<T> + Send + 'static,
) -> Res<T> {
    let core = core.inner().clone();
    blocking(move || f(&core).map_err(err)).await
}

fn reviewer(core: &Core, id: i64) -> app_core::Result<Reviewer> {
    core.store()
        .reviewer(id)?
        .ok_or_else(|| app_core::Error::Invalid("审稿人不存在".into()))
}

fn check_name(core: &Core, name: &str, except: Option<i64>) -> app_core::Result<()> {
    if name.trim().is_empty() {
        return Err(app_core::Error::Invalid("请填写审稿人姓名".into()));
    }
    match core.store().reviewer_by_name(name.trim())? {
        Some(r) if Some(r.id) != except => Err(app_core::Error::Invalid(format!(
            "已有名为「{}」的审稿人",
            r.name
        ))),
        _ => Ok(()),
    }
}

#[tauri::command]
pub async fn list_reviewers(core: CoreState<'_>) -> Res<Vec<Reviewer>> {
    with_core(&core, |c| c.store().reviewers()).await
}

#[tauri::command]
pub async fn create_reviewer(name: String, note: String, core: CoreState<'_>) -> Res<Reviewer> {
    with_core(&core, move |c| {
        check_name(c, &name, None)?;
        c.store().create_reviewer(&name, note.trim())
    })
    .await
}

#[tauri::command]
pub async fn update_reviewer(
    id: i64,
    name: String,
    note: String,
    threshold: Option<f32>,
    core: CoreState<'_>,
) -> Res<Reviewer> {
    with_core(&core, move |c| {
        check_name(c, &name, Some(id))?;
        let threshold = threshold
            .filter(|t| t.is_finite())
            .map(|t| t.clamp(0.5, 0.99));
        c.store()
            .update_reviewer(id, &name, note.trim(), threshold)?;
        reviewer(c, id)
    })
    .await
}

#[tauri::command]
pub async fn delete_reviewer(id: i64, core: CoreState<'_>) -> Res<()> {
    with_core(&core, move |c| c.store().delete_reviewer(id)).await
}

#[tauri::command]
pub async fn merge_reviewers(from: i64, into: i64, core: CoreState<'_>) -> Res<()> {
    with_core(&core, move |c| {
        if from == into {
            return Err(app_core::Error::Invalid("不能合并到同一位审稿人".into()));
        }
        reviewer(c, into)?;
        c.store().merge_reviewers(from, into)
    })
    .await
}

#[tauri::command]
pub async fn reviewer_profile(id: i64, core: CoreState<'_>) -> Res<Option<ProfileView>> {
    with_core(&core, move |c| profiles::view(&c.store(), id)).await
}

#[tauri::command]
pub async fn distill_profile(id: i64, core: CoreState<'_>) -> Res<ProfileView> {
    core.distill_profile(id).await.map_err(err)
}

#[tauri::command]
pub async fn reviewer_cases(id: i64, core: CoreState<'_>) -> Res<Vec<Case>> {
    with_core(&core, move |c| c.store().reviewer_cases(id, 500)).await
}

#[tauri::command]
pub async fn pre_review(
    doc_id: u64,
    reviewer_id: i64,
    start: usize,
    end: usize,
    docs: Docs<'_>,
    core: CoreState<'_>,
) -> Res<Vec<PreReviewItem>> {
    let open = docs.get(doc_id)?;
    let paragraphs: Vec<(usize, String)> = {
        let doc = open.doc.read().unwrap();
        let end = end.min(doc.paragraphs.len());
        (start.min(end)..end)
            .map(|i| (i, doc.editable_text(i)))
            .collect()
    };
    core.pre_review(reviewer_id, &paragraphs).await.map_err(err)
}

#[tauri::command]
pub async fn export_dataset(path: String, core: CoreState<'_>) -> Res<usize> {
    with_core(&core, move |c| c.export_dataset(&PathBuf::from(path))).await
}

// Settings and models

#[tauri::command]
pub async fn get_settings(core: CoreState<'_>) -> Res<Settings> {
    with_core(&core, |c| c.settings()).await
}

#[tauri::command]
pub async fn save_settings(settings: Settings, core: CoreState<'_>) -> Res<Settings> {
    with_core(&core, move |c| c.save_settings(settings)).await
}

#[tauri::command]
pub async fn list_providers(core: CoreState<'_>) -> Res<Vec<ProviderView>> {
    with_core(&core, |c| providers::views(&c.store(), c.secrets())).await
}

#[tauri::command]
pub async fn save_provider(
    provider: ProviderRecord,
    api_key: Option<String>,
    keep_key: bool,
    core: CoreState<'_>,
) -> Res<ProviderView> {
    with_core(&core, move |c| c.save_provider(provider, api_key, keep_key)).await
}

#[tauri::command]
pub async fn delete_provider(id: String, core: CoreState<'_>) -> Res<()> {
    with_core(&core, move |c| c.delete_provider(&id)).await
}

#[tauri::command]
pub async fn fetch_models(provider_id: String, core: CoreState<'_>) -> Res<Vec<ModelView>> {
    core.fetch_models(&provider_id).await.map_err(err)
}

#[tauri::command]
pub async fn provider_models(provider_id: String, core: CoreState<'_>) -> Res<Vec<ModelView>> {
    with_core(&core, move |c| {
        providers::model_views(&c.store(), &provider_id)
    })
    .await
}

#[tauri::command]
pub async fn set_model_profile(
    provider_id: String,
    model_id: String,
    profile: Option<ProfileOverride>,
    core: CoreState<'_>,
) -> Res<ModelView> {
    with_core(&core, move |c| {
        providers::set_override(&c.store(), &provider_id, &model_id, profile.as_ref())
    })
    .await
}

#[tauri::command]
pub async fn test_role(role: RoleName, core: CoreState<'_>) -> Res<ProbeResult> {
    Ok(core.test_role(role).await)
}
