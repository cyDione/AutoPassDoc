//! Enhanced knowledge-base parsing against mock MinerU and PaddleOCR
//! services: the clients, key checks and the import flow with fallback.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app_core::Core;
use app_core::enhanced::{
    EnhancedError, EnhancedOptions, MineruClient, PaddleClient, PollConfig, http_client,
};
use app_core::secrets::SecretStore;
use app_core::settings::ParserKind;
use models::Client;
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TOKEN: &str = "tok-test";

fn fast_poll() -> PollConfig {
    PollConfig {
        first_interval: Duration::from_millis(5),
        max_interval: Duration::from_millis(20),
        timeout: Duration::from_secs(5),
    }
}

fn mineru(server: &MockServer) -> MineruClient {
    MineruClient {
        base_url: format!("{}/api/v4", server.uri()),
        poll: fast_poll(),
        ..MineruClient::new(http_client().unwrap(), TOKEN, "vlm")
    }
}

fn paddle(base: &str) -> PaddleClient {
    PaddleClient {
        poll: fast_poll(),
        ..PaddleClient::new(http_client().unwrap(), base, TOKEN, "PaddleOCR-VL-1.6")
    }
}

/// Answers with each body in turn, repeating the last.
struct Sequence {
    bodies: Vec<Value>,
    calls: AtomicUsize,
}

impl Sequence {
    fn new(bodies: Vec<Value>) -> Self {
        Self {
            bodies,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Respond for Sequence {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        ResponseTemplate::new(200).set_body_json(&self.bodies[i.min(self.bodies.len() - 1)])
    }
}

fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in files {
        out.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(text.as_bytes()).unwrap();
    }
    out.finish().unwrap().into_inner()
}

type Stages = Mutex<Vec<(String, usize, usize)>>;

fn recorder(stages: &Stages) -> impl Fn(&str, usize, usize) + Send + Sync + '_ {
    move |stage, done, total| stages.lock().unwrap().push((stage.into(), done, total))
}

const MINERU_MD: &str = "# 某区项目实施方案\n\n<table><tr><td>项目</td><td>金额</td></tr><tr><td>道路</td><td>100</td></tr></table>\n\n![](images/1.jpg)\n";

async fn mount_mineru(server: &MockServer, result: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_string_contains("\"model_version\":\"vlm\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": 0,
            "msg": "ok",
            "data": {"batch_id": "b-1", "file_urls": [format!("{}/upload/b-1", server.uri())]}
        })))
        .mount(server)
        .await;
    // The presigned upload must not carry a Content-Type or the key.
    Mock::given(method("PUT"))
        .and(path("/upload/b-1"))
        .and(|r: &Request| {
            !r.headers.contains_key("content-type") && !r.headers.contains_key("authorization")
        })
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v4/extract-results/batch/b-1"))
        .respond_with(Sequence::new(vec![
            json!({"code": 0, "data": {"extract_result": [{"state": "waiting-file"}]}}),
            json!({"code": 0, "data": {"extract_result": [{"state": "running",
                "extract_progress": {"extracted_pages": 1, "total_pages": 2}}]}}),
            json!({"code": 0, "data": {"extract_result": [result]}}),
        ]))
        .mount(server)
        .await;
}

#[tokio::test]
async fn mineru_uploads_polls_and_reads_full_md() {
    let server = MockServer::start().await;
    mount_mineru(
        &server,
        json!({"state": "done", "full_zip_url": format!("{}/results/b-1.zip", server.uri())}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/results/b-1.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_of(&[
            ("b-1/layout.md", "不是这个"),
            ("b-1/full.md", MINERU_MD),
        ])))
        .mount(&server)
        .await;
    let stages = Stages::default();
    let md = mineru(&server)
        .parse("方案.pdf", b"%PDF-1.7".to_vec(), &recorder(&stages))
        .await
        .unwrap();
    assert_eq!(md, MINERU_MD);
    let stages = stages.into_inner().unwrap();
    assert_eq!(stages[0].0, "upload");
    assert!(stages.contains(&("parsing".into(), 1, 2)), "{stages:?}");
    assert_eq!(stages.last().unwrap().0, "download");
}

#[tokio::test]
async fn mineru_reports_failures() {
    let server = MockServer::start().await;
    mount_mineru(&server, json!({"state": "failed", "err_msg": "文件损坏"})).await;
    let err = mineru(&server)
        .parse("a.pdf", b"x".to_vec(), &|_, _, _| {})
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "MinerU 解析失败：文件损坏");

    let small = MineruClient {
        max_bytes: 2,
        ..mineru(&server)
    };
    assert!(matches!(
        small.parse("a.pdf", b"xyz".to_vec(), &|_, _, _| {}).await,
        Err(EnhancedError::TooLarge(_))
    ));

    let other = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code": -60005, "msg": "文件大小超出限制"})),
        )
        .mount(&other)
        .await;
    let err = mineru(&other)
        .parse("a.pdf", b"x".to_vec(), &|_, _, _| {})
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "MinerU 返回错误：文件大小超出限制");
    // The service answered, so the key itself is fine.
    mineru(&other).check_key().await.unwrap();
}

#[tokio::test]
async fn checks_mineru_keys() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .and(header("authorization", "Bearer bad"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"msg": "token invalid"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .and(header("authorization", "Bearer expired"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code": "A0211", "msg": "token expired"})),
        )
        .mount(&server)
        .await;
    let with = |token: &str| MineruClient {
        token: token.into(),
        ..mineru(&server)
    };
    let err = with("bad").check_key().await.unwrap_err();
    assert!(matches!(err, EnhancedError::InvalidKey(_)), "{err}");
    let err = with("expired").check_key().await.unwrap_err();
    assert!(matches!(err, EnhancedError::InvalidKey(_)), "{err}");
}

const JSONL: &str = r#"{"result":{"layoutParsingResults":[{"markdown":{"text":"第一条 图片中的规定。"}}]}}
{"result":{"layoutParsingResults":[{"markdown":{"text":"| 名称 | 数量 |\n|---|---|\n| 路灯 | 3 |"}}]}}
"#;

async fn mount_paddle(server: &MockServer, polls: usize) {
    Mock::given(method("POST"))
        .and(path("/api/v2/ocr/jobs"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(|r: &Request| {
            let ct = r.headers.get("content-type").and_then(|v| v.to_str().ok());
            let body = String::from_utf8_lossy(&r.body);
            ct.is_some_and(|c| c.starts_with("multipart/form-data; boundary="))
                && body.contains("name=\"model\"\r\n\r\nPaddleOCR-VL-1.6\r\n")
                && body.contains(r#"{"useDocOrientationClassify":true,"useDocUnwarping":false}"#)
                && body.contains("name=\"file\"; filename=")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"code": 0, "data": {"jobId": "j-1"}})),
        )
        .expect(polls as u64)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/ocr/jobs/j-1"))
        .respond_with(Sequence::new(vec![
            json!({"code": 0, "data": {"state": "pending"}}),
            json!({"code": 0, "data": {"state": "running",
                "extractProgress": {"totalPages": 2, "extractedPages": 1}}}),
            json!({"code": 0, "data": {"state": "done",
                "resultUrl": {"jsonUrl": format!("{}/bos/j-1.jsonl", server.uri())}}}),
        ]))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bos/j-1.jsonl"))
        .respond_with(ResponseTemplate::new(200).set_body_string(JSONL))
        .mount(server)
        .await;
}

#[tokio::test]
async fn paddle_submits_polls_and_joins_pages() {
    let server = MockServer::start().await;
    mount_paddle(&server, 1).await;
    let stages = Stages::default();
    let md = paddle(&server.uri())
        .parse("扫描.png", b"\x89PNG".to_vec(), &recorder(&stages))
        .await
        .unwrap();
    assert_eq!(
        md,
        "第一条 图片中的规定。\n\n| 名称 | 数量 |\n|---|---|\n| 路灯 | 3 |"
    );
    let stages = stages.into_inner().unwrap();
    assert!(stages.contains(&("queued".into(), 0, 0)));
    assert!(stages.contains(&("parsing".into(), 1, 2)));
}

#[tokio::test]
async fn paddle_errors_keys_and_timeouts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/ocr/jobs"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"code": 400, "errorMsg": "文件格式不支持"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/ocr/jobs/apd-connection-test"))
        .and(header("authorization", "Bearer bad"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/ocr/jobs/apd-connection-test"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"code": 404, "msg": "job not found"})),
        )
        .mount(&server)
        .await;
    let client = paddle(&server.uri());
    let err = client
        .parse("a.pdf", b"x".to_vec(), &|_, _, _| {})
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "PaddleOCR 返回 HTTP 400：文件格式不支持");
    client.check_key().await.unwrap();
    let bad = PaddleClient {
        token: "bad".into(),
        ..paddle(&server.uri())
    };
    assert!(matches!(
        bad.check_key().await,
        Err(EnhancedError::InvalidKey(_))
    ));
    assert!(matches!(
        paddle("http://127.0.0.1:9").check_key().await,
        Err(EnhancedError::Network(_))
    ));

    let slow = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/ocr/jobs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"code": 0, "data": {"jobId": "j-2"}})),
        )
        .mount(&slow)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/ocr/jobs/j-2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code": 0, "data": {"state": "running"}})),
        )
        .mount(&slow)
        .await;
    let client = PaddleClient {
        poll: PollConfig {
            timeout: Duration::from_millis(60),
            ..fast_poll()
        },
        ..paddle(&slow.uri())
    };
    assert!(matches!(
        client.parse("a.pdf", b"x".to_vec(), &|_, _, _| {}).await,
        Err(EnhancedError::Timeout(_))
    ));
}

fn core(dir: &Path, parser: ParserKind, paddle_url: &str, mineru_url: &str) -> Core {
    let data = dir.join("data");
    let core =
        Core::with_parts(&data, Client::new().unwrap(), SecretStore::file_only(&data)).unwrap();
    let mut settings = core.settings().unwrap();
    settings.kb.parser = parser;
    settings.kb.paddleocr_base_url = paddle_url.into();
    core.save_settings(settings).unwrap();
    core.set_enhanced_options(EnhancedOptions {
        mineru_base_url: format!("{mineru_url}/api/v4"),
        poll: fast_poll(),
        ..EnhancedOptions::default()
    });
    core
}

fn folder(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("资料");
    std::fs::create_dir_all(root.join("图片")).unwrap();
    std::fs::write(
        root.join("说明.md"),
        "# 说明\n\n第一条 本说明适用于崇明区。",
    )
    .unwrap();
    std::fs::write(root.join("扫描件.pdf"), b"%PDF-1.4 scanned").unwrap();
    std::fs::write(root.join("图片/照片.jpg"), b"\xFF\xD8\xFF").unwrap();
    std::fs::write(root.join("~$说明.docx"), b"lock").unwrap();
    root
}

#[tokio::test]
async fn imports_folders_through_paddleocr() {
    let server = MockServer::start().await;
    // One job per file: the PDF and the image, not the Markdown, and not
    // again when the folder is imported a second time.
    mount_paddle(&server, 2).await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(
        dir.path(),
        ParserKind::Paddleocr,
        &server.uri(),
        &server.uri(),
    );
    let root = folder(dir.path());

    // No key yet: built-in parsers only, images left out.
    assert_eq!(
        core.kb_count_import(std::slice::from_ref(&root)).unwrap(),
        2
    );
    assert!(core.enhanced_parser().unwrap().is_none());

    core.set_parser_key(ParserKind::Paddleocr, &format!(" {TOKEN} "))
        .unwrap();
    assert_eq!(
        core.kb_count_import(std::slice::from_ref(&root)).unwrap(),
        3
    );
    let seen = Mutex::new(Vec::new());
    let results = core
        .kb_import(std::slice::from_ref(&root), |done, total, current| {
            seen.lock()
                .unwrap()
                .push((done, total, current.to_string()))
        })
        .await;
    let summary: Vec<(&str, &str, bool)> = results
        .iter()
        .map(|r| (r.file_name.as_str(), r.parser.as_str(), r.error.is_none()))
        .collect();
    assert_eq!(
        summary,
        [
            ("照片.jpg", "paddleocr", true),
            ("扫描件.pdf", "paddleocr", true),
            ("说明.md", "builtin", true),
        ],
        "{results:?}"
    );
    let seen = seen.into_inner().unwrap();
    assert!(
        seen.iter()
            .any(|(_, _, c)| c == "照片.jpg（PaddleOCR 解析中 1/2 页）"),
        "{seen:?}"
    );
    assert_eq!(seen.last().unwrap(), &(3, 3, String::new()));

    let pdf = results[1].doc_id.unwrap();
    let view = core.kb_document_view(pdf).unwrap();
    assert_eq!(view.document.parser, "paddleocr");
    let texts: Vec<&str> = view.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(texts.contains(&"名称：路灯；数量：3"), "{texts:?}");
    assert!(!view.chunks.is_empty());
    let image = core
        .kb_documents()
        .unwrap()
        .into_iter()
        .find(|d| d.file_name == "照片.jpg")
        .unwrap();
    assert_eq!(image.format, "image");

    let again = core.kb_import(&[root], |_, _, _| {}).await;
    assert!(again.iter().all(|r| r.unchanged), "{again:?}");
    assert_eq!(again[0].parser, "paddleocr");
}

#[tokio::test]
async fn falls_back_to_builtin_when_mineru_fails() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"code": -10002, "msg": "额度不足"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(dir.path(), ParserKind::Mineru, &server.uri(), &server.uri());
    core.set_parser_key(ParserKind::Mineru, TOKEN).unwrap();
    let root = folder(dir.path());
    std::fs::write(root.join("正文.pdf"), "not a pdf").unwrap();
    let results = core.kb_import(&[root], |_, _, _| {}).await;
    let by_name = |n: &str| results.iter().find(|r| r.file_name == n).unwrap();

    let broken = by_name("正文.pdf");
    assert_eq!(broken.parser, "builtin");
    let error = broken.error.as_deref().unwrap();
    assert!(
        error.starts_with("增强解析失败：MinerU 返回错误：额度不足；普通模式也无法读取："),
        "{error}"
    );
    let image = by_name("照片.jpg");
    assert_eq!(
        image.error.as_deref(),
        Some("增强解析失败：MinerU 返回错误：额度不足")
    );
    assert!(by_name("说明.md").error.is_none());
}

#[tokio::test]
async fn parser_keys_and_connection_tests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .and(header("authorization", "Bearer wrong"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"msg": "Unauthorized"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"code": -500, "msg": "files is empty"})),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let core = core(
        dir.path(),
        ParserKind::Mineru,
        "http://127.0.0.1:9",
        &server.uri(),
    );

    let t = core.test_parser(ParserKind::Mineru, None).await.unwrap();
    assert!(!t.ok);
    assert_eq!(t.message, "请先填写 Key");
    assert!(t.quota.is_none());
    assert_eq!(t.console_url, "https://mineru.net/apiManage/token");

    let t = core
        .test_parser(ParserKind::Mineru, Some("wrong"))
        .await
        .unwrap();
    assert!(!t.ok);
    assert_eq!(t.message, "Key 无效：Unauthorized");
    let json = serde_json::to_value(&t).unwrap();
    assert!(json.get("quotaNote").is_some() && json.get("consoleUrl").is_some());

    let info = core.set_parser_key(ParserKind::Mineru, TOKEN).unwrap();
    assert!(info.has_key);
    assert_eq!(info.key_url, "https://mineru.net/apiManage/token");
    let t = core.test_parser(ParserKind::Mineru, None).await.unwrap();
    assert!(t.ok, "{}", t.message);

    core.set_parser_key(ParserKind::Paddleocr, TOKEN).unwrap();
    let t = core.test_parser(ParserKind::Paddleocr, None).await.unwrap();
    assert!(!t.ok);
    assert!(
        t.message.starts_with("无法连接 PaddleOCR："),
        "{}",
        t.message
    );

    let infos = core.parser_infos().unwrap();
    assert_eq!(infos.len(), 2);
    assert!(infos.iter().all(|i| i.has_key));
    core.clear_parser_key(ParserKind::Mineru).unwrap();
    assert!(!core.has_parser_key(ParserKind::Mineru).unwrap());
    assert!(core.set_parser_key(ParserKind::Mineru, "  ").is_err());
    assert!(core.test_parser(ParserKind::Builtin, None).await.is_err());
    // Keys are never in the settings.
    let settings = serde_json::to_string(&core.settings().unwrap()).unwrap();
    assert!(!settings.contains(TOKEN));
}

#[test]
fn kb_import_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let core =
        Core::with_parts(&data, Client::new().unwrap(), SecretStore::file_only(&data)).unwrap();
    assert_send(core.kb_import(&[], |_, _, _| {}));
}
