//! Web search for "【待补充…】" gaps: the chat model's own search, the
//! whitelist search fallback and downloads into the knowledge base.

use std::time::Duration;

use app_core::secrets::SecretStore;
use app_core::settings::RoleModel;
use app_core::store::ProviderRecord;
use app_core::web::{ResultKind, WebMode};
use app_core::{Core, providers};
use models::{Client, ClientConfig, WebSearch};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn core(dir: &std::path::Path, server: &MockServer, mode: WebMode) -> Core {
    let client = Client::with_config(ClientConfig {
        retry_base_delay: Duration::from_millis(1),
        ..ClientConfig::default()
    })
    .unwrap();
    let core = Core::with_parts(dir, client, SecretStore::file_only(dir)).unwrap();
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
        s.roles.chat = RoleModel {
            provider_id: "gw".into(),
            model: "qwen-plus".into(),
            thinking: String::new(),
        };
        s.web.mode = mode;
        s.web.model_search = Some(WebSearch::DashScope);
        s.web.whitelist = vec!["127.0.0.1".into()];
        store.save_settings(&s).unwrap();
    }
    core.secrets()
        .set(&providers::secret_name("gw"), "sk-test")
        .unwrap();
    core.set_search_bases(&server.uri(), &server.uri());
    core
}

#[tokio::test]
async fn model_search_keeps_only_links_the_provider_reported() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server, WebMode::Model);
    let real = format!("{}/tjgb/2024.html", server.uri());
    let answer = json!([
        {"title": "2024年统计公报", "url": real, "snippet": "常住人口 2480 万"},
        {"title": "编造的", "url": "https://www.gov.cn/made-up.html", "snippet": ""}
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_partial_json(
            json!({"enable_search": true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": answer.to_string()}}],
            "search_info": {"search_results": [{"url": real, "title": "公报"}]}
        })))
        .mount(&server)
        .await;
    let out = core.web_search("崇明区 2024 年 常住人口").await.unwrap();
    assert_eq!(out.via, "model");
    assert_eq!(out.results.len(), 1, "{:?}", out.results);
    assert_eq!(out.results[0].title, "2024年统计公报");
    assert_eq!(out.results[0].snippet, "常住人口 2480 万");
}

#[tokio::test]
async fn falls_back_to_the_whitelist_and_downloads_attachments() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server, WebMode::Auto);
    let base = server.uri();
    // The model's service fails, so the whitelist search runs.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("down"))
        .mount(&server)
        .await;
    let results = format!(
        r#"<ol><li class="b_algo"><h2><a href="{base}/zw/notice.html">关于印发<strong>办法</strong>的通知</a></h2><p>自发布之日起施行</p></li>
        <li class="b_algo"><h2><a href="https://elsewhere.example.com/x">不在白名单</a></h2></li></ol>"#
    );
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("ensearch", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(results, "text/html; charset=utf-8"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private/\n"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/zw/notice.html"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<p>附件：<a href="files/%E5%8A%9E%E6%B3%95.md">管理办法全文</a> <a href="/private/x.pdf">内部</a></p>"#,
            "text/html",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/zw/files/%E5%8A%9E%E6%B3%95.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "# 政务信息化项目管理办法\n\n第一条 为规范政务信息化项目管理，制定本办法。\n",
        ))
        .mount(&server)
        .await;

    let out = core.web_search("政务信息化项目管理办法").await.unwrap();
    assert_eq!(out.via, "local");
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("大语言模型联网搜索失败")),
        "{:?}",
        out.notes
    );
    assert_eq!(out.results.len(), 3, "{:?}", out.results);
    assert_eq!(out.results[0].title, "关于印发办法的通知");
    let file = out
        .results
        .iter()
        .find(|r| r.kind == ResultKind::File && r.importable)
        .unwrap();
    assert_eq!(file.title, "管理办法全文");
    // robots.txt does not stop listing a link, only fetching pages under it.
    assert!(
        out.results
            .iter()
            .any(|r| r.url.ends_with("/private/x.pdf"))
    );

    assert!(
        core.web_download_to_kb("https://www.gov.cn/unseen.pdf")
            .await
            .is_err()
    );
    let report = core.web_download_to_kb(&file.url).await.unwrap();
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.file_name, "办法.md");
    let docs = core.kb_documents().unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].title, "政务信息化项目管理办法");
}

#[tokio::test]
async fn off_mode_refuses() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server, WebMode::Off);
    assert!(core.web_search("任何内容").await.is_err());
}
