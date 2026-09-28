//! Exports the decided fixes as JSON Lines, to evaluate or fine-tune the
//! decision model on the user's own accept/edit/reject history.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use serde_json::json;

use crate::core::Core;
use crate::error::Result;
use crate::store::CaseAction;

impl Core {
    /// Writes one line per accepted, edited or rejected fix; returns the count.
    pub fn export_dataset(&self, path: &Path) -> Result<usize> {
        let (cases, names) = {
            let store = self.store();
            let names: HashMap<i64, String> = store
                .reviewers()?
                .into_iter()
                .map(|r| (r.id, r.name))
                .collect();
            (store.decided_cases()?, names)
        };
        let tmp = path.with_extension("jsonl.tmp");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        for c in &cases {
            let (label, verdict) = match c.action {
                CaseAction::Accepted => (1.0, "accepted"),
                CaseAction::Edited => (0.5, "edited"),
                CaseAction::Rejected => (0.0, "rejected"),
                CaseAction::Pending => continue,
            };
            let state = format!(
                "【审稿专家批注】\n{}\n\n【修改前】\n{}\n\n【修改后】\n{}\n",
                c.comment.trim(),
                c.original,
                c.suggestion
            );
            let judge: Option<serde_json::Value> = c
                .judge
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok());
            let line = json!({
                "id": c.id,
                "reviewer": c.reviewer_id.and_then(|id| names.get(&id)),
                "author": c.author,
                "document": c.doc_name,
                "category": c.category,
                "comment": c.comment,
                "original": c.original,
                "suggestion": c.suggestion,
                "final": c.final_text,
                "action": verdict,
                "label": label,
                "state": state,
                "judge": judge,
                "createdAt": c.created_at,
            });
            serde_json::to_writer(&mut out, &line)?;
            out.write_all(b"\n")?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(cases
            .iter()
            .filter(|c| c.action != CaseAction::Pending)
            .count())
    }
}
