//! AI fixes that look up public material: before the first draft when the
//! comment or direction names it, and again for gaps the draft left.

use std::sync::Mutex;
use std::time::Duration;

use app_core::fix::context::{self, FixRequest, FixSource};
use app_core::fix::{FixJob, Proposal, Stage};
use app_core::secrets::SecretStore;
use app_core::settings::RoleModel;
use app_core::store::ProviderRecord;
use app_core::web::WebMode;
use app_core::{Core, providers};
use docx_engine::Document;
use docx_engine::testgen::{Spec, generate};
use models::{Client, ClientConfig};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn core(dir: &std::path::Path, server: &MockServer) -> Core {
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
            model: "deepseek-chat".into(),
            thinking: "off".into(),
        };
        s.web.mode = WebMode::Local;
        s.web.whitelist = vec!["127.0.0.1".into()];
        store.save_settings(&s).unwrap();
    }
    core.secrets()
        .set(&providers::secret_name("gw"), "sk-test")
        .unwrap();
    core.set_search_bases(&server.uri(), &server.uri());
    core
}

fn doc() -> Document {
    Document::from_bytes(generate(&Spec {
        target_chars: 20_000,
        comments: 20,
        seed: 9,
    }))
    .unwrap()
}

/// An open top-level comment on a single paragraph without revisions.
fn pick(doc: &Document) -> String {
    doc.comments
        .iter()
        .find(|c| {
            c.parent_id.is_none()
                && !c.done
                && c.anchor.as_ref().is_some_and(|a| {
                    a.start_paragraph == a.end_paragraph
                        && doc.paragraphs[a.start_paragraph]
                            .runs
                            .iter()
                            .all(|r| r.revision == docx_engine::Revision::None)
                })
        })
        .unwrap()
        .id
        .clone()
}

fn reply(content: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": content.to_string()}, "finish_reason": "stop"}]
    }))
}

const TERMS: &str = "搜索引擎查找这项资料的关键词";
const DRAFT: &str = "需要修改的段落";
const PAGE_FACT: &str = "到2030年，美丽上海建设取得显著成效";

/// A search engine page listing one whitelisted page, and that page.
async fn mount_page(server: &MockServer) {
    let base = server.uri();
    let results = format!(
        r#"<ol><li class="b_algo"><h2><a href="{base}/plan.html">关于印发美丽上海建设“十五五”规划的通知</a></h2><p>沪府发</p></li></ol>"#
    );
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(results, "text/html; charset=utf-8"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/plan.html"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!(
                "<html><body><nav>首页</nav><h1>美丽上海建设“十五五”规划</h1><p>{PAGE_FACT}，生态环境质量持续改善。</p><p>附件下载</p></body></html>"
            ),
            "text/html; charset=utf-8",
        ))
        .mount(server)
        .await;
}

async fn propose(
    core: &Core,
    doc: &Document,
    comment_id: &str,
    request: &FixRequest,
) -> (Proposal, Vec<Stage>) {
    let stages = Mutex::new(Vec::new());
    let job = FixJob {
        doc_key: "doc".into(),
        doc_name: "报告.docx".into(),
        input: context::gather_request(doc, comment_id, request).unwrap(),
    };
    let proposal = core
        .propose(
            job,
            |c| Ok(doc.check_edits(c)?),
            |s| stages.lock().unwrap().push(s),
        )
        .await
        .unwrap();
    (proposal, stages.into_inner().unwrap())
}

#[tokio::test]
async fn a_direction_naming_a_plan_is_looked_up_before_the_draft() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    let doc = doc();
    let comment_id = pick(&doc);
    let old = context::gather(&doc, &comment_id).unwrap().paragraphs[0]
        .1
        .clone();
    mount_page(&server).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(TERMS))
        .and(body_string_contains("美丽上海 十五五"))
        .respond_with(reply(json!({"queries": ["美丽上海 十五五 规划"]})))
        .mount(&server)
        .await;
    let new = format!("对照美丽上海建设“十五五”规划，{old}");
    // The page's text is in the prompt as a numbered source.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(DRAFT))
        .and(body_string_contains(PAGE_FACT))
        .and(body_string_contains("（网页："))
        .respond_with(reply(
            json!({"paragraphs": [new], "explanation": "加入规划简述", "citations": [1]}),
        ))
        .mount(&server)
        .await;

    let request = FixRequest {
        direction: Some("加入“美丽上海 十五五”的简述".into()),
        ..FixRequest::default()
    };
    let (proposal, stages) = propose(&core, &doc, &comment_id, &request).await;
    assert_eq!(
        stages,
        [
            Stage::Context,
            Stage::Retrieve,
            Stage::Web,
            Stage::Generate,
            Stage::Judge
        ]
    );
    assert_eq!(proposal.paragraphs[0].new, new);
    assert_eq!(proposal.citations.len(), 1, "{:?}", proposal.warnings);
    let cite = &proposal.citations[0];
    assert_eq!(
        cite.url.as_deref(),
        Some(format!("{}/plan.html", server.uri()).as_str())
    );
    assert!(cite.text.contains(PAGE_FACT));
}

#[tokio::test]
async fn public_gaps_left_in_the_draft_are_looked_up_and_rewritten() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    let doc = doc();
    let comment_id = pick(&doc);
    let old = context::gather(&doc, &comment_id).unwrap().paragraphs[0]
        .1
        .clone();
    mount_page(&server).await;
    // Terms for anything but the plan: the project's own data, not searched.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(TERMS))
        .and(body_string_contains("美丽上海“十五五”规划目标"))
        .respond_with(reply(json!({"queries": ["美丽上海 十五五 规划"]})))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(TERMS))
        .respond_with(reply(json!({"queries": []})))
        .mount(&server)
        .await;
    let filled =
        format!("{old}本项目衔接美丽上海建设“十五五”规划。【待补充：需编制单位提供测算方法】");
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(DRAFT))
        .and(body_string_contains(PAGE_FACT))
        .respond_with(reply(
            json!({"paragraphs": [filled], "explanation": "补充规划衔接", "citations": [1]}),
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    let draft = format!(
        "{old}本项目衔接【待补充：美丽上海“十五五”规划目标】。【待补充：需编制单位提供测算方法】"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(DRAFT))
        .respond_with(reply(
            json!({"paragraphs": [draft], "explanation": "初稿", "citations": []}),
        ))
        .mount(&server)
        .await;

    let (proposal, stages) = propose(&core, &doc, &comment_id, &FixRequest::default()).await;
    let web_after_draft = stages
        .windows(3)
        .any(|w| w == [Stage::Generate, Stage::Web, Stage::Generate]);
    assert!(web_after_draft, "{stages:?}");
    assert_eq!(proposal.paragraphs[0].new, filled);
    assert_eq!(proposal.explanation, "补充规划衔接");
    assert_eq!(proposal.citations.len(), 1);
    assert!(proposal.citations[0].url.is_some());
}

#[tokio::test]
async fn material_the_user_found_goes_into_the_next_fix() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    {
        let store = core.store();
        let mut s = store.settings().unwrap();
        s.fix.use_web = false;
        store.save_settings(&s).unwrap();
    }
    let doc = doc();
    let comment_id = pick(&doc);
    let old = context::gather(&doc, &comment_id).unwrap().paragraphs[0]
        .1
        .clone();
    mount_page(&server).await;
    let new = format!("{old}（依据美丽上海建设“十五五”规划）");
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(DRAFT))
        .and(body_string_contains("用户查到：2030年目标"))
        .and(body_string_contains(PAGE_FACT))
        .respond_with(reply(
            json!({"paragraphs": [new], "explanation": "引用查到的规划", "citations": [1]}),
        ))
        .mount(&server)
        .await;
    let request = FixRequest {
        sources: vec![FixSource {
            title: "美丽上海建设“十五五”规划".into(),
            url: format!("{}/plan.html", server.uri()),
            text: "用户查到：2030年目标".into(),
        }],
        ..FixRequest::default()
    };
    let (proposal, _) = propose(&core, &doc, &comment_id, &request).await;
    assert_eq!(proposal.paragraphs[0].new, new);
    assert_eq!(proposal.citations[0].title, "美丽上海建设“十五五”规划");
}
