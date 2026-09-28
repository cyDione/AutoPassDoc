use models::{
    ModelInfo, ModelProfile, ModelRole, PartialProfile, ProfileSource, Provider, ProviderKind,
    Reasoning, ThinkingLevel, ThinkingParam, available_levels, builtin_profile, guess_role,
    normalize_base_url, resolve_profile,
};

#[test]
fn normalizes_openai_style_base_urls() {
    let oa = |u| normalize_base_url(ProviderKind::OpenAiCompatible, u);
    assert_eq!(
        oa("https://api.deepseek.com"),
        "https://api.deepseek.com/v1"
    );
    assert_eq!(
        oa(" https://api.deepseek.com/v1/ "),
        "https://api.deepseek.com/v1"
    );
    assert_eq!(
        oa("https://gw.example.com/api/v1"),
        "https://gw.example.com/api/v1"
    );
    assert_eq!(
        oa("https://gw.example.com/api"),
        "https://gw.example.com/api/v1"
    );
    assert_eq!(oa("https://gw.example.com/v2"), "https://gw.example.com/v2");
    assert_eq!(
        oa("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        "https://dashscope.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        oa("https://open.bigmodel.cn/api/paas/v4/"),
        "https://open.bigmodel.cn/api/paas/v4"
    );
    assert_eq!(
        oa("https://generativelanguage.googleapis.com/v1beta/openai/"),
        "https://generativelanguage.googleapis.com/v1beta/openai"
    );
    assert_eq!(
        oa("https://api.siliconflow.cn/v1/chat/completions"),
        "https://api.siliconflow.cn/v1"
    );
    assert_eq!(oa("localhost:8000"), "http://localhost:8000/v1");
    assert_eq!(oa("api.example.com"), "https://api.example.com/v1");
    assert_eq!(oa("  "), "");

    let or = |u| normalize_base_url(ProviderKind::OpenRouter, u);
    assert_eq!(
        or("https://openrouter.ai/api/v1/"),
        "https://openrouter.ai/api/v1"
    );
    assert_eq!(or("https://openrouter.ai"), "https://openrouter.ai/api/v1");
    assert_eq!(or(""), "https://openrouter.ai/api/v1");
}

#[test]
fn normalizes_anthropic_and_ollama_base_urls() {
    let an = |u| normalize_base_url(ProviderKind::Anthropic, u);
    assert_eq!(an(""), "https://api.anthropic.com");
    assert_eq!(
        an("https://api.anthropic.com/"),
        "https://api.anthropic.com"
    );
    assert_eq!(
        an("https://api.anthropic.com/v1"),
        "https://api.anthropic.com"
    );
    assert_eq!(
        an("https://proxy.example.com/anthropic/v1/messages"),
        "https://proxy.example.com/anthropic"
    );

    let ol = |u| normalize_base_url(ProviderKind::Ollama, u);
    assert_eq!(ol(""), "http://localhost:11434");
    assert_eq!(ol("http://localhost:11434/"), "http://localhost:11434");
    assert_eq!(ol("http://localhost:11434/v1"), "http://localhost:11434");
    assert_eq!(ol("127.0.0.1:11434/api"), "http://127.0.0.1:11434");
}

#[test]
fn guesses_roles_from_model_ids() {
    let cases = [
        ("BAAI/bge-reranker-v2-m3", ModelRole::Rerank),
        ("jina-reranker-v2-base-multilingual", ModelRole::Rerank),
        ("BAAI/bge-m3", ModelRole::Embedding),
        ("bge-large-zh-v1.5", ModelRole::Embedding),
        ("text-embedding-3-small", ModelRole::Embedding),
        ("nomic-embed-text:latest", ModelRole::Embedding),
        ("intfloat/multilingual-e5-large", ModelRole::Embedding),
        ("Alibaba-NLP/gte-Qwen2-7B-instruct", ModelRole::Embedding),
        ("moka-ai/m3e-base", ModelRole::Embedding),
        ("jina-embeddings-v3", ModelRole::Embedding),
        ("jev-latest", ModelRole::Decision),
        ("typesafe/Laya-1.5B", ModelRole::Decision),
        ("cline-pass/deepseek-v4.1-flash", ModelRole::Chat),
        ("qwen2.5-72b-instruct", ModelRole::Chat),
        ("gpt-4o", ModelRole::Chat),
    ];
    for (id, role) in cases {
        assert_eq!(guess_role(id), role, "{id}");
    }
}

#[test]
fn builtin_profiles_cover_known_families() {
    let p = |id: &str| builtin_profile(id).unwrap_or_else(|| panic!("no profile for {id}"));

    let ds = p("cline-pass/deepseek-v4.1-flash");
    assert_eq!(ds.reasoning, Reasoning::Toggle);
    assert_eq!(ds.thinking_param, ThinkingParam::ThinkingType);
    assert_eq!(ds.context_window, 131_072);
    assert_eq!(ds.source, ProfileSource::BuiltIn);
    assert_eq!(p("deepseek-chat").reasoning, Reasoning::Toggle);
    assert_eq!(p("deepseek-reasoner").reasoning, Reasoning::AlwaysOn);
    assert_eq!(p("deepseek-ai/DeepSeek-R1").reasoning, Reasoning::AlwaysOn);
    assert_eq!(p("deepseek-ai/DeepSeek-V3").reasoning, Reasoning::None);
    assert_eq!(
        p("deepseek-ai/DeepSeek-V3.2-Exp").reasoning,
        Reasoning::Toggle
    );

    let qwen = p("Qwen/Qwen3-235B-A22B");
    assert_eq!(qwen.reasoning, Reasoning::Toggle);
    assert_eq!(qwen.thinking_param, ThinkingParam::EnableThinking);
    assert_eq!(p("qwen-plus").reasoning, Reasoning::Toggle);
    assert_eq!(p("qwq-32b").reasoning, Reasoning::AlwaysOn);
    assert_eq!(p("qwen-long").context_window, 1_000_000);

    let glm = p("glm-4.5");
    assert_eq!(
        (glm.reasoning, glm.thinking_param),
        (Reasoning::Toggle, ThinkingParam::ThinkingType)
    );
    assert_eq!(p("GLM-Z1-Air").reasoning, Reasoning::AlwaysOn);

    assert_eq!(p("kimi-k2-0905-preview").context_window, 262_144);
    assert_eq!(p("moonshot-v1-8k").context_window, 8_192);
    assert_eq!(p("moonshot-v1-8k").max_output_tokens, 4_096);
    assert_eq!(p("moonshot-v1-128k").context_window, 131_072);

    assert_eq!(p("doubao-seed-1-6-250615").reasoning, Reasoning::Toggle);
    assert_eq!(p("doubao-1-5-pro-32k-250115").context_window, 32_768);

    let gpt4o = p("openai/gpt-4o");
    assert_eq!(
        (gpt4o.context_window, gpt4o.reasoning),
        (128_000, Reasoning::None)
    );
    assert_eq!(p("gpt-4.1-mini").context_window, 1_047_576);
    let effort = Reasoning::Effort(vec![
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
    ]);
    assert_eq!(p("o3-mini").reasoning, effort);
    assert_eq!(p("gpt-5").reasoning, effort);
    assert_eq!(p("gpt-5").thinking_param, ThinkingParam::ReasoningEffort);

    let claude = p("claude-sonnet-4-5-20250929");
    assert_eq!(claude.context_window, 200_000);
    assert_eq!(
        claude.reasoning,
        Reasoning::Budget {
            min: 1_024,
            max: 32_000
        }
    );
    assert_eq!(claude.thinking_param, ThinkingParam::AnthropicBudget);
    assert_eq!(p("claude-3-5-sonnet-latest").reasoning, Reasoning::None);

    let gemini = p("google/gemini-2.5-flash");
    assert_eq!(gemini.context_window, 1_048_576);
    assert!(matches!(gemini.reasoning, Reasoning::Budget { .. }));

    assert_eq!(p("MiniMax-M2").reasoning, Reasoning::AlwaysOn);
    assert_eq!(p("ERNIE-4.5-turbo-128k").context_window, 131_072);

    assert!(builtin_profile("llama-3.1-8b-instruct").is_none());
    assert!(builtin_profile("BAAI/bge-m3").is_none());
}

fn info(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        display_name: None,
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        role_hint: guess_role(id),
    }
}

#[test]
fn resolve_profile_prefers_manual_then_fetched_then_builtin() {
    let d = resolve_profile("some-unknown-model", None, None);
    assert_eq!(d, ModelProfile::default());
    assert_eq!((d.context_window, d.max_output_tokens), (32_768, 4_096));
    assert_eq!(d.source, ProfileSource::Default);
    assert!(d.json_mode);

    let b = resolve_profile("deepseek-chat", None, None);
    assert_eq!(b.source, ProfileSource::BuiltIn);
    assert_eq!(b.reasoning, Reasoning::Toggle);

    let mut fetched = info("deepseek-chat");
    fetched.context_window = Some(65_536);
    fetched.max_output_tokens = Some(6_000);
    let f = resolve_profile("deepseek-chat", Some(&fetched), None);
    assert_eq!(f.source, ProfileSource::Fetched);
    assert_eq!((f.context_window, f.max_output_tokens), (65_536, 6_000));
    assert_eq!(
        f.reasoning,
        Reasoning::Toggle,
        "reasoning falls back to built-in"
    );
    assert_eq!(f.thinking_param, ThinkingParam::ThinkingType);

    let manual = PartialProfile {
        context_window: Some(16_000),
        reasoning: Some(Reasoning::None),
        json_mode: Some(false),
        thinking_param: Some(ThinkingParam::EnableThinking),
        ..Default::default()
    };
    let m = resolve_profile("deepseek-chat", Some(&fetched), Some(&manual));
    assert_eq!(m.source, ProfileSource::Manual);
    assert_eq!(m.context_window, 16_000);
    assert_eq!(m.max_output_tokens, 6_000, "max output still from fetched");
    assert_eq!(m.reasoning, Reasoning::None);
    assert!(!m.json_mode);
    assert_eq!(m.thinking_param, ThinkingParam::EnableThinking);

    let tiny = PartialProfile {
        context_window: Some(2_000),
        ..Default::default()
    };
    assert_eq!(
        resolve_profile("x", None, Some(&tiny)).max_output_tokens,
        2_000
    );
}

#[test]
fn available_levels_follow_reasoning_kind() {
    use ThinkingLevel::*;
    let with = |reasoning| ModelProfile {
        reasoning,
        ..Default::default()
    };
    assert!(available_levels(&with(Reasoning::None)).is_empty());
    assert!(available_levels(&with(Reasoning::AlwaysOn)).is_empty());
    assert_eq!(available_levels(&with(Reasoning::Toggle)), vec![Off, High]);
    assert_eq!(
        available_levels(&with(Reasoning::Budget {
            min: 1024,
            max: 32000
        })),
        vec![Off, Low, Medium, High]
    );
    assert_eq!(
        available_levels(&with(Reasoning::Effort(vec![High, Low, Medium, Low]))),
        vec![Low, Medium, High]
    );
}

#[test]
fn provider_never_exposes_its_key() {
    let p = Provider::new(ProviderKind::OpenAiCompatible, "https://x").with_api_key("sk-secret-42");
    let debug = format!("{p:?}");
    assert!(!debug.contains("sk-secret-42"), "{debug}");
    assert!(debug.contains("redacted"));

    let json = serde_json::to_string(&p).unwrap();
    assert!(!json.contains("sk-secret-42"), "{json}");
    let back: Provider = serde_json::from_str(&json).unwrap();
    assert_eq!(back.api_key, None);
    assert_eq!(back.base_url, "https://x");
}
