//! Knowledge base in the app: import, embedding, hybrid search with the
//! reranker, and citations in a fix, against mock model endpoints.

use std::time::Duration;

use app_core::fix::{FixJob, context};
use app_core::secrets::SecretStore;
use app_core::settings::RoleModel;
use app_core::store::ProviderRecord;
use app_core::{Core, knowledge, providers};
use docx_engine::Document;
use docx_engine::testgen::{Spec, generate};
use models::{Client, ClientConfig};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const NOTICE: &str = "关于加强政务数据共享工作的通知
国办发〔2024〕12号
第一章 总则
第一条 为加快推进政务数据共享，提升政府治理能力，制定本通知。
第二条 政务数据共享遵循“以共享为原则、不共享为例外”的要求，各部门应当依托全国一体化政务服务平台开展数据共享。
第二章 保障措施
第三条 各地区应将政务数据共享工作经费纳入财政预算，统计口径以财政部门核定的数据为准。
第四条 建立数据质量考核机制，考核结果纳入年度绩效评价。";

const PLAN: &str = "某市智慧交通三年行动方案
一、总体目标
到2026年，全市主要路口信号灯联网率达到95%。
二、重点任务
（一）建设交通大数据平台，汇聚公安、交通、城管等部门数据。
（二）推广电子停车收费，覆盖中心城区全部道路停车位。";

/// Deterministic 16-dim "embeddings": character counts hashed into buckets,
/// so texts sharing characters are close.
fn vector(text: &str) -> Vec<f32> {
    let mut v = [0.0f32; 16];
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        v[(c as usize) % 16] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter().map(|x| x / norm).collect()
}

struct Embeddings;

impl Respond for Embeddings {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let data: Vec<Value> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"index": i, "embedding": vector(t.as_str().unwrap())}))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({"data": data}))
    }
}

/// Scores each document by the query characters it contains.
struct Rerank;

impl Respond for Rerank {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let query: Vec<char> = body["query"].as_str().unwrap().chars().collect();
        let results: Vec<Value> = body["documents"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let d = d.as_str().unwrap();
                let hits = query.iter().filter(|c| d.contains(**c)).count();
                json!({"index": i, "relevance_score": hits as f64 / query.len().max(1) as f64})
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({"results": results}))
    }
}

fn core(dir: &std::path::Path, server: &MockServer) -> Core {
    let client = Client::with_config(ClientConfig {
        retry_base_delay: Duration::from_millis(1),
        ..ClientConfig::default()
    })
    .unwrap();
    let core = Core::with_parts(
        &dir.join("data"),
        client,
        SecretStore::file_only(&dir.join("data")),
    )
    .unwrap();
    {
        let store = core.store();
        store
            .upsert_provider(&ProviderRecord {
                id: "gw".into(),
                name: "测试网关".into(),
                kind: "openai".into(),
                base_url: server.uri(),
                decision_path: None,
                rerank_path: None,
            })
            .unwrap();
        let mut s = store.settings().unwrap();
        let role = |model: &str| RoleModel {
            provider_id: "gw".into(),
            model: model.into(),
            thinking: String::new(),
        };
        s.roles.chat = role("cline-pass/deepseek-v4.1-flash");
        s.roles.embedding = role("BAAI/bge-m3");
        s.roles.rerank = role("BAAI/bge-reranker-v2-m3");
        store.save_settings(&s).unwrap();
    }
    core.secrets()
        .set(&providers::secret_name("gw"), "sk-test")
        .unwrap();
    core
}

#[tokio::test]
async fn imports_embeds_searches_and_cites() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(Embeddings)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/rerank"))
        .respond_with(Rerank)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);

    let notice = dir.path().join("政务数据共享通知.txt");
    let plan = dir.path().join("智慧交通方案.md");
    std::fs::write(&notice, NOTICE).unwrap();
    std::fs::write(&plan, PLAN).unwrap();
    let missing = dir.path().join("不存在.docx");
    let progress = std::sync::Mutex::new(Vec::new());
    let reports = core.kb_import(&[notice.clone(), plan, missing], |done, total, _| {
        progress.lock().unwrap().push((done, total))
    });
    assert!(
        reports[0].error.is_none() && reports[0].chunks > 0,
        "{reports:?}"
    );
    assert!(reports[1].error.is_none());
    assert!(
        reports[2].error.is_some(),
        "a bad file is reported, not fatal"
    );
    assert_eq!(progress.lock().unwrap().last(), Some(&(3, 3)));
    let docs = core.kb_documents().unwrap();
    assert_eq!(docs.len(), 2);
    let notice_doc = docs.iter().find(|d| d.file_name.contains("通知")).unwrap();
    assert_eq!(
        notice_doc.meta.doc_number.as_deref(),
        Some("国办发〔2024〕12号")
    );

    // Keyword-only until vectors exist.
    let found = core.kb_search("政务数据共享经费", 5).await.unwrap();
    assert!(!found.hits.is_empty());
    assert!(found.warnings.iter().any(|w| w.contains("还没有生成向量")));

    let embedded = core.kb_embed(|_, _| {}).await.unwrap();
    let stats = core.kb_stats().unwrap();
    assert_eq!(embedded, stats.chunks);
    assert_eq!(stats.embedded, stats.chunks);
    assert_eq!(stats.embedding_model.as_deref(), Some("BAAI/bge-m3"));
    assert_eq!(core.kb_embed(|_, _| {}).await.unwrap(), 0, "nothing left");

    let found = core.kb_search("数据共享工作经费预算", 3).await.unwrap();
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    assert!(found.hits.len() <= 3);
    let top = &found.hits[0];
    assert!(top.rerank_score.is_some());
    assert!(top.hit.text.contains("经费"), "{}", top.hit.text);
    assert!(
        found
            .hits
            .windows(2)
            .all(|w| w[0].rerank_score >= w[1].rerank_score)
    );

    let (citations, _) = knowledge::retrieve(&core, "数据共享工作经费", 4)
        .await
        .unwrap();
    assert_eq!(citations[0].n, 1);
    assert!(std::path::Path::new(&citations[0].stored_path).exists());

    // A fix cites the passage the model used.
    let doc = Document::from_bytes(generate(&Spec {
        target_chars: 5_000,
        comments: 5,
        seed: 3,
    }))
    .unwrap();
    let comment_id = doc
        .comments
        .iter()
        .find(|c| c.parent_id.is_none() && c.anchor.is_some())
        .unwrap()
        .id
        .clone();
    let input = context::gather(&doc, &comment_id).unwrap();
    let old = input.paragraphs[0].1.clone();
    let answer = json!({
        "paragraphs": [format!("{old}（经费纳入财政预算）")],
        "explanation": "依据通知第三条补充经费来源",
        "citations": [1]
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": answer.to_string()}, "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;
    let proposal = core
        .propose(
            FixJob {
                doc_key: "d".into(),
                doc_name: "报告.docx".into(),
                input,
            },
            |c| Ok(doc.check_edits(c)?),
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(proposal.citations.len(), 1);
    assert_eq!(proposal.citations[0].n, 1);
    assert!(proposal.context.passages > 0);
    let requests = server.received_requests().await.unwrap();
    let chat: Value = requests
        .iter()
        .rev()
        .find(|r| r.url.path().ends_with("chat/completions"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .unwrap();
    let prompt = chat["messages"][1]["content"].as_str().unwrap();
    assert!(prompt.contains("参考资料"), "{prompt}");

    core.kb_remove(notice_doc.id).unwrap();
    assert_eq!(core.kb_documents().unwrap().len(), 1);
}
