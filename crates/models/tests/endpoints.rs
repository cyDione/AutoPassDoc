use std::time::Duration;

use models::{
    Client, ClientConfig, Error, ModelRole, Provider, ProviderKind, Reasoning, ThinkingLevel,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "sk-endpoint-test";

fn client_with_batch(embed_batch_size: usize) -> Client {
    Client::with_config(ClientConfig {
        retry_base_delay: Duration::from_millis(1),
        embed_batch_size,
        ..Default::default()
    })
    .unwrap()
}

fn client() -> Client {
    client_with_batch(32)
}

fn provider(kind: ProviderKind, base: String) -> Provider {
    Provider::new(kind, base).with_api_key(KEY)
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

#[tokio::test]
async fn lists_openai_compatible_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [
                { "id": "cline-pass/deepseek-v4.1-flash", "object": "model", "context_length": 131072 },
                { "id": "Qwen/Qwen3-8B", "max_model_len": 40960 },
                { "id": "jev-latest", "context_window": "8192" },
                { "id": "BAAI/bge-m3", "max_context_length": 8192 },
                { "id": "BAAI/bge-reranker-v2-m3" },
                { "object": "model" }
            ]
        })))
        .mount(&server)
        .await;
    let models = client()
        .list_models(&provider(ProviderKind::OpenAiCompatible, server.uri()))
        .await
        .unwrap();
    let got: Vec<_> = models
        .iter()
        .map(|m| (m.id.as_str(), m.context_window, m.role_hint))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "cline-pass/deepseek-v4.1-flash",
                Some(131_072),
                ModelRole::Chat
            ),
            ("Qwen/Qwen3-8B", Some(40_960), ModelRole::Chat),
            ("jev-latest", Some(8_192), ModelRole::Decision),
            ("BAAI/bge-m3", Some(8_192), ModelRole::Embedding),
            ("BAAI/bge-reranker-v2-m3", None, ModelRole::Rerank),
        ]
    );
    assert!(models.iter().all(|m| m.reasoning.is_none()));
}

#[tokio::test]
async fn lists_openrouter_models_with_limits_and_reasoning() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {
                    "id": "deepseek/deepseek-chat-v3.1",
                    "name": "DeepSeek: DeepSeek V3.1",
                    "context_length": 163840,
                    "top_provider": { "context_length": 163840, "max_completion_tokens": 32768 },
                    "supported_parameters": ["max_tokens", "temperature", "reasoning", "include_reasoning"]
                },
                {
                    "id": "openai/gpt-4o",
                    "name": "OpenAI: GPT-4o",
                    "context_length": 128000,
                    "top_provider": { "max_completion_tokens": 16384 },
                    "supported_parameters": ["max_tokens", "response_format"]
                }
            ]
        })))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenRouter, format!("{}/api/v1", server.uri()));
    let models = client().list_models(&p).await.unwrap();
    assert_eq!(models.len(), 2);
    let ds = &models[0];
    assert_eq!(ds.display_name.as_deref(), Some("DeepSeek: DeepSeek V3.1"));
    assert_eq!(
        (ds.context_window, ds.max_output_tokens),
        (Some(163_840), Some(32_768))
    );
    use ThinkingLevel::*;
    assert_eq!(
        ds.reasoning,
        Some(Reasoning::Effort(vec![Off, Low, Medium, High]))
    );
    assert_eq!(models[1].reasoning, Some(Reasoning::None));
    assert_eq!(models[1].max_output_tokens, Some(16_384));
}

#[tokio::test]
async fn lists_anthropic_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "claude-sonnet-4-5-20250929", "display_name": "Claude Sonnet 4.5", "type": "model" }],
            "has_more": false
        })))
        .mount(&server)
        .await;
    let models = client()
        .list_models(&provider(ProviderKind::Anthropic, server.uri()))
        .await
        .unwrap();
    assert_eq!(models[0].id, "claude-sonnet-4-5-20250929");
    assert_eq!(models[0].display_name.as_deref(), Some("Claude Sonnet 4.5"));
}

#[tokio::test]
async fn lists_ollama_models_and_tolerates_show_failures() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [
                { "name": "qwen3:8b", "model": "qwen3:8b", "size": 5_000_000_000u64 },
                { "name": "bge-m3:latest" },
                { "name": "broken:1b" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .and(body_partial_json(json!({ "model": "qwen3:8b" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_info": { "general.architecture": "qwen3", "qwen3.context_length": 40960, "qwen3.block_count": 36 },
            "capabilities": ["completion", "tools", "thinking"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .and(body_partial_json(json!({ "model": "bge-m3:latest" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model_info": { "bert.context_length": 8192 },
            "capabilities": ["embedding"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(404).set_body_string("model not found"))
        .mount(&server)
        .await;

    let p = Provider::new(ProviderKind::Ollama, format!("{}/v1", server.uri()));
    let models = client().list_models(&p).await.unwrap();
    let got: Vec<_> = models
        .iter()
        .map(|m| {
            (
                m.id.as_str(),
                m.context_window,
                m.role_hint,
                m.reasoning.clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "qwen3:8b",
                Some(40_960),
                ModelRole::Chat,
                Some(Reasoning::Toggle)
            ),
            ("bge-m3:latest", Some(8_192), ModelRole::Embedding, None),
            ("broken:1b", None, ModelRole::Chat, None),
        ]
    );
    let reqs = server.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .all(|r| !r.headers.contains_key("authorization"))
    );
}

/// Echoes `[n, len]` for each input `"tn"`, returning rows in reverse order.
fn embed_responder(req: &Request) -> ResponseTemplate {
    let body: Value = req.body_json().unwrap();
    let inputs = body["input"].as_array().unwrap();
    let mut data: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let n: f32 = t.as_str().unwrap()[1..].parse().unwrap();
            json!({ "object": "embedding", "index": i, "embedding": [n, 0.5] })
        })
        .collect();
    data.reverse();
    ResponseTemplate::new(200).set_body_json(json!({ "data": data, "model": body["model"] }))
}

#[tokio::test]
async fn embeds_in_batches_and_restores_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(embed_responder)
        .expect(3)
        .mount(&server)
        .await;
    let texts: Vec<String> = (0..5).map(|i| format!("t{i}")).collect();
    let p = provider(ProviderKind::OpenAiCompatible, server.uri());
    let vectors = client_with_batch(2)
        .embed(&p, "BAAI/bge-m3", &texts)
        .await
        .unwrap();
    assert_eq!(
        vectors,
        (0..5).map(|i| vec![i as f32, 0.5]).collect::<Vec<_>>()
    );

    let b = bodies(&server).await;
    assert_eq!(
        b[0],
        json!({ "model": "BAAI/bge-m3", "input": ["t0", "t1"], "encoding_format": "float" })
    );
    assert_eq!(b[2]["input"], json!(["t4"]));

    assert!(client().embed(&p, "m", &[]).await.unwrap().is_empty());
    assert_eq!(
        bodies(&server).await.len(),
        3,
        "empty input makes no request"
    );
}

#[tokio::test]
async fn embed_rejects_bad_counts_and_dimensions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .and(body_partial_json(json!({ "model": "short" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "index": 0, "embedding": [0.1] }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "index": 0, "embedding": [0.1, 0.2] }, { "index": 1, "embedding": [0.3] }]
        })))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, server.uri());
    let texts = vec!["a".to_string(), "b".to_string()];
    let err = client().embed(&p, "short", &texts).await.unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
    assert!(err.to_string().contains("POST /v1/embeddings"), "{err}");
    let err = client().embed(&p, "ragged", &texts).await.unwrap_err();
    assert!(err.to_string().contains("维度"), "{err}");
}

#[tokio::test]
async fn embed_and_rerank_are_rejected_for_anthropic() {
    let p = Provider::new(ProviderKind::Anthropic, "");
    let texts = vec!["a".to_string()];
    assert!(matches!(
        client().embed(&p, "m", &texts).await,
        Err(Error::InvalidConfig(_))
    ));
    assert!(matches!(
        client().rerank(&p, "m", "q", &texts).await,
        Err(Error::InvalidConfig(_))
    ));
}

#[tokio::test]
async fn rerank_accepts_common_response_shapes() {
    let docs: Vec<String> = ["苹果是水果", "汽车要加油", "香蕉也是水果"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let shapes = [
        json!({ "results": [
            { "index": 2, "relevance_score": 0.8 },
            { "index": 0, "relevance_score": 0.9 }
        ] }),
        json!({ "data": [
            { "index": 1, "score": 0.1 }, { "index": 0, "score": 0.7 }, { "index": 2, "score": 0.6 }
        ] }),
        json!([{ "index": 0, "score": 0.5 }, { "index": 1, "score": -2.0 }, { "index": 2, "score": 0.4 }]),
    ];
    let expected = [
        vec![0.9, f32::NEG_INFINITY, 0.8],
        vec![0.7, 0.1, 0.6],
        vec![0.5, -2.0, 0.4],
    ];
    for (shape, want) in shapes.iter().zip(expected) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/rerank"))
            .respond_with(ResponseTemplate::new(200).set_body_json(shape))
            .mount(&server)
            .await;
        let p = provider(ProviderKind::OpenAiCompatible, server.uri());
        let scores = client()
            .rerank(&p, "BAAI/bge-reranker-v2-m3", "水果", &docs)
            .await
            .unwrap();
        assert_eq!(scores, want);
        assert_eq!(
            bodies(&server).await[0],
            json!({
                "model": "BAAI/bge-reranker-v2-m3",
                "query": "水果",
                "documents": docs,
                "top_n": 3,
                "return_documents": false
            })
        );
    }
}

#[tokio::test]
async fn rerank_honours_custom_path_and_skips_empty_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/rerank"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "index": 0, "relevance_score": 0.3 }]
        })))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, server.uri())
        .with_rerank_path(format!("{}/v2/rerank", server.uri()));
    let scores = client()
        .rerank(&p, "rerank-v3.5", "q", &["d".to_string()])
        .await
        .unwrap();
    assert_eq!(scores, vec![0.3]);
    assert!(client().rerank(&p, "m", "q", &[]).await.unwrap().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn probes_each_role() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "index": 0, "embedding": vec![0.0; 1024] }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/rerank"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "index": 0, "relevance_score": 0.95 }, { "index": 1, "relevance_score": 0.01 }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "好" }, "finish_reason": "stop" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "answers": { "probe": { "type": "noul", "noul": 0.99 } }
        })))
        .mount(&server)
        .await;
    let p = provider(ProviderKind::OpenAiCompatible, server.uri());
    let c = client();
    let summary = |role, model: &'static str| {
        let (c, p) = (c.clone(), p.clone());
        async move { c.probe(&p, role, model).await.unwrap().summary }
    };
    assert_eq!(
        summary(ModelRole::Embedding, "BAAI/bge-m3").await,
        "向量维度 1024"
    );
    assert_eq!(
        summary(ModelRole::Rerank, "BAAI/bge-reranker-v2-m3").await,
        "得分 [0.950, 0.010]"
    );
    assert_eq!(
        summary(ModelRole::Chat, "cline-pass/deepseek-v4.1-flash").await,
        "回复：好"
    );
    assert_eq!(
        summary(ModelRole::Decision, "jev-latest").await,
        "P(是) = 0.990"
    );

    let chat_body = bodies(&server)
        .await
        .into_iter()
        .find(|b| b.get("messages").is_some())
        .unwrap();
    assert_eq!(chat_body["thinking"], json!({ "type": "disabled" }));
}
