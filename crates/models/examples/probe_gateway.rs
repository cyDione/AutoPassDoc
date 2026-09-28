//! Smoke test against a real OpenAI-compatible gateway serving all four roles.
//!
//! ```sh
//! APD_TEST_BASE_URL=https://gateway.example.com APD_TEST_API_KEY=... \
//!     cargo run -p models --example probe_gateway
//! ```
//!
//! Optional: `APD_TEST_CHAT_MODEL`, `APD_TEST_DECISION_MODEL`,
//! `APD_TEST_EMBED_MODEL`, `APD_TEST_RERANK_MODEL`.

use std::env;
use std::time::Instant;

use models::{
    ChatRequest, Client, Message, ModelRole, Provider, ProviderKind, Question, ThinkingLevel,
    available_levels, resolve_profile,
};

fn var(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn short(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

#[tokio::main]
async fn main() {
    let Ok(base_url) = env::var("APD_TEST_BASE_URL") else {
        eprintln!("请设置 APD_TEST_BASE_URL（以及 APD_TEST_API_KEY）");
        std::process::exit(2);
    };
    let mut provider = Provider::new(ProviderKind::OpenAiCompatible, base_url);
    provider.api_key = env::var("APD_TEST_API_KEY").ok();
    let chat_model = var("APD_TEST_CHAT_MODEL", "cline-pass/deepseek-v4.1-flash");
    let decision_model = var("APD_TEST_DECISION_MODEL", "jev-latest");
    let embed_model = var("APD_TEST_EMBED_MODEL", "BAAI/bge-m3");
    let rerank_model = var("APD_TEST_RERANK_MODEL", "BAAI/bge-reranker-v2-m3");
    let client = Client::new().expect("client");

    println!("== list_models");
    let listed = match client.list_models(&provider).await {
        Ok(models) => {
            println!("{} models", models.len());
            for m in models.iter().take(30) {
                println!(
                    "  {:<45} {:?} ctx={:?}",
                    m.id, m.role_hint, m.context_window
                );
            }
            models
        }
        Err(e) => {
            println!("  error: {e}");
            Vec::new()
        }
    };

    for (role, model) in [
        (ModelRole::Chat, &chat_model),
        (ModelRole::Decision, &decision_model),
        (ModelRole::Embedding, &embed_model),
        (ModelRole::Rerank, &rerank_model),
    ] {
        match client.probe(&provider, role, model).await {
            Ok(r) => println!(
                "== probe {role:?} {model}: {} ms, {}",
                r.latency_ms, r.summary
            ),
            Err(e) => println!("== probe {role:?} {model}: error: {e}"),
        }
    }

    let fetched = listed.iter().find(|m| m.id == chat_model);
    let profile = resolve_profile(&chat_model, fetched, None);
    println!(
        "== chat profile: ctx={} out={} reasoning={:?} param={:?} levels={:?} source={:?}",
        profile.context_window,
        profile.max_output_tokens,
        profile.reasoning,
        profile.thinking_param,
        available_levels(&profile),
        profile.source
    );
    for level in [ThinkingLevel::Off, ThinkingLevel::High] {
        let mut req = ChatRequest::new(
            &chat_model,
            vec![
                Message::system("你是公文写作助手，回答简洁。"),
                Message::user("用一句话说明“数据来源”批注应该怎么改。"),
            ],
        );
        req.profile = profile.clone();
        req.thinking = Some(level);
        req.max_tokens = Some(1024);
        let start = Instant::now();
        match client.chat(&provider, &req).await {
            Ok(r) => println!(
                "== chat thinking={level:?}: {} ms, finish={:?}, usage={:?}\n   content: {}\n   reasoning: {} chars",
                start.elapsed().as_millis(),
                r.finish_reason,
                r.usage,
                short(&r.content, 120),
                r.reasoning.map_or(0, |s| s.chars().count()),
            ),
            Err(e) => println!("== chat thinking={level:?}: error: {e}"),
        }
    }

    let texts: Vec<String> = [
        "数据来源应标注到具体年份",
        "本报告数据来自统计年鉴",
        "汽车需要定期保养",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    match client.embed(&provider, &embed_model, &texts).await {
        Ok(v) => println!("== embed: {} vectors x {} dims", v.len(), v[0].len()),
        Err(e) => println!("== embed: error: {e}"),
    }
    match client
        .rerank(&provider, &rerank_model, "数据来源", &texts)
        .await
    {
        Ok(s) => println!("== rerank: {s:?}"),
        Err(e) => println!("== rerank: error: {e}"),
    }

    let questions = vec![
        Question::yes_no("addressed", "修改后的文字是否回应了批注？"),
        Question::choice(
            "action",
            "这条批注应如何处理？",
            [
                ("revise", "修改原文"),
                ("explain", "补充说明"),
                ("ignore", "无需处理"),
            ],
        ),
        Question::score(
            "quality",
            "修改质量如何？",
            ["很差", "较差", "一般", "较好", "很好"],
        ),
    ];
    let state = "批注：请补充数据来源。\n原文：全市GDP增长5%。\n修改：据市统计局《2024年统计公报》，全市GDP增长5%。";
    match client
        .decide(&provider, &decision_model, state, &questions)
        .await
    {
        Ok(answers) => {
            println!("== decide (Jev)");
            for a in answers {
                println!("   {}: {:?}", a.key, a.value);
            }
        }
        Err(e) => println!("== decide (Jev): error: {e}"),
    }
    match client
        .decide_via_chat(&provider, &chat_model, &profile, state, &questions)
        .await
    {
        Ok(answers) => {
            println!("== decide_via_chat");
            for a in answers {
                println!("   {}: {:?}", a.key, a.value);
            }
        }
        Err(e) => println!("== decide_via_chat: error: {e}"),
    }
}
