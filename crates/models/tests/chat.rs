use std::time::Duration;

use models::{
    ChatRequest, Client, ClientConfig, Error, Message, ModelProfile, Provider, ProviderKind,
    Reasoning, ThinkingLevel, Usage, resolve_profile,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-secret-123";

fn client() -> Client {
    Client::with_config(ClientConfig {
        retry_base_delay: Duration::from_millis(1),
        ..Default::default()
    })
    .unwrap()
}

fn provider(kind: ProviderKind, server: &MockServer) -> Provider {
    Provider::new(kind, server.uri()).with_api_key(KEY)
}

fn ok_body(content: &str) -> Value {
    json!({
        "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 12, "completion_tokens": 3 }
    })
}

async fn mount_ok(server: &MockServer, route: &str) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("好")))
        .mount(server)
        .await;
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.body_json().unwrap())
        .collect()
}

fn request(model: &str, thinking: Option<ThinkingLevel>) -> ChatRequest {
    let mut req = ChatRequest::new(
        model,
        vec![Message::system("你是助手"), Message::user("你好")],
    );
    req.thinking = thinking;
    req
}

/// Sends one request per level and returns the thinking-related fields of each body.
async fn thinking_fields(
    kind: ProviderKind,
    model: &str,
    levels: &[Option<ThinkingLevel>],
) -> Vec<Value> {
    let server = MockServer::start().await;
    let route = match kind {
        ProviderKind::Anthropic => "/v1/messages",
        _ => "/v1/chat/completions",
    };
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(match kind {
            ProviderKind::Anthropic => json!({ "content": [{ "type": "text", "text": "好" }] }),
            _ => ok_body("好"),
        }))
        .mount(&server)
        .await;
    let p = provider(kind, &server);
    for level in levels {
        client().chat(&p, &request(model, *level)).await.unwrap();
    }
    bodies(&server)
        .await
        .into_iter()
        .map(|b| {
            let pick = [
                "thinking",
                "enable_thinking",
                "reasoning_effort",
                "reasoning",
            ];
            Value::Object(
                pick.iter()
                    .filter_map(|k| Some((k.to_string(), b.get(*k)?.clone())))
                    .collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn maps_thinking_type_style() {
    use ThinkingLevel::*;
    let got = thinking_fields(
        ProviderKind::OpenAiCompatible,
        "cline-pass/deepseek-v4.1-flash",
        &[Some(Off), Some(High), Some(Medium), None],
    )
    .await;
    assert_eq!(
        got,
        vec![
            json!({ "thinking": { "type": "disabled" } }),
            json!({ "thinking": { "type": "enabled" } }),
            json!({ "thinking": { "type": "enabled" } }),
            json!({}),
        ]
    );
}

#[tokio::test]
async fn maps_enable_thinking_style() {
    use ThinkingLevel::*;
    let got = thinking_fields(
        ProviderKind::OpenAiCompatible,
        "qwen3-32b",
        &[Some(Off), Some(Low)],
    )
    .await;
    assert_eq!(
        got,
        vec![
            json!({ "enable_thinking": false }),
            json!({ "enable_thinking": true })
        ]
    );
}

#[tokio::test]
async fn maps_reasoning_effort_style_and_skips_unsupported_levels() {
    use ThinkingLevel::*;
    let got = thinking_fields(
        ProviderKind::OpenAiCompatible,
        "gpt-5-mini",
        &[Some(Medium), Some(Off)],
    )
    .await;
    assert_eq!(
        got,
        vec![json!({ "reasoning_effort": "medium" }), json!({})]
    );

    let got = thinking_fields(ProviderKind::OpenAiCompatible, "gpt-4o", &[Some(High)]).await;
    assert_eq!(got, vec![json!({})], "no reasoning support: send nothing");

    let got = thinking_fields(
        ProviderKind::OpenAiCompatible,
        "deepseek-reasoner",
        &[Some(Off)],
    )
    .await;
    assert_eq!(got, vec![json!({})], "always-on: send nothing");
}

#[tokio::test]
async fn maps_openrouter_reasoning_style() {
    use ThinkingLevel::*;
    let got = thinking_fields(
        ProviderKind::OpenRouter,
        "deepseek/deepseek-chat-v3.1",
        &[Some(Off), Some(High)],
    )
    .await;
    assert_eq!(
        got,
        vec![
            json!({ "reasoning": { "enabled": false } }),
            json!({ "reasoning": { "effort": "high" } }),
        ]
    );
    let got = thinking_fields(ProviderKind::OpenRouter, "openai/o3-mini", &[Some(Low)]).await;
    assert_eq!(got, vec![json!({ "reasoning": { "effort": "low" } })]);
}

#[tokio::test]
async fn maps_anthropic_budget_style_on_openai_gateways() {
    let got = thinking_fields(
        ProviderKind::OpenAiCompatible,
        "claude-sonnet-4-5",
        &[Some(ThinkingLevel::Low)],
    )
    .await;
    assert_eq!(
        got,
        vec![json!({ "thinking": { "type": "enabled", "budget_tokens": 2048 } })]
    );
}

#[tokio::test]
async fn ollama_uses_openai_endpoint_with_reasoning_effort() {
    let got = thinking_fields(
        ProviderKind::Ollama,
        "qwen3:8b",
        &[Some(ThinkingLevel::High)],
    )
    .await;
    assert_eq!(got, vec![json!({ "reasoning_effort": "high" })]);
}

#[tokio::test]
async fn sends_base_fields_and_json_mode() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("{\"a\":1}")))
        .expect(2)
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let mut req = request("gpt-4o", None);
    req.max_tokens = Some(256);
    req.temperature = Some(0.2);
    req.json_output = true;
    let resp = client().chat(&p, &req).await.unwrap();
    assert_eq!(resp.content, "{\"a\":1}");
    assert_eq!(resp.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        resp.usage,
        Some(Usage {
            prompt_tokens: 12,
            completion_tokens: 3
        })
    );

    req.profile.json_mode = false;
    client().chat(&p, &req).await.unwrap();

    let b = bodies(&server).await;
    assert_eq!(
        b[0],
        json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "你是助手" },
                { "role": "user", "content": "你好" }
            ],
            "max_tokens": 256,
            "temperature": 0.2,
            "response_format": { "type": "json_object" }
        })
    );
    assert!(b[1].get("response_format").is_none());
}

#[tokio::test]
async fn retries_once_without_optional_params_when_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(
            json!({ "thinking": { "type": "enabled" } }),
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "Unrecognized request argument supplied: thinking" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    mount_ok(&server, "/v1/chat/completions").await;

    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let mut req = request("deepseek-chat", Some(ThinkingLevel::High));
    req.json_output = true;
    req.temperature = Some(0.5);
    req.max_tokens = Some(100);
    let resp = client().chat(&p, &req).await.unwrap();
    assert_eq!(resp.content, "好");

    let b = bodies(&server).await;
    assert_eq!(b.len(), 2);
    assert!(b[0].get("response_format").is_some());
    for key in ["thinking", "response_format", "temperature"] {
        assert!(b[1].get(key).is_none(), "{key} should be dropped: {}", b[1]);
    }
    assert_eq!(b[1]["max_tokens"], 100);
}

#[tokio::test]
async fn fallback_switches_to_max_completion_tokens_when_asked() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({ "max_tokens": 50 })))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.",
        ))
        .mount(&server)
        .await;
    mount_ok(&server, "/v1/chat/completions").await;

    let mut req = request("o3-mini", Some(ThinkingLevel::High));
    req.max_tokens = Some(50);
    client()
        .chat(&provider(ProviderKind::OpenAiCompatible, &server), &req)
        .await
        .unwrap();
    let b = bodies(&server).await;
    assert_eq!(b[1]["max_completion_tokens"], 50);
    assert!(b[1].get("max_tokens").is_none());
}

#[tokio::test]
async fn unrelated_400_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string("context length exceeded"))
        .expect(1)
        .mount(&server)
        .await;
    let err = client()
        .chat(
            &provider(ProviderKind::OpenAiCompatible, &server),
            &request("deepseek-chat", Some(ThinkingLevel::High)),
        )
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(400));
    assert!(err.to_string().contains("context length exceeded"), "{err}");
}

#[tokio::test]
async fn retries_429_and_5xx_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_ok(&server, "/v1/chat/completions").await;

    let resp = client()
        .chat(
            &provider(ProviderKind::OpenAiCompatible, &server),
            &request("gpt-4o", None),
        )
        .await
        .unwrap();
    assert_eq!(resp.content, "好");
    assert_eq!(bodies(&server).await.len(), 3);
}

#[tokio::test]
async fn gives_up_after_max_attempts_or_long_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .expect(3)
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let err = client()
        .chat(&p, &request("gpt-4o", None))
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(429));
    assert!(err.to_string().contains("请求过多（429）"), "{err}");
    server.verify().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .expect(1)
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    assert!(client().chat(&p, &request("gpt-4o", None)).await.is_err());
}

#[tokio::test]
async fn splits_think_tags_and_reasoning_fields() {
    let server = MockServer::start().await;
    let responses = [
        json!({ "choices": [{ "message": { "content": "<think>\n先分析。\n</think>\n\n结论：通过" } }] }),
        json!({ "choices": [{ "message": { "content": "结论", "reasoning_content": "推理过程" } }] }),
        json!({ "choices": [{ "message": { "content": "结论", "reasoning": "OpenRouter 推理" } }] }),
        json!({ "choices": [{ "message": { "content": "只有闭合标签</think>答案" } }] }),
        json!({ "choices": [{ "message": { "content": [{ "type": "text", "text": "分段" }, { "type": "text", "text": "内容" }] } }] }),
    ];
    for r in &responses {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(r))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let mut got = Vec::new();
    for _ in &responses {
        let r = client().chat(&p, &request("gpt-4o", None)).await.unwrap();
        got.push((r.content, r.reasoning));
    }
    let s = |x: &str| x.to_string();
    assert_eq!(
        got,
        vec![
            (s("结论：通过"), Some(s("先分析。"))),
            (s("结论"), Some(s("推理过程"))),
            (s("结论"), Some(s("OpenRouter 推理"))),
            (s("答案"), Some(s("只有闭合标签"))),
            (s("分段内容"), None),
        ]
    );
}

#[tokio::test]
async fn anthropic_request_and_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                { "type": "thinking", "thinking": "想一想", "signature": "sig" },
                { "type": "text", "text": "你好！" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 20, "output_tokens": 7 }
        })))
        .expect(2)
        .mount(&server)
        .await;
    let p = provider(ProviderKind::Anthropic, &server);

    let mut req = request("claude-sonnet-4-5", Some(ThinkingLevel::High));
    req.max_tokens = Some(8_000);
    req.temperature = Some(0.3);
    let resp = client().chat(&p, &req).await.unwrap();
    assert_eq!(resp.content, "你好！");
    assert_eq!(resp.reasoning.as_deref(), Some("想一想"));
    assert_eq!(resp.finish_reason.as_deref(), Some("end_turn"));
    assert_eq!(resp.usage.unwrap().completion_tokens, 7);

    let mut plain = request("claude-sonnet-4-5", None);
    plain.temperature = Some(0.3);
    client().chat(&p, &plain).await.unwrap();

    let b = bodies(&server).await;
    assert_eq!(
        b[0],
        json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 8000,
            "system": "你是助手",
            "messages": [{ "role": "user", "content": "你好" }],
            "thinking": { "type": "enabled", "budget_tokens": 7999 }
        })
    );
    assert_eq!(
        b[1]["max_tokens"], 64_000,
        "defaults to the profile's output limit"
    );
    assert_eq!(b[1]["temperature"], 0.3);
    assert!(b[1].get("thinking").is_none());
}

#[tokio::test]
async fn anthropic_budget_follows_level_and_profile_range() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "content": [] })))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::Anthropic, &server);
    let mut req = request("claude-opus-4-1", Some(ThinkingLevel::Medium));
    client().chat(&p, &req).await.unwrap();
    req.thinking = Some(ThinkingLevel::High);
    req.profile = ModelProfile {
        reasoning: Reasoning::Budget {
            min: 1024,
            max: 10_000,
        },
        ..resolve_profile("claude-opus-4-1", None, None)
    };
    client().chat(&p, &req).await.unwrap();
    req.max_tokens = Some(1_000);
    client().chat(&p, &req).await.unwrap();

    let b = bodies(&server).await;
    assert_eq!(b[0]["thinking"]["budget_tokens"], 8192);
    assert_eq!(b[1]["thinking"]["budget_tokens"], 10_000);
    assert!(
        b[2].get("thinking").is_none(),
        "no room for the 1024 minimum"
    );
}

#[tokio::test]
async fn auth_error_is_readable_and_never_leaks_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": format!("Incorrect API key provided: {KEY}") }
        })))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let err = client()
        .chat(&p, &request("gpt-4o", None))
        .await
        .unwrap_err();
    let (display, debug) = (err.to_string(), format!("{err:?}"));
    assert!(matches!(err, Error::Http { status: 401, .. }));
    assert!(
        display.contains("认证失败（401），请检查 API Key"),
        "{display}"
    );
    assert!(display.contains("POST /v1/chat/completions"), "{display}");
    assert!(
        display.contains("Incorrect API key provided: ***"),
        "{display}"
    );
    assert!(!display.contains(KEY) && !debug.contains(KEY));
    assert!(!format!("{p:?}").contains(KEY));
}

#[tokio::test]
async fn error_bodies_are_truncated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(404).set_body_string("模".repeat(2_000)))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let err = client().chat(&p, &request("nope", None)).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.starts_with("模型不存在（404）"), "{msg}");
    assert!(msg.chars().count() < 600, "{}", msg.chars().count());
}

#[tokio::test]
async fn network_errors_name_the_endpoint() {
    let p = Provider::new(ProviderKind::OpenAiCompatible, "http://127.0.0.1:1").with_api_key(KEY);
    let err = client()
        .chat(&p, &request("gpt-4o", None))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains("POST /v1/chat/completions") && !msg.contains(KEY),
        "{msg}"
    );
}

#[tokio::test]
async fn web_search_parameters_and_sources() {
    use models::WebSearch;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "[]",
                "annotations": [{ "type": "url_citation",
                    "url_citation": { "url": "https://www.gov.cn/a.html", "title": "通知", "content": "摘要" } }] } }],
            "search_info": { "search_results": [{ "url": "https://www.shanghai.gov.cn/b.html", "title": "上海" }] },
            "web_search": [{ "link": "https://flk.npc.gov.cn/c", "title": "法规" }, { "link": "javascript:x" }]
        })))
        .mount(&server)
        .await;
    let c = client();
    let p = provider(ProviderKind::OpenAiCompatible, &server);
    let mut sent = Vec::new();
    for kind in [
        WebSearch::OpenRouter,
        WebSearch::DashScope,
        WebSearch::Zhipu,
        WebSearch::OpenAi,
    ] {
        let mut req = request("m", None);
        req.web_search = Some(kind);
        let r = c.chat(&p, &req).await.unwrap();
        let urls: Vec<&str> = r.citations.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://www.gov.cn/a.html",
                "https://www.shanghai.gov.cn/b.html",
                "https://flk.npc.gov.cn/c"
            ]
        );
        assert_eq!(r.citations[0].snippet, "摘要");
    }
    for b in bodies(&server).await {
        sent.push(b);
    }
    assert_eq!(sent[0]["plugins"][0]["id"], "web");
    assert_eq!(sent[1]["enable_search"], true);
    assert_eq!(sent[2]["tools"][0]["type"], "web_search");
    assert!(sent[3]["web_search_options"].is_object());

    let anthropic = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({ "tools": [{ "name": "web_search" }] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                { "type": "server_tool_use", "id": "1", "name": "web_search", "input": {} },
                { "type": "web_search_tool_result", "content": [{ "type": "web_search_result", "url": "https://www.mee.gov.cn/x", "title": "标准" }] },
                { "type": "text", "text": "[]", "citations": [{ "url": "https://www.mee.gov.cn/x", "title": "标准", "cited_text": "..." }] }
            ],
            "stop_reason": "end_turn"
        })))
        .mount(&anthropic)
        .await;
    let mut req = request("claude", None);
    req.web_search = Some(WebSearch::Anthropic);
    let r = c
        .chat(&provider(ProviderKind::Anthropic, &anthropic), &req)
        .await
        .unwrap();
    assert_eq!(r.content, "[]");
    assert_eq!(r.citations.len(), 1);

    let detect = |kind, url: &str| WebSearch::detect(&Provider::new(kind, url));
    assert_eq!(
        detect(
            ProviderKind::OpenAiCompatible,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ),
        Some(WebSearch::DashScope)
    );
    assert_eq!(
        detect(
            ProviderKind::OpenAiCompatible,
            "https://open.bigmodel.cn/api/paas/v4"
        ),
        Some(WebSearch::Zhipu)
    );
    assert_eq!(
        detect(ProviderKind::OpenAiCompatible, "https://api.deepseek.com"),
        None
    );
    assert_eq!(detect(ProviderKind::Ollama, ""), None);
}
