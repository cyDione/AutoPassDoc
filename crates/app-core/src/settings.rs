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
    /// Look up public material (policies, plans, standards) on the web
    /// before writing a fix, and again for gaps the model left.
    pub use_web: bool,
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
            use_web: true,
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
    pub kb: KbSettings,
    pub proofread: ProofreadSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProofreadSettings {
    /// Parallel model requests while proofreading.
    pub concurrency: usize,
    /// Let the chat model think before answering. Off by default: finding
    /// typos gains little from it and it makes every section several times
    /// slower.
    pub thinking: bool,
}

impl Default for ProofreadSettings {
    fn default() -> Self {
        Self {
            concurrency: 6,
            thinking: false,
        }
    }
}

/// Who reads PDFs and images into the knowledge base.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParserKind {
    /// The built-in parsers (Word, PDF text layer, TXT, Markdown).
    #[default]
    Builtin,
    /// MinerU (mineru.net) online document parsing.
    Mineru,
    /// PaddleOCR on Baidu AI Studio.
    Paddleocr,
}

impl ParserKind {
    /// The name stored with each document (`KbDocument::parser`).
    pub fn id(self) -> &'static str {
        match self {
            ParserKind::Builtin => kb::BUILTIN_PARSER,
            ParserKind::Mineru => "mineru",
            ParserKind::Paddleocr => "paddleocr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ParserKind::Builtin => "普通",
            ParserKind::Mineru => "MinerU",
            ParserKind::Paddleocr => "PaddleOCR",
        }
    }

    /// Where the service's key is kept in the secret store; `None` for the
    /// built-in parser.
    pub fn secret_name(self) -> Option<&'static str> {
        match self {
            ParserKind::Builtin => None,
            ParserKind::Mineru => Some("parser:mineru"),
            ParserKind::Paddleocr => Some("parser:paddleocr"),
        }
    }
}

pub const DEFAULT_MINERU_MODEL: &str = "vlm";
pub const DEFAULT_PADDLEOCR_BASE_URL: &str = "https://paddleocr.aistudio-app.com";
pub const DEFAULT_PADDLEOCR_MODEL: &str = "PaddleOCR-VL-1.6";

/// Knowledge-base import: the parser for PDFs and images. An online parser
/// is used only once its key is saved (enhanced mode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KbSettings {
    pub parser: ParserKind,
    /// MinerU `model_version`: `vlm` or `pipeline`.
    pub mineru_model: String,
    /// MinerU `is_ocr`: OCR every page, for scans and image-only PDFs.
    pub mineru_ocr: bool,
    /// MinerU `enable_formula`: recognise formulas.
    pub mineru_formula: bool,
    /// PaddleOCR service address; change it for a self-hosted service.
    pub paddleocr_base_url: String,
    /// `PaddleOCR-VL-1.6` or `PP-StructureV3`.
    pub paddleocr_model: String,
}

impl Default for KbSettings {
    fn default() -> Self {
        Self {
            parser: ParserKind::Builtin,
            mineru_model: DEFAULT_MINERU_MODEL.into(),
            mineru_ocr: true,
            mineru_formula: true,
            paddleocr_base_url: DEFAULT_PADDLEOCR_BASE_URL.into(),
            paddleocr_model: DEFAULT_PADDLEOCR_MODEL.into(),
        }
    }
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
        self.proofread.concurrency = self.proofread.concurrency.clamp(1, 16);
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
        let kb = &mut self.kb;
        let or_default = |v: &str, default: &str| {
            let v = v.trim();
            if v.is_empty() { default } else { v }.to_string()
        };
        kb.mineru_model = or_default(&kb.mineru_model, DEFAULT_MINERU_MODEL);
        kb.paddleocr_model = or_default(&kb.paddleocr_model, DEFAULT_PADDLEOCR_MODEL);
        kb.paddleocr_base_url = or_default(
            kb.paddleocr_base_url.trim().trim_end_matches('/'),
            DEFAULT_PADDLEOCR_BASE_URL,
        );
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
        assert_eq!(s.kb, KbSettings::default());
    }

    #[test]
    fn reads_kb_settings() {
        let s: Settings = serde_json::from_str(
            r#"{"roles": {}, "kb": {"parser": "paddleocr", "paddleocrBaseUrl": " http://ocr.local/ ", "mineruModel": ""}}"#,
        )
        .unwrap();
        let s = s.normalized();
        assert_eq!(s.kb.parser, ParserKind::Paddleocr);
        assert_eq!(s.kb.paddleocr_base_url, "http://ocr.local");
        assert_eq!(s.kb.mineru_model, "vlm");
        assert!(
            s.kb.mineru_ocr && s.kb.mineru_formula,
            "on unless turned off"
        );
        assert_eq!(s.kb.paddleocr_model, "PaddleOCR-VL-1.6");
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["kb"]["parser"], "paddleocr");
        assert_eq!(json["kb"]["paddleocrModel"], "PaddleOCR-VL-1.6");
    }
}
