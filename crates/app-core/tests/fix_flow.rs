//! The AI-fix flow against mock chat and Jev endpoints.

use std::sync::Mutex;
use std::time::Duration;

use app_core::fix::{FixJob, Stage, context};
use app_core::secrets::SecretStore;
use app_core::settings::RoleModel;
use app_core::store::{CaseAction, ProviderRecord};
use app_core::{Core, providers};
use docx_engine::Document;
use docx_engine::testgen::{Spec, generate};
use models::{Client, ClientConfig};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
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
            model: "cline-pass/deepseek-v4.1-flash".into(),
            thinking: "off".into(),
        };
        s.roles.decision = RoleModel {
            provider_id: "gw".into(),
            model: "jev-latest".into(),
            thinking: String::new(),
        };
        s.fix.reply_on_apply = true;
        store.save_settings(&s).unwrap();
    }
    core.secrets()
        .set(&providers::secret_name("gw"), "sk-test")
        .unwrap();
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

fn chat_reply(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 10}
    }))
}

fn jev_reply(grounded: f64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"answers": {
        "addresses": {"type": "noul", "noul": 0.93},
        "meaning": {"type": "noul", "noul": 0.95},
        "grounded": {"type": "noul", "noul": grounded},
        "style": {"type": "score", "score": 0.8},
        "category": {"type": "choice", "choice": "数据口径",
                     "probabilities": {"数据口径": 0.7, "政策依据": 0.1, "措辞规范": 0.1, "格式": 0.04, "逻辑结构": 0.03, "其他": 0.03},
                     "confidence": 0.7}
    }}))
}

async fn propose(
    core: &Core,
    doc: &Document,
    comment_id: &str,
    stages: &Mutex<Vec<Stage>>,
) -> app_core::Result<app_core::fix::Proposal> {
    let job = FixJob {
        doc_key: "doc".into(),
        doc_name: "报告.docx".into(),
        input: context::gather(doc, comment_id).unwrap(),
    };
    core.propose(
        job,
        |c| Ok(doc.check_edits(c)?),
        |s| stages.lock().unwrap().push(s),
    )
    .await
}

#[tokio::test]
async fn proposes_judges_applies_and_learns() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    let mut doc = doc();
    let comment_id = pick(&doc);
    let input = context::gather(&doc, &comment_id).unwrap();
    let old = input.paragraphs[0].1.clone();
    let new = format!("经核实，{old}");

    // The first answer has the wrong paragraph count; the correction is used.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(chat_reply("{\"paragraphs\": [\"a\", \"b\"]}"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let answer =
        json!({"paragraphs": [new], "explanation": "补充了核实依据", "citations": []}).to_string();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(chat_reply(&format!("```json\n{answer}\n```")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(jev_reply(0.9))
        .mount(&server)
        .await;

    let stages = Mutex::new(Vec::new());
    let proposal = propose(&core, &doc, &comment_id, &stages).await.unwrap();
    assert_eq!(
        *stages.lock().unwrap(),
        [
            Stage::Context,
            Stage::Retrieve,
            Stage::Generate,
            Stage::Judge
        ]
    );
    assert_eq!(proposal.paragraphs[0].new, new);
    assert_eq!(proposal.paragraphs[0].diff[0].text, "经核实，");
    let judge = proposal.judge.as_ref().unwrap();
    assert!(judge.passed, "{judge:?}");
    assert_eq!(judge.category.as_deref(), Some("数据口径"));
    assert_eq!(proposal.explanation, "补充了核实依据");

    // The model saw the comment and was asked to fix its paragraph count.
    let requests = server.received_requests().await.unwrap();
    let chats: Vec<Value> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("chat/completions"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(chats.len(), 2);
    let first_user = chats[0]["messages"][1]["content"].as_str().unwrap();
    assert!(first_user.contains(&input.comment));
    let retry = chats[1]["messages"].as_array().unwrap();
    assert!(
        retry.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("应返回 1 段")
    );
    let jev: Value = requests
        .iter()
        .find(|r| r.url.path().ends_with("systemone"))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .unwrap();
    assert!(jev["state"].as_str().unwrap().contains(&new));
    assert_eq!(jev["questions"]["style"]["type"], "score");

    let paragraph = proposal.paragraphs[0].index;
    let replies_before = doc.comments.len();
    let due = core.apply_fix(&mut doc, &proposal.id, None, false).unwrap();
    assert_eq!(due, None, "no reviewer is mapped");
    assert_eq!(doc.editable_text(paragraph), new);
    assert!(doc.comment(&comment_id).unwrap().done);
    assert_eq!(doc.comments.len(), replies_before + 1, "a reply was added");
    assert!(doc.undo_label().unwrap().starts_with("AI 修复"));
    let case = core.store().decided_cases().unwrap().pop().unwrap();
    assert_eq!(case.action, CaseAction::Accepted);
    assert_eq!(case.final_text.as_deref(), Some(new.as_str()));

    // One undo step removes the rewrite, the reply and the resolved mark.
    doc.undo().unwrap();
    assert_eq!(doc.editable_text(paragraph), old);
    assert!(!doc.is_modified());
    assert!(
        core.apply_fix(&mut doc, &proposal.id, None, false).is_err(),
        "applied proposals are gone"
    );
}

#[tokio::test]
async fn low_confidence_needs_force_and_rejections_are_recorded() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    {
        let store = core.store();
        let r = store.create_reviewer("张处长", "").unwrap();
        let mut s = store.settings().unwrap();
        s.fix.profile_every = 1;
        store.save_settings(&s).unwrap();
        drop(store);
        let author = doc().comment(&pick(&doc())).unwrap().author.clone();
        app_core::reviewers::assign(&core.store(), &author, "", "doc", Some(r.id)).unwrap();
    }
    let mut doc = doc();
    let comment_id = pick(&doc);
    let old = context::gather(&doc, &comment_id).unwrap().paragraphs[0]
        .1
        .clone();
    let answer = json!({"paragraphs": [format!("{old}（约1.2亿元）")], "explanation": "补充金额", "citations": []});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(chat_reply(&answer.to_string()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(jev_reply(0.3))
        .mount(&server)
        .await;

    let stages = Mutex::new(Vec::new());
    let p = propose(&core, &doc, &comment_id, &stages).await.unwrap();
    assert_eq!(p.reviewer.as_deref(), Some("张处长"));
    let j = p.judge.as_ref().unwrap();
    assert!(!j.passed, "an invented number fails the grounded check");
    assert!(core.apply_fix(&mut doc, &p.id, None, false).is_err());
    let edited = vec![format!("{old}（金额待补充）")];
    let due = core
        .apply_fix(&mut doc, &p.id, Some(edited.clone()), true)
        .unwrap();
    assert!(
        due.is_some(),
        "the reviewer's profile is due after one case"
    );
    assert_eq!(doc.editable_text(p.paragraphs[0].index), edited[0]);
    assert_eq!(
        core.store().decided_cases().unwrap()[0].action,
        CaseAction::Edited
    );

    let p2 = propose(&core, &doc, &pick(&doc), &stages).await.unwrap();
    core.reject_fix(&p2.id).unwrap();
    let cases = core.store().decided_cases().unwrap();
    assert_eq!(cases.last().unwrap().action, CaseAction::Rejected);

    let out = dir.path().join("dataset.jsonl");
    assert_eq!(core.export_dataset(&out).unwrap(), 2);
    let lines: Vec<Value> = std::fs::read_to_string(&out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["action"], "edited");
    assert_eq!(lines[0]["reviewer"], "张处长");
    assert_eq!(lines[1]["label"], 0.0);
    assert!(lines[0]["state"].as_str().unwrap().contains("【修改后】"));
}

#[tokio::test]
async fn pre_review_predicts_the_reviewers_comments() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    let doc = doc();
    let reviewer = core.store().create_reviewer("李主任", "财政口").unwrap();
    let paragraphs: Vec<(usize, String)> = (0..40).map(|i| (i, doc.editable_text(i))).collect();
    let err = core.pre_review(reviewer.id, &paragraphs).await.unwrap_err();
    assert!(err.to_string().contains("还不了解"), "{err}");

    let summary = app_core::profiles::ProfileSummary {
        summary: "关注资金测算".into(),
        ..Default::default()
    };
    core.store()
        .add_profile(reviewer.id, &serde_json::to_string(&summary).unwrap(), 3)
        .unwrap();
    let (index, text) = paragraphs
        .iter()
        .find(|(_, t)| t.chars().count() > 20)
        .unwrap();
    let quote: String = text.chars().take(6).collect();
    let answer = json!({"items": [
        {"paragraph": index, "quote": quote, "comment": "请补充测算依据", "category": "数据口径"},
        {"paragraph": 999, "quote": "", "comment": "越界"}
    ]});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(chat_reply(&answer.to_string()))
        .mount(&server)
        .await;
    let items = core.pre_review(reviewer.id, &paragraphs).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].paragraph_index, *index);
    assert_eq!(items[0].quote, quote);
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("关注资金测算") && user.contains(&format!("[{index}] ")));
}

#[tokio::test]
async fn works_without_a_decision_model() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server);
    {
        let store = core.store();
        let mut s = store.settings().unwrap();
        s.roles.decision = RoleModel::default();
        store.save_settings(&s).unwrap();
    }
    let doc = doc();
    let comment_id = pick(&doc);
    let old = context::gather(&doc, &comment_id).unwrap().paragraphs[0]
        .1
        .clone();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(chat_reply(&json!({"paragraphs": [old]}).to_string()))
        .mount(&server)
        .await;
    let stages = Mutex::new(Vec::new());
    let p = propose(&core, &doc, &comment_id, &stages).await.unwrap();
    assert!(p.judge.is_none());
    assert_eq!(p.judge_error.as_deref(), Some("尚未配置决策模型"));
    assert!(p.warnings.iter().any(|w| w.contains("不需要修改")));
}
