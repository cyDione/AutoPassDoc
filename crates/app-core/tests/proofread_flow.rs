//! Proofreading against a mock chat endpoint and a fake web search.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use app_core::proofread::{
    self, Category, Citation, CitationLookup, ProofInput, ProofOptions, ProofParagraph,
    ProofProgress, ProofStage, SearchSnippet, Severity, Source, Status,
};
use app_core::secrets::SecretStore;
use app_core::settings::RoleModel;
use app_core::store::ProviderRecord;
use app_core::{Core, Error, providers};
use docx_engine::Document;
use docx_engine::testgen::{Spec, generate};
use futures::future::BoxFuture;
use models::{Client, ClientConfig};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn core(dir: &std::path::Path, server: &MockServer, with_chat: bool) -> Core {
    let client = Client::with_config(ClientConfig {
        retry_base_delay: Duration::from_millis(1),
        ..ClientConfig::default()
    })
    .unwrap();
    let core = Core::with_parts(dir, client, SecretStore::file_only(dir)).unwrap();
    if with_chat {
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
        s.proofread.concurrency = 2;
        store.save_settings(&s).unwrap();
        drop(store);
        core.secrets()
            .set(&providers::secret_name("gw"), "sk-test")
            .unwrap();
    }
    core
}

fn chat_reply(content: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": content.to_string()}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 10}
    }))
}

struct FakeSearch {
    calls: AtomicUsize,
}

impl CitationLookup for FakeSearch {
    fn lookup<'a>(
        &'a self,
        citation: &'a Citation,
    ) -> BoxFuture<'a, app_core::Result<Vec<SearchSnippet>>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![SearchSnippet {
                title: "GB 50014-2021 室外排水设计标准".into(),
                url: "https://openstd.samr.gov.cn/example".into(),
                snippet: format!(
                    "本标准代替{}",
                    citation.standard_no.clone().unwrap_or_default()
                ),
            }])
        })
    }
}

fn filler() -> String {
    "项目建设内容包括管网改造和泵站建设。".repeat(80)
}

fn input(chapter_two_tail: &str) -> ProofInput {
    let p4 = format!(
        "本项目位于浦东新区，总投资3.2亿元，建成后可有效改善区域水环竟。依据《室外排水设计规范》（GB 50014-2006）设计。{}",
        filler()
    );
    let p6 = format!("{chapter_two_tail}{}经复核，项目总投资3.5亿元。", filler());
    ProofInput {
        paragraphs: vec![
            ProofParagraph::new(0, "崇明区城桥镇污水管网改造工程可行性研究报告"),
            ProofParagraph::new(1, "项目名称：崇明区城桥镇污水管网改造工程"),
            ProofParagraph::new(2, "建设地点：上海市崇明区城桥镇"),
            ProofParagraph::heading(3, "第一章 概况", 0),
            ProofParagraph::new(4, p4),
            ProofParagraph::heading(5, "第二章 方案", 0),
            ProofParagraph::new(6, p6),
            ProofParagraph::new(7, "一、道路"),
            ProofParagraph::new(8, "三、绿化 。"),
        ],
    }
}

async fn mount(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("水环竟"))
        .respond_with(chat_reply(json!({
            "issues": [
                {"p": 4, "original": "水环竟", "suggestion": "水环境", "category": "typo", "reason": "错别字"},
                {"p": 4, "original": "找不到的片段", "suggestion": "x", "category": "typo", "reason": "应被丢弃"}
            ],
            "facts": [
                {"p": 4, "subject": "本项目", "metric": "总投资", "value": "3.2", "unit": "亿元", "text": "总投资3.2亿元"}
            ]
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("第二章 方案"))
        .respond_with(chat_reply(json!({
            "issues": [],
            "facts": [
                [6, "项目", "总投资", 3.5, "亿元"]
            ]
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("【第1组】"))
        .respond_with(chat_reply(json!({
            "groups": [{"id": 1, "conflict": true, "reason": "同一口径的总投资数值不同"}]
        })))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("【被引用的"))
        .respond_with(chat_reply(json!({
            "status": "superseded",
            "replacement": {"title": "室外排水设计标准", "number": "GB 50014-2021", "url": "https://openstd.samr.gov.cn/example"},
            "reason": "GB 50014-2021 已代替 2006 版"
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn requests_containing(server: &MockServer, needle: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains(needle))
        .count()
}

fn assert_send<T: Send>(t: T) -> T {
    t
}

#[tokio::test]
async fn proofreads_with_rules_model_and_citations() {
    let server = MockServer::start().await;
    mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server, true);
    let search = FakeSearch {
        calls: AtomicUsize::new(0),
    };
    let stages = Mutex::new(Vec::<ProofProgress>::new());
    let cancel = AtomicBool::new(false);
    let options = ProofOptions {
        doc_key: "报告.docx".into(),
        ..Default::default()
    };
    let doc = input("");
    let report = assert_send(core.proofread(
        &doc,
        &options,
        Some(&search),
        |p| stages.lock().unwrap().push(p),
        &cancel,
    ))
    .await
    .unwrap();

    assert_eq!(report.facts.district.as_deref(), Some("崇明区"));
    assert_eq!(
        report.facts.name.as_deref(),
        Some("崇明区城桥镇污水管网改造工程")
    );
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.sections, 2);
    assert_eq!(report.cached_sections, 0);
    assert_eq!(report.model_calls, 4, "2 sections + confirm + 1 citation");
    assert_eq!(search.calls.load(Ordering::SeqCst), 1);

    let find = |cat: Category, p: usize| {
        report
            .issues
            .iter()
            .filter(|i| i.category == cat && i.paragraph == p)
            .collect::<Vec<_>>()
    };
    let typo = find(Category::Typo, 4);
    assert_eq!(typo.len(), 1, "{:?}", report.issues);
    assert_eq!(typo[0].original, "水环竟");
    assert_eq!(typo[0].source, Source::Model);
    let wrong_place = find(Category::Misattribution, 4);
    assert_eq!(wrong_place[0].original, "浦东新区");
    assert_eq!(wrong_place[0].suggestion.as_deref(), Some("崇明区"));
    assert_eq!(wrong_place[0].severity, Severity::Error);
    assert_eq!(find(Category::Consistency, 4)[0].original, "总投资3.2亿元");
    assert_eq!(find(Category::Consistency, 6)[0].original, "总投资3.5亿元");
    let numbering = find(Category::Numbering, 8);
    assert_eq!(numbering[0].suggestion.as_deref(), Some("二、"));
    assert_eq!(find(Category::Format, 8)[0].original, " ");
    let citation = find(Category::Citation, 4);
    assert_eq!(citation[0].original, "GB 50014-2006");
    assert_eq!(citation[0].suggestion.as_deref(), Some("GB 50014-2021"));
    let cited = report
        .citations
        .iter()
        .find(|c| c.citation.title == "室外排水设计规范")
        .unwrap();
    assert_eq!(cited.status.as_ref().unwrap().status, Status::Superseded);
    assert_eq!(report.counts["consistency"], 2);
    let ids: std::collections::HashSet<_> = report.issues.iter().map(|i| &i.id).collect();
    assert_eq!(ids.len(), report.issues.len());

    let seen: Vec<ProofStage> = stages.lock().unwrap().iter().map(|p| p.stage).collect();
    assert_eq!(seen.first(), Some(&ProofStage::Rules));
    assert!(seen.contains(&ProofStage::Model));
    assert!(seen.contains(&ProofStage::Consistency));
    assert!(seen.contains(&ProofStage::Citations));
    assert_eq!(seen.last(), Some(&ProofStage::Done));
    // Findings arrive while the run is going: rules first, then each section.
    let streamed: Vec<(ProofStage, String)> = stages
        .lock()
        .unwrap()
        .iter()
        .flat_map(|p| p.found.iter().map(move |i| (p.stage, i.original.clone())))
        .collect();
    assert!(streamed.contains(&(ProofStage::Model, "水环竟".to_string())));
    assert!(streamed.iter().any(|(s, _)| *s == ProofStage::Rules));

    // Applying the typo fix to the paragraph text.
    let fixed = proofread::apply_issue(&doc.paragraphs[4].text, typo[0]).unwrap();
    assert!(fixed.contains("水环境。"));

    // The JSON the UI receives.
    let v = serde_json::to_value(&report).unwrap();
    assert!(v["issues"][0].get("paragraph").is_some());
    assert!(v["citations"][0].get("standardNo").is_some());
    assert!(v.get("cachedSections").is_some());

    // Unchanged text: everything comes from the cache.
    let before = server.received_requests().await.unwrap().len();
    let again = core
        .proofread(&doc, &options, Some(&search), |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(again.model_calls, 0);
    assert_eq!(again.cached_sections, 2);
    assert_eq!(again.issues, report.issues);
    assert_eq!(search.calls.load(Ordering::SeqCst), 1);

    // Editing chapter two only re-checks that section.
    let chapter_two = requests_containing(&server, "第二章 方案").await;
    let chapter_one = requests_containing(&server, "水环竟").await;
    let edited = input("另外，");
    let third = core
        .proofread(&edited, &options, Some(&search), |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(third.cached_sections, 1);
    assert_eq!(third.model_calls, 1);
    assert_eq!(
        requests_containing(&server, "第二章 方案").await,
        chapter_two + 1
    );
    assert_eq!(requests_containing(&server, "水环竟").await, chapter_one);
    assert_eq!(third.issues.len(), report.issues.len());
}

#[tokio::test]
async fn cancel_rules_only_and_setup() {
    let server = MockServer::start().await;
    mount(&server).await;
    let dir = tempfile::tempdir().unwrap();

    // Without a chat model: rules only, or an error asking to set one up.
    let bare = core(dir.path(), &server, false);
    let rules_only = ProofOptions {
        use_model: false,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let report = bare
        .proofread(&input(""), &rules_only, None, |_| {}, &cancel)
        .await
        .unwrap();
    assert!(report.issues.iter().all(|i| i.source == Source::Rule));
    assert!(report.counts.contains_key("misattribution"));
    assert!(report.citations.iter().all(|c| c.status.is_none()));
    let err = bare
        .proofread(&input(""), &ProofOptions::default(), None, |_| {}, &cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Setup(_)));

    // Cancelled before the first request: no model calls.
    let dir2 = tempfile::tempdir().unwrap();
    let core = core(dir2.path(), &server, true);
    let cancel = AtomicBool::new(true);
    let report = core
        .proofread(&input(""), &ProofOptions::default(), None, |_| {}, &cancel)
        .await
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.model_calls, 0);
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(!report.issues.is_empty());

    // Only the chosen checks are reported.
    let only_format = ProofOptions {
        categories: vec![Category::Format],
        use_model: false,
        ..Default::default()
    };
    let report = core
        .proofread(
            &input(""),
            &only_format,
            None,
            |_| {},
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    assert!(report.issues.iter().all(|i| i.category == Category::Format));
    assert!(!report.issues.is_empty());
}

#[test]
fn gathers_a_generated_document() {
    let doc = Document::from_bytes(generate(&Spec {
        target_chars: 30_000,
        comments: 5,
        seed: 3,
    }))
    .unwrap();
    let all = proofread::gather(&doc, None);
    assert_eq!(all.paragraphs.len(), doc.paragraphs.len());
    assert!(all.paragraphs.iter().any(|p| p.heading_level.is_some()));
    let part = proofread::gather(&doc, Some((5, 9)));
    let idx: Vec<usize> = part.paragraphs.iter().map(|p| p.index).collect();
    assert_eq!(idx, [5, 6, 7, 8, 9]);
    assert!(
        proofread::gather(&doc, Some((usize::MAX - 1, usize::MAX)))
            .paragraphs
            .is_empty()
    );
    // The rules run over a whole document without trouble.
    let facts = proofread::extract_facts(&all.paragraphs);
    let issues = proofread::check_rules(&all, &facts, &Category::ALL);
    for i in &issues {
        let text = &all
            .paragraphs
            .iter()
            .find(|p| p.index == i.paragraph)
            .unwrap()
            .text;
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars[i.start..i.end].iter().collect::<String>(), i.original);
    }
    let _ = proofread::extract_citations(&all.paragraphs);
}

#[tokio::test]
async fn applies_issues_as_one_tracked_change() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server, false);
    let mut doc = Document::from_bytes(generate(&Spec {
        target_chars: 5_000,
        comments: 0,
        seed: 7,
    }))
    .unwrap();
    let (index, text) = (0..doc.paragraphs.len())
        .map(|i| (i, doc.editable_text(i)))
        .find(|(_, t)| t.chars().count() > 20)
        .unwrap();
    let first: String = text.chars().take(2).collect();
    let issue =
        |id: &str, start: usize, original: &str, suggestion: Option<&str>| proofread::Issue {
            id: id.into(),
            category: Category::Typo,
            severity: Severity::Error,
            paragraph: index,
            start,
            end: start + original.chars().count(),
            original: original.into(),
            suggestion: suggestion.map(Into::into),
            reason: String::new(),
            source: Source::Rule,
        };
    let issues = vec![
        issue("a", 0, &first, Some("【改】")),
        // Stale: the text no longer matches.
        issue("b", 5, "不存在", Some("x")),
        // No suggestion: left for the user.
        issue("c", 8, "", None),
    ];
    let applied = core.apply_proof_issues(&mut doc, &issues).unwrap();
    assert_eq!(applied, ["a"]);
    assert!(doc.editable_text(index).starts_with("【改】"));
    assert_eq!(doc.undo_label(), Some(proofread::PROOFREAD_LABEL));
    assert!(core.apply_proof_issues(&mut doc, &issues[1..]).is_err());
}

/// Jev: "yes" for paragraphs holding 竟, "no" for the rest.
struct JevGate;

impl wiremock::Respond for JevGate {
    fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        let state = body["state"].as_str().unwrap();
        let answers: serde_json::Map<String, serde_json::Value> = body["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| {
                let line = format!("[{}] ", &k[1..]);
                let flagged = state
                    .lines()
                    .any(|l| l.starts_with(&line) && l.contains('竟'));
                let p = if flagged { 0.9 } else { 0.03 };
                (k.clone(), json!({"type": "noul", "noul": p}))
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "answers": answers }))
    }
}

#[tokio::test]
async fn decision_model_screens_what_the_chat_model_reads() {
    let server = MockServer::start().await;
    mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(JevGate)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), &server, true);
    {
        let store = core.store();
        let mut s = store.settings().unwrap();
        s.roles.decision = RoleModel {
            provider_id: "gw".into(),
            model: "jev-latest".into(),
            thinking: String::new(),
        };
        store.save_settings(&s).unwrap();
    }
    let mut doc = input("");
    let clean = "本工程管网采用球墨铸铁管，接口为承插式橡胶圈接口。";
    let typo = doc.paragraphs[4].text.clone();
    doc.paragraphs.extend([
        ProofParagraph::new(9, clean),
        ProofParagraph::new(10, "12.5"),
        ProofParagraph::new(11, typo.clone()),
        ProofParagraph::new(12, "统一布署，分期实施。"),
    ]);
    let options = ProofOptions {
        doc_key: "报告.docx".into(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let stages = Mutex::new(Vec::<ProofStage>::new());
    let report = core
        .proofread(
            &doc,
            &options,
            None,
            |p| stages.lock().unwrap().push(p.stage),
            &cancel,
        )
        .await
        .unwrap();
    assert!(stages.lock().unwrap().contains(&ProofStage::Screen));
    // "12.5" is too short and paragraph 11 repeats paragraph 4.
    assert_eq!(report.skipped, 2);
    assert!(report.screen_calls >= 1);
    assert_eq!(report.screened_out, 10, "all but paragraph 4");
    assert_eq!(
        requests_containing(&server, clean).await,
        1,
        "only Jev saw it"
    );
    // The typo is found in both copies; the word list catches 布署.
    let typos: Vec<(usize, &str)> = report
        .issues
        .iter()
        .filter(|i| i.category == Category::Typo)
        .map(|i| (i.paragraph, i.original.as_str()))
        .collect();
    assert_eq!(typos, [(4, "水环竟"), (11, "水环竟"), (12, "布署")]);
    // Conflicting figures come from the text itself.
    assert_eq!(
        report.counts["consistency"], 3,
        "paragraph 11 repeats the figure"
    );

    // Screening results are cached with the paragraphs.
    let jev_calls = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with("systemone"))
        .count();
    let again = core
        .proofread(&doc, &options, None, |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(again.screen_calls, 0);
    assert_eq!(again.screened_out, 10);
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().ends_with("systemone"))
            .count(),
        jev_calls
    );

    // Reading everything skips Jev.
    let full = core
        .proofread(
            &doc,
            &ProofOptions {
                screen: false,
                ..options.clone()
            },
            None,
            |_| {},
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(full.screen_calls, 0);
    assert_eq!(full.screened_out, 0);
}
