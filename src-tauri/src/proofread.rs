//! Document proofreading (文档校对).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use app_core::Core;
use app_core::proofread::{self, Category, Issue, ProofOptions, ProofProgress, ProofReport};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::documents::{EditOutcome, OpenDocuments};
use crate::{Res, blocking, err};

type Docs<'a> = State<'a, Arc<OpenDocuments>>;
type CoreState<'a> = State<'a, Arc<Core>>;

/// Set by `cancel_proofread`; one proofreading run at a time.
static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    doc_id: u64,
    #[serde(flatten)]
    progress: ProofProgress,
    /// Text of each paragraph with a finding in `progress.found`.
    paragraphs: BTreeMap<usize, &'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofResult {
    #[serde(flatten)]
    report: ProofReport,
    /// Text of each paragraph with an issue, as it was checked.
    paragraphs: BTreeMap<usize, String>,
}

/// Proofreads the document, or paragraphs `range` (first and last index,
/// inclusive), reporting each step as a `proofread-progress` event.
#[tauri::command]
pub async fn proofread(
    doc_id: u64,
    mut options: ProofOptions,
    range: Option<(usize, usize)>,
    app: AppHandle,
    docs: Docs<'_>,
    core: CoreState<'_>,
) -> Res<ProofResult> {
    let open = docs.get(doc_id)?;
    let input = {
        let doc = open.doc.read().unwrap();
        if range.is_some() && options.facts.is_none() {
            // Project facts come from the whole document, not the range.
            options.facts = Some(proofread::extract_facts(
                &proofread::gather(&doc, None).paragraphs,
            ));
        }
        proofread::gather(&doc, range)
    };
    if options.doc_key.is_empty() {
        options.doc_key = open.key.clone();
    }
    let wants_citations =
        options.categories.is_empty() || options.categories.contains(&Category::Citation);
    let lookup = if wants_citations {
        core.citation_lookup().map_err(err)?
    } else {
        None
    };
    CANCEL.store(false, Ordering::SeqCst);
    let texts: std::collections::HashMap<usize, &str> = input
        .paragraphs
        .iter()
        .map(|p| (p.index, p.text.as_str()))
        .collect();
    let report = core
        .proofread(
            &input,
            &options,
            lookup,
            |progress| {
                let paragraphs = progress
                    .found
                    .iter()
                    .filter_map(|i| Some((i.paragraph, *texts.get(&i.paragraph)?)))
                    .collect();
                let _ = app.emit(
                    "proofread-progress",
                    Progress {
                        doc_id,
                        progress,
                        paragraphs,
                    },
                );
            },
            &CANCEL,
        )
        .await
        .map_err(err)?;
    drop(texts);
    let wanted: std::collections::HashSet<usize> =
        report.issues.iter().map(|i| i.paragraph).collect();
    let paragraphs = input
        .paragraphs
        .into_iter()
        .filter(|p| wanted.contains(&p.index))
        .map(|p| (p.index, p.text))
        .collect();
    Ok(ProofResult { report, paragraphs })
}

/// Stops the running proofread before its next model request.
#[tauri::command]
pub fn cancel_proofread() {
    CANCEL.store(true, Ordering::SeqCst);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    outcome: EditOutcome,
    /// Ids of the issues written into the document.
    applied: Vec<String>,
}

/// Writes the suggestions of `issues` into the document as one undo step.
#[tauri::command]
pub async fn apply_proof_issues(
    doc_id: u64,
    issues: Vec<Issue>,
    docs: Docs<'_>,
    core: CoreState<'_>,
) -> Res<Applied> {
    let open = docs.get(doc_id)?;
    let core = core.inner().clone();
    blocking(move || {
        let mut doc = open.doc.write().unwrap();
        let applied = core.apply_proof_issues(&mut doc, &issues).map_err(err)?;
        Ok(Applied {
            outcome: open.outcome(&doc),
            applied,
        })
    })
    .await
}

/// Forgets cached model results, so the next run checks everything again.
#[tauri::command]
pub async fn clear_proofread_cache(core: CoreState<'_>) -> Res<()> {
    let core = core.inner().clone();
    blocking(move || core.store().clear_proofread_cache(None).map_err(err)).await
}
