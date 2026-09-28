//! User settings, stored as JSON values in the `settings` table.

use serde::{Deserialize, Serialize};

use docx_engine::EditMode;

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
}
