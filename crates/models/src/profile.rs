//! Model capability profiles: context window, output limit and how (if at all)
//! the model's thinking can be controlled.

use serde::{Deserialize, Serialize};

use crate::list::ModelInfo;
use crate::provider::base_id;

/// Context window used when nothing better is known.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 32_768;
/// Output limit used when nothing better is known.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4_096;

/// A unified thinking level; adapters map it onto each API's parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    /// Thinking disabled.
    Off,
    /// Light thinking.
    Low,
    /// Moderate thinking.
    Medium,
    /// Deep thinking.
    High,
}

impl ThinkingLevel {
    /// `none` / `low` / `medium` / `high`, as used by `reasoning_effort`.
    pub fn effort_str(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "none",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
        }
    }

    /// Anthropic-style token budget for this level (0 for `Off`).
    pub fn budget_tokens(self) -> u32 {
        match self {
            ThinkingLevel::Off => 0,
            ThinkingLevel::Low => 2_048,
            ThinkingLevel::Medium => 8_192,
            ThinkingLevel::High => 24_576,
        }
    }
}

/// How a model's reasoning can be controlled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    /// No reasoning, or not controllable and not produced.
    None,
    /// Discrete effort levels (e.g. `reasoning_effort` low/medium/high).
    Effort(Vec<ThinkingLevel>),
    /// Thinking can be switched on or off.
    Toggle,
    /// Thinking takes a token budget within `min..=max`.
    Budget {
        /// Smallest accepted budget.
        min: u32,
        /// Largest useful budget.
        max: u32,
    },
    /// The model always thinks; nothing to configure.
    AlwaysOn,
}

/// How the thinking parameter is sent for a model family on an OpenAI-style
/// API. OpenRouter and Anthropic providers always use their own style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingParam {
    /// `reasoning_effort: "low" | "medium" | "high" | "none"` (OpenAI, Gemini).
    ReasoningEffort,
    /// `reasoning: {effort}` or `reasoning: {enabled: false}` (OpenRouter).
    OpenRouterReasoning,
    /// `enable_thinking: bool` (Qwen / DashScope, SiliconFlow).
    EnableThinking,
    /// `thinking: {type: "enabled" | "disabled"}` (DeepSeek, GLM, Doubao).
    ThinkingType,
    /// `thinking: {type: "enabled", budget_tokens}` (Anthropic).
    AnthropicBudget,
}

/// Where a profile's context window came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    /// Reported by the provider's model list.
    Fetched,
    /// From the built-in table of known families.
    BuiltIn,
    /// Entered by the user.
    Manual,
    /// Nothing known; crate defaults.
    Default,
}

/// Capabilities of one model, used to budget prompts and map thinking levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Total context window in tokens.
    pub context_window: u32,
    /// Maximum output tokens per response.
    pub max_output_tokens: u32,
    /// Reasoning control.
    pub reasoning: Reasoning,
    /// Whether `response_format: json_object` may be sent.
    pub json_mode: bool,
    /// How to send the thinking parameter on OpenAI-style APIs.
    pub thinking_param: ThinkingParam,
    /// Where `context_window` came from.
    pub source: ProfileSource,
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            reasoning: Reasoning::None,
            json_mode: true,
            thinking_param: ThinkingParam::ReasoningEffort,
            source: ProfileSource::Default,
        }
    }
}

/// User-entered overrides; every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartialProfile {
    /// Context window override.
    pub context_window: Option<u32>,
    /// Output limit override.
    pub max_output_tokens: Option<u32>,
    /// Reasoning override.
    pub reasoning: Option<Reasoning>,
    /// JSON mode override.
    pub json_mode: Option<bool>,
    /// Thinking parameter style override (e.g. a gateway that wants `enable_thinking`).
    pub thinking_param: Option<ThinkingParam>,
}

const K128: u32 = 131_072;
const K256: u32 = 262_144;
const M1: u32 = 1_048_576;

/// Looks up a well-known model family by substring of the lowercased id
/// (without its `org/` prefix). Size suffixes such as `-32k` override the
/// family's context window.
pub fn builtin_profile(model_id: &str) -> Option<ModelProfile> {
    use Reasoning::{AlwaysOn, None as NoReasoning, Toggle};
    use ThinkingParam::*;

    let id = base_id(model_id);
    let has = |needles: &[&str]| needles.iter().any(|n| id.contains(n));
    let lmh = || {
        Reasoning::Effort(vec![
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ])
    };
    let budget = |min, max| Reasoning::Budget { min, max };

    let (ctx, out, reasoning, param) = if has(&["deepseek-reasoner", "deepseek-r1"]) {
        (K128, 32_768, AlwaysOn, ThinkingType)
    } else if has(&["deepseek-v3", "deepseek-v2", "deepseek-coder"])
        && !has(&["v3.1", "v3-1", "v3p1", "v3.2", "v3-2", "v3p2"])
    {
        (K128, 8_192, NoReasoning, ThinkingType)
    } else if has(&["deepseek"]) {
        // V3.1+ (deepseek-chat, v3.1/v3.2, v4.x incl. flash) are hybrid models.
        // Some gateways (e.g. SiliconFlow) want `enable_thinking` instead.
        (K128, 8_192, Toggle, ThinkingType)
    } else if has(&["qwq", "qvq"]) {
        (K128, 8_192, AlwaysOn, EnableThinking)
    } else if has(&["qwen-long"]) {
        (1_000_000, 8_192, NoReasoning, EnableThinking)
    } else if has(&["qwen3"]) && has(&["thinking"]) {
        // Qwen3 2507 thinking variants (unsure: context differs per size).
        (K256, 32_768, AlwaysOn, EnableThinking)
    } else if has(&["qwen3"]) && has(&["instruct", "coder", "max"]) {
        // 2507 instruct / coder / qwen3-max are non-thinking (unsure for later qwen3-max releases).
        (K256, 32_768, NoReasoning, EnableThinking)
    } else if has(&["qwen3", "qwen-plus", "qwen-turbo", "qwen-flash"]) {
        (K128, 8_192, Toggle, EnableThinking)
    } else if has(&["qwen-max"]) {
        // DashScope documents qwen-max (2.5) with a 32k window.
        (32_768, 8_192, NoReasoning, EnableThinking)
    } else if has(&["qwen"]) {
        (K128, 8_192, NoReasoning, EnableThinking)
    } else if has(&["glm-z1"]) {
        // Z1 models always reason (unsure: z1-airx is 32k).
        (K128, 32_768, AlwaysOn, ThinkingType)
    } else if has(&["glm-4.6", "glm-4.7", "glm-5"]) {
        (200_000, 16_384, Toggle, ThinkingType)
    } else if has(&["glm-4.5"]) {
        (K128, 16_384, Toggle, ThinkingType)
    } else if has(&["glm"]) {
        (K128, 4_096, NoReasoning, ThinkingType)
    } else if has(&["kimi-k2-thinking"]) {
        (K256, 32_768, AlwaysOn, ThinkingType)
    } else if has(&["kimi-k2-0711"]) {
        (K128, 16_384, NoReasoning, ThinkingType)
    } else if has(&["kimi-k2"]) {
        (K256, 16_384, NoReasoning, ThinkingType)
    } else if has(&["kimi", "moonshot"]) {
        (K128, 8_192, NoReasoning, ThinkingType)
    } else if has(&["doubao-seed", "seed-1."]) {
        // Doubao Seed 1.6+ also accepts `thinking: {type: "auto"}`.
        (K256, 32_768, Toggle, ThinkingType)
    } else if has(&["doubao"]) && has(&["thinking"]) {
        (K128, 16_384, AlwaysOn, ThinkingType)
    } else if has(&["doubao"]) {
        // Older doubao-1.5 models encode the window in the name (-32k / -256k).
        (K128, 8_192, NoReasoning, ThinkingType)
    } else if id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") {
        // o1-mini / o1-preview: 128k and no `reasoning_effort` (unsure how gateways treat them).
        (200_000, 100_000, lmh(), ReasoningEffort)
    } else if has(&["gpt-5"]) && has(&["chat"]) {
        (128_000, 16_384, NoReasoning, ReasoningEffort)
    } else if has(&["gpt-5"]) {
        (400_000, 128_000, lmh(), ReasoningEffort)
    } else if has(&["gpt-oss"]) {
        (K128, 32_768, lmh(), ReasoningEffort)
    } else if has(&["gpt-4.1"]) {
        (1_047_576, 32_768, NoReasoning, ReasoningEffort)
    } else if has(&["gpt-4o"]) {
        (128_000, 16_384, NoReasoning, ReasoningEffort)
    } else if has(&["claude-3-7", "claude-3.7"]) {
        (200_000, 64_000, budget(1_024, 32_000), AnthropicBudget)
    } else if has(&["claude-3-5", "claude-3.5"]) {
        (200_000, 8_192, NoReasoning, AnthropicBudget)
    } else if has(&["claude-3"]) {
        (200_000, 4_096, NoReasoning, AnthropicBudget)
    } else if has(&["claude"]) && has(&["opus"]) {
        // Opus 4 / 4.1 cap output at 32k; later Opus releases allow more.
        (200_000, 32_000, budget(1_024, 32_000), AnthropicBudget)
    } else if has(&["claude"]) {
        (200_000, 64_000, budget(1_024, 32_000), AnthropicBudget)
    } else if has(&["gemini-2.5-pro"]) {
        // Pro cannot disable thinking; budget 128..32768.
        (M1, 65_536, budget(128, 32_768), ReasoningEffort)
    } else if has(&["gemini-2.5"]) {
        (M1, 65_536, budget(0, 24_576), ReasoningEffort)
    } else if has(&["gemini"]) {
        (M1, 8_192, NoReasoning, ReasoningEffort)
    } else if has(&["minimax-m2"]) {
        // M2 always thinks and returns `<think>` inline (unsure: output cap).
        (204_800, 32_768, AlwaysOn, ReasoningEffort)
    } else if has(&["minimax-m1"]) {
        (1_000_000, 40_000, AlwaysOn, ReasoningEffort)
    } else if has(&["minimax", "abab"]) {
        // MiniMax-Text-01 (unsure for older abab models).
        (1_000_192, 8_192, NoReasoning, ReasoningEffort)
    } else if has(&["ernie-x1"]) {
        // Unsure: X1 / X1.1 windows vary between 32k and 128k.
        (32_768, 16_384, AlwaysOn, EnableThinking)
    } else if has(&["ernie-4.5", "ernie-5"]) {
        (K128, 12_288, NoReasoning, EnableThinking)
    } else if has(&["ernie"]) {
        // ERNIE 3.5 / 4.0 encode the window in the name (-8k / -128k).
        (32_768, 4_096, NoReasoning, EnableThinking)
    } else {
        return None;
    };

    let context_window = context_from_name(&id).unwrap_or(ctx);
    Some(ModelProfile {
        context_window,
        max_output_tokens: out.min(context_window / 2),
        reasoning,
        json_mode: true,
        thinking_param: param,
        source: ProfileSource::BuiltIn,
    })
}

/// Parses a size segment such as `-8k`, `-128k` or `-1m` from a model id.
fn context_from_name(id: &str) -> Option<u32> {
    id.split(['-', '_', ':']).find_map(|seg| {
        if let Some(n) = seg.strip_suffix('k').and_then(|n| n.parse::<u32>().ok()) {
            (4..=2048).contains(&n).then(|| n * 1024)
        } else {
            let n: u32 = seg.strip_suffix('m')?.parse().ok()?;
            (1..=10).contains(&n).then(|| n * 1_048_576)
        }
    })
}

/// Resolves a profile field by field: manual > fetched > built-in > defaults.
/// `source` records where the context window came from.
pub fn resolve_profile(
    model_id: &str,
    fetched: Option<&ModelInfo>,
    manual: Option<&PartialProfile>,
) -> ModelProfile {
    let builtin = builtin_profile(model_id);
    let builtin = builtin.as_ref();
    let default = ModelProfile::default();

    let (context_window, source) = if let Some(v) = manual.and_then(|m| m.context_window) {
        (v, ProfileSource::Manual)
    } else if let Some(v) = fetched.and_then(|f| f.context_window) {
        (v, ProfileSource::Fetched)
    } else if let Some(b) = builtin {
        (b.context_window, ProfileSource::BuiltIn)
    } else {
        (default.context_window, ProfileSource::Default)
    };

    let max_output_tokens = manual
        .and_then(|m| m.max_output_tokens)
        .or_else(|| fetched.and_then(|f| f.max_output_tokens))
        .or_else(|| builtin.map(|b| b.max_output_tokens))
        .unwrap_or(default.max_output_tokens)
        .min(context_window);

    let reasoning = manual
        .and_then(|m| m.reasoning.clone())
        .or_else(|| fetched.and_then(|f| f.reasoning.clone()))
        .or_else(|| builtin.map(|b| b.reasoning.clone()))
        .unwrap_or(default.reasoning);

    ModelProfile {
        context_window,
        max_output_tokens,
        reasoning,
        json_mode: manual
            .and_then(|m| m.json_mode)
            .or_else(|| builtin.map(|b| b.json_mode))
            .unwrap_or(default.json_mode),
        thinking_param: manual
            .and_then(|m| m.thinking_param)
            .or_else(|| builtin.map(|b| b.thinking_param))
            .unwrap_or(default.thinking_param),
        source,
    }
}

/// Thinking levels the UI should offer for `profile` (empty: no control).
pub fn available_levels(profile: &ModelProfile) -> Vec<ThinkingLevel> {
    use ThinkingLevel::*;
    match &profile.reasoning {
        Reasoning::None | Reasoning::AlwaysOn => Vec::new(),
        Reasoning::Effort(levels) => {
            let mut levels = levels.clone();
            levels.sort();
            levels.dedup();
            levels
        }
        Reasoning::Toggle => vec![Off, High],
        Reasoning::Budget { .. } => vec![Off, Low, Medium, High],
    }
}

/// The level actually sent: `None` when unsupported. Toggle models treat any
/// non-`Off` level as "on" (`High`).
pub(crate) fn effective_level(
    profile: &ModelProfile,
    requested: Option<ThinkingLevel>,
) -> Option<ThinkingLevel> {
    let level = requested?;
    if available_levels(profile).contains(&level) {
        Some(level)
    } else if profile.reasoning == Reasoning::Toggle {
        Some(ThinkingLevel::High)
    } else {
        None
    }
}
