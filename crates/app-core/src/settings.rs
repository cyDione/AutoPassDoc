//! User settings, stored as JSON values in the `settings` table.

use serde::{Deserialize, Serialize};

use docx_engine::EditMode;

pub use crate::web::WebSettings;

/// Which provider and model serve one role.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoleModel {
    pub provider_id: String,
    pub model: String,
    /// "off" | "low" | "medium" | "high"; empty = the model's default.
    pub thinking: String,
}

impl RoleModel {
    pub fn is_set(&self) -> bool {
        !self.provider_id.is_empty() && !self.model.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionBackend {
    /// Jev through the `systemone` decision API.
    #[default]
    Jev,
    /// A chat model asked for probabilities (fallback without Jev access).
    Chat,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Roles {
    pub chat: RoleModel,
    pub decision: RoleModel,
    pub decision_backend: DecisionBackend,
    pub embedding: RoleModel,
    pub rerank: RoleModel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FixSettings {
    /// Confidence needed for one-click apply.
    pub threshold: f32,
    pub edit_mode: EditMode,
    /// Revision and reply author shown in Word.
    pub author: String,
    pub resolve_on_apply: bool,
    pub reply_on_apply: bool,
    pub reply_text: String,
    pub use_kb: bool,
    /// Knowledge-base passages given to the model.
    pub kb_passages: usize,
    /// Parallel requests in batch fixes.
    pub concurrency: usize,
    /// Distil a reviewer profile after this many new cases.
    pub profile_every: usize,
}

impl Default for FixSettings {
    fn default() -> Self {
        Self {
            threshold: 0.8,
            edit_mode: EditMode::Tracked,
            author: "AutoPassDoc".into(),
            resolve_on_apply: true,
            reply_on_apply: false,
            reply_text: "已根据该意见修改。".into(),
            use_kb: true,
            kb_passages: 6,
            concurrency: 3,
            profile_every: 10,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub roles: Roles,
    pub fix: FixSettings,
    pub web: WebSettings,
}

impl Settings {
    /// Brings values from the settings screen into their usable ranges.
    pub fn normalized(mut self) -> Self {
        let fix = &mut self.fix;
        fix.threshold = if fix.threshold.is_finite() {
            fix.threshold.clamp(0.5, 0.99)
        } else {
            FixSettings::default().threshold
        };
        fix.author = fix.author.trim().to_string();
        if fix.author.is_empty() {
            fix.author = FixSettings::default().author;
        }
        fix.kb_passages = fix.kb_passages.clamp(1, 20);
        fix.concurrency = fix.concurrency.clamp(1, 8);
        fix.profile_every = fix.profile_every.clamp(1, 200);
        let mut seen = std::collections::HashSet::new();
        self.web.whitelist = self
            .web
            .whitelist
            .iter()
            .map(|w| {
                w.trim()
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            })
            .filter(|w| !w.is_empty() && seen.insert(w.clone()))
            .collect();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_partial_json_and_clamps() {
        let s: Settings =
            serde_json::from_str(r#"{"fix": {"threshold": 1.5, "concurrency": 0, "author": " "}}"#)
                .unwrap();
        let s = s.normalized();
        assert_eq!(s.fix.threshold, 0.99);
        assert_eq!(s.fix.concurrency, 1);
        assert_eq!(s.fix.author, "AutoPassDoc");
        assert!(s.fix.resolve_on_apply, "missing fields keep their defaults");
    }
}
