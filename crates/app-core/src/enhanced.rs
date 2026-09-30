//! Enhanced knowledge-base parsing: PDFs and images go to an online
//! service (MinerU or PaddleOCR on Baidu AI Studio) for layout analysis,
//! table recognition and OCR, and come back as Markdown for
//! [`kb::KnowledgeBase::import_parsed`].
//!
//! Both services work the same way: submit the file, poll the job with a
//! growing interval, then download the result. Keys live in the secret
//! store under [`ParserKind::secret_name`] and are sent only to the
//! service itself, never to the storage hosts that serve uploads and
//! results. Neither service has a public quota endpoint.

use std::io::Read;
use std::time::{Duration, Instant};

use reqwest::{RequestBuilder, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::core::Core;
use crate::error::{Error, Result};
use crate::settings::ParserKind;

pub const MINERU_BASE_URL: &str = "https://mineru.net/api/v4";
/// Largest file either service accepts.
pub const MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// Job id used to check a PaddleOCR key without submitting anything.
const PROBE_JOB_ID: &str = "apd-connection-test";
/// MinerU `code`s for a wrong or expired token.
const MINERU_TOKEN_CODES: &[&str] = &["A0202", "A0211"];
const NO_QUOTA_NOTE: &str = "该服务不提供额度查询，请到控制台查看剩余额度";

/// Reports parsing progress: `(stage, done pages, total pages)`. Stages
/// are `upload`, `queued`, `parsing` (with page counts once the service
/// reports them, else 0/0) and `download`.
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Send + Sync);

/// Why an online parse or key check failed. Messages are shown as is.
#[derive(Debug, thiserror::Error)]
pub enum EnhancedError {
    #[error("Key 无效或已过期：{0}")]
    InvalidKey(String),
    #[error("无法连接：{0}")]
    Network(String),
    #[error("{0}")]
    Api(String),
    #[error("解析超时（超过 {0} 分钟），请稍后重试")]
    Timeout(u64),
    #[error("文件超过 {0} MB，在线解析不支持")]
    TooLarge(u64),
}

type EResult<T> = std::result::Result<T, EnhancedError>;

/// How often and how long to wait for a job.
#[derive(Debug, Clone)]
pub struct PollConfig {
    /// First wait; each later one is 1.5 times longer. Default 2 s.
    pub first_interval: Duration,
    /// Longest wait between two polls. Default 10 s.
    pub max_interval: Duration,
    /// Give up after this long. Default 10 min.
    pub timeout: Duration,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            first_interval: Duration::from_secs(2),
            max_interval: Duration::from_secs(10),
            timeout: Duration::from_secs(600),
        }
    }
}

struct Poller<'a> {
    config: &'a PollConfig,
    started: Instant,
    delay: Duration,
}

impl<'a> Poller<'a> {
    fn new(config: &'a PollConfig) -> Self {
        Self {
            config,
            started: Instant::now(),
            delay: config.first_interval,
        }
    }

    async fn wait(&mut self) -> EResult<()> {
        let elapsed = self.started.elapsed();
        if elapsed >= self.config.timeout {
            return Err(EnhancedError::Timeout(
                self.config.timeout.as_secs().div_ceil(60),
            ));
        }
        tokio::time::sleep(self.delay.min(self.config.timeout - elapsed)).await;
        self.delay = (self.delay * 3 / 2).min(self.config.max_interval);
        Ok(())
    }
}

/// Service addresses and limits the core uses; tests shorten the waits.
#[derive(Debug, Clone)]
pub struct EnhancedOptions {
    pub mineru_base_url: String,
    pub poll: PollConfig,
    pub max_bytes: u64,
}

impl Default for EnhancedOptions {
    fn default() -> Self {
        Self {
            mineru_base_url: MINERU_BASE_URL.into(),
            poll: PollConfig::default(),
            max_bytes: MAX_UPLOAD_BYTES,
        }
    }
}

/// An HTTP client for the parsing services: no overall timeout (uploads
/// can be large), but connections and stalled reads time out.
pub fn http_client() -> EResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(120))
        .user_agent(concat!("AutoPassDoc/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| EnhancedError::Network(format!("HTTP 客户端初始化失败：{e}")))
}

fn network(e: reqwest::Error) -> EnhancedError {
    let mut message = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        message = format!("{message}：{s}");
        source = s.source();
    }
    EnhancedError::Network(message)
}

/// The service's error text from a JSON body, or the start of a text body.
fn message_of(body: Option<&Value>, text: &str) -> String {
    body.and_then(|b| {
        ["msg", "errorMsg", "message", "err_msg", "error"]
            .iter()
            .find_map(|k| b[k].as_str().filter(|s| !s.is_empty()))
            .map(str::to_string)
    })
    .unwrap_or_else(|| {
        let t = text.trim();
        if t.is_empty() {
            "（无说明）".into()
        } else {
            t.chars().take(200).collect()
        }
    })
}

fn code_ok(code: &Value) -> bool {
    match code {
        Value::Null => true,
        Value::Number(n) => n.as_i64() == Some(0),
        Value::String(s) => s == "0",
        _ => false,
    }
}

/// Sends a request to a service API and checks HTTP status and `code`.
async fn send_json(service: &str, request: RequestBuilder) -> EResult<Value> {
    let response = request.send().await.map_err(network)?;
    let status = response.status();
    let text = response.text().await.map_err(network)?;
    let body: Option<Value> = serde_json::from_str(&text).ok();
    let message = || message_of(body.as_ref(), &text);
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(EnhancedError::InvalidKey(message()));
    }
    if !status.is_success() {
        return Err(EnhancedError::Api(format!(
            "{service} 返回 HTTP {}：{}",
            status.as_u16(),
            message()
        )));
    }
    let Some(body) = body.clone() else {
        return Err(EnhancedError::Api(format!("{service} 返回的内容无法识别")));
    };
    let code = &body["code"];
    if !code_ok(code) {
        if code
            .as_str()
            .is_some_and(|c| MINERU_TOKEN_CODES.contains(&c))
        {
            return Err(EnhancedError::InvalidKey(message()));
        }
        return Err(EnhancedError::Api(format!(
            "{service} 返回错误：{}",
            message()
        )));
    }
    Ok(body)
}

/// Downloads a result file (no key: result hosts are presigned storage).
async fn download(service: &str, http: &reqwest::Client, url: &str) -> EResult<Vec<u8>> {
    let response = http.get(url).send().await.map_err(network)?;
    let status = response.status();
    if !status.is_success() {
        return Err(EnhancedError::Api(format!(
            "下载 {service} 解析结果失败：HTTP {}",
            status.as_u16()
        )));
    }
    Ok(response.bytes().await.map_err(network)?.to_vec())
}

fn check_size(bytes: &[u8], max: u64) -> EResult<()> {
    if bytes.len() as u64 > max {
        return Err(EnhancedError::TooLarge(max / (1024 * 1024)));
    }
    Ok(())
}

fn count(v: &Value) -> usize {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0) as usize
}

/// MinerU precise parsing API (<https://mineru.net/apiManage/docs>).
#[derive(Debug, Clone)]
pub struct MineruClient {
    pub http: reqwest::Client,
    /// Default [`MINERU_BASE_URL`].
    pub base_url: String,
    pub token: String,
    /// `vlm` or `pipeline`.
    pub model_version: String,
    /// `is_ocr`: OCR every page.
    pub ocr: bool,
    /// `enable_formula`.
    pub formula: bool,
    pub poll: PollConfig,
    pub max_bytes: u64,
}

impl MineruClient {
    pub fn new(http: reqwest::Client, token: &str, model_version: &str) -> Self {
        Self {
            http,
            base_url: MINERU_BASE_URL.into(),
            token: token.trim().to_string(),
            model_version: model_version.to_string(),
            ocr: true,
            formula: true,
            poll: PollConfig::default(),
            max_bytes: MAX_UPLOAD_BYTES,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }

    /// Uploads a PDF or image, waits for the result and returns its Markdown.
    pub async fn parse(
        &self,
        file_name: &str,
        bytes: Vec<u8>,
        progress: Progress<'_>,
    ) -> EResult<String> {
        check_size(&bytes, self.max_bytes)?;
        let data_id: String = Sha256::digest(&bytes)
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        let body = json!({
            "files": [{
                "name": file_name,
                "data_id": format!("apd-{data_id}"),
                "is_ocr": self.ocr,
            }],
            "model_version": self.model_version,
            "enable_table": true,
            "enable_formula": self.formula,
            "language": "ch",
        });
        let created = send_json(
            "MinerU",
            self.http
                .post(self.url("/file-urls/batch"))
                .bearer_auth(&self.token)
                .json(&body),
        )
        .await?;
        let data = &created["data"];
        let (Some(batch_id), Some(upload_url)) =
            (data["batch_id"].as_str(), data["file_urls"][0].as_str())
        else {
            return Err(EnhancedError::Api("MinerU 没有返回上传地址".into()));
        };

        progress("upload", 0, 0);
        // The presigned URL was signed without Content-Type; sending one
        // makes the storage reject the upload.
        let response = self
            .http
            .put(upload_url)
            .body(bytes)
            .send()
            .await
            .map_err(network)?;
        if !response.status().is_success() {
            return Err(EnhancedError::Api(format!(
                "上传文件到 MinerU 失败：HTTP {}",
                response.status().as_u16()
            )));
        }

        let mut poller = Poller::new(&self.poll);
        let zip_url = loop {
            let status = send_json(
                "MinerU",
                self.http
                    .get(self.url(&format!("/extract-results/batch/{batch_id}")))
                    .bearer_auth(&self.token),
            )
            .await?;
            let result = &status["data"]["extract_result"][0];
            match result["state"].as_str().unwrap_or("waiting-file") {
                "done" => match result["full_zip_url"].as_str() {
                    Some(url) => break url.to_string(),
                    None => return Err(EnhancedError::Api("MinerU 没有返回结果地址".into())),
                },
                "failed" => {
                    let reason = result["err_msg"].as_str().unwrap_or("（无说明）");
                    return Err(EnhancedError::Api(format!("MinerU 解析失败：{reason}")));
                }
                "running" | "converting" => {
                    let p = &result["extract_progress"];
                    progress(
                        "parsing",
                        count(&p["extracted_pages"]),
                        count(&p["total_pages"]),
                    );
                }
                _ => progress("queued", 0, 0),
            }
            poller.wait().await?;
        };

        progress("download", 0, 0);
        let archive = download("MinerU", &self.http, &zip_url).await?;
        markdown_from_zip(&archive)
    }

    /// Checks the key with an empty batch request, which uploads nothing.
    pub async fn check_key(&self) -> EResult<()> {
        let body = json!({"files": [], "model_version": self.model_version});
        let request = self
            .http
            .post(self.url("/file-urls/batch"))
            .bearer_auth(&self.token)
            .json(&body);
        match send_json("MinerU", request).await {
            // The service rejects the empty list only after accepting the key.
            Ok(_) | Err(EnhancedError::Api(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// `full.md` from MinerU's result archive, else its first `.md` file.
fn markdown_from_zip(bytes: &[u8]) -> EResult<String> {
    let bad =
        |e: zip::result::ZipError| EnhancedError::Api(format!("MinerU 结果压缩包无法读取：{e}"));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(bad)?;
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let is_md = |n: &String| n.to_ascii_lowercase().ends_with(".md");
    let name = names
        .iter()
        .find(|n| *n == "full.md" || n.ends_with("/full.md"))
        .or_else(|| names.iter().filter(|n| is_md(n)).min())
        .ok_or_else(|| EnhancedError::Api("MinerU 结果中没有 Markdown 文件".into()))?
        .clone();
    let mut text = String::new();
    archive
        .by_name(&name)
        .map_err(bad)?
        .read_to_string(&mut text)
        .map_err(|e| EnhancedError::Api(format!("MinerU 结果无法读取：{e}")))?;
    Ok(text)
}

/// PaddleOCR document parsing on Baidu AI Studio
/// (<https://ai.baidu.com/ai-doc/AISTUDIO/Kmfl2ycs0>), as the official
/// `paddleocr` API client calls it.
#[derive(Debug, Clone)]
pub struct PaddleClient {
    pub http: reqwest::Client,
    /// Default `https://paddleocr.aistudio-app.com`.
    pub base_url: String,
    /// AI Studio access token.
    pub token: String,
    /// `PaddleOCR-VL-1.6` or `PP-StructureV3`.
    pub model: String,
    pub poll: PollConfig,
    pub max_bytes: u64,
}

impl PaddleClient {
    pub fn new(http: reqwest::Client, base_url: &str, token: &str, model: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
            model: model.to_string(),
            poll: PollConfig::default(),
            max_bytes: MAX_UPLOAD_BYTES,
        }
    }

    fn jobs_url(&self) -> String {
        format!("{}/api/v2/ocr/jobs", self.base_url.trim_end_matches('/'))
    }

    /// Submits a PDF or image, waits for the job and returns the pages'
    /// Markdown joined with blank lines.
    pub async fn parse(
        &self,
        file_name: &str,
        bytes: Vec<u8>,
        progress: Progress<'_>,
    ) -> EResult<String> {
        check_size(&bytes, self.max_bytes)?;
        progress("upload", 0, 0);
        let options = json!({"useDocOrientationClassify": true, "useDocUnwarping": false});
        let (content_type, body) = multipart(
            &[
                ("model", &self.model),
                ("optionalPayload", &options.to_string()),
            ],
            ("file", file_name, &bytes),
        );
        drop(bytes);
        let created = send_json(
            "PaddleOCR",
            self.http
                .post(self.jobs_url())
                .bearer_auth(&self.token)
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body),
        )
        .await?;
        let Some(job_id) = created["data"]["jobId"].as_str().map(str::to_string) else {
            return Err(EnhancedError::Api("PaddleOCR 没有返回任务编号".into()));
        };

        let mut poller = Poller::new(&self.poll);
        let json_url = loop {
            let status = send_json(
                "PaddleOCR",
                self.http
                    .get(format!("{}/{job_id}", self.jobs_url()))
                    .bearer_auth(&self.token),
            )
            .await?;
            let data = &status["data"];
            match data["state"].as_str().unwrap_or("pending") {
                "done" => match data["resultUrl"]["jsonUrl"].as_str() {
                    Some(url) => break url.to_string(),
                    None => return Err(EnhancedError::Api("PaddleOCR 没有返回结果地址".into())),
                },
                "failed" => {
                    let reason = data["errorMsg"].as_str().unwrap_or("（无说明）");
                    return Err(EnhancedError::Api(format!("PaddleOCR 解析失败：{reason}")));
                }
                "running" => {
                    let p = &data["extractProgress"];
                    progress(
                        "parsing",
                        count(&p["extractedPages"]),
                        count(&p["totalPages"]),
                    );
                }
                _ => progress("queued", 0, 0),
            }
            poller.wait().await?;
        };

        progress("download", 0, 0);
        let lines = download("PaddleOCR", &self.http, &json_url).await?;
        markdown_from_jsonl(&String::from_utf8_lossy(&lines))
    }

    /// Checks the key by asking for a job that does not exist: a rejected
    /// key gives 401/403, an accepted one "not found".
    pub async fn check_key(&self) -> EResult<()> {
        let request = self
            .http
            .get(format!("{}/{PROBE_JOB_ID}", self.jobs_url()))
            .bearer_auth(&self.token);
        match send_json("PaddleOCR", request).await {
            Ok(_) | Err(EnhancedError::Api(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Page Markdown from PaddleOCR's JSONL result, one page per
/// `result.layoutParsingResults` entry.
fn markdown_from_jsonl(text: &str) -> EResult<String> {
    let mut pages = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_str(line)
            .map_err(|e| EnhancedError::Api(format!("PaddleOCR 结果无法识别：{e}")))?;
        if let Some(results) = v["result"]["layoutParsingResults"].as_array() {
            pages.extend(
                results
                    .iter()
                    .filter_map(|r| r["markdown"]["text"].as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string),
            );
        }
    }
    Ok(pages.join("\n\n"))
}

/// A `multipart/form-data` body: `(content type, body)`.
fn multipart(fields: &[(&str, &str)], file: (&str, &str, &[u8])) -> (String, Vec<u8>) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let boundary = format!("----AutoPassDoc{nanos:x}");
    let quote = |s: &str| s.replace(['"', '\r', '\n'], "_");
    let mut body = Vec::with_capacity(file.2.len() + 1024);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{value}\r\n",
                quote(name)
            )
            .as_bytes(),
        );
    }
    let (name, file_name, bytes) = file;
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
            quote(name),
            quote(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// A configured online parser.
#[derive(Debug, Clone)]
pub enum EnhancedParser {
    Mineru(MineruClient),
    Paddle(PaddleClient),
}

impl EnhancedParser {
    pub fn kind(&self) -> ParserKind {
        match self {
            EnhancedParser::Mineru(_) => ParserKind::Mineru,
            EnhancedParser::Paddle(_) => ParserKind::Paddleocr,
        }
    }

    pub fn max_bytes(&self) -> u64 {
        match self {
            EnhancedParser::Mineru(c) => c.max_bytes,
            EnhancedParser::Paddle(c) => c.max_bytes,
        }
    }

    pub async fn parse(
        &self,
        file_name: &str,
        bytes: Vec<u8>,
        progress: Progress<'_>,
    ) -> EResult<String> {
        match self {
            EnhancedParser::Mineru(c) => c.parse(file_name, bytes, progress).await,
            EnhancedParser::Paddle(c) => c.parse(file_name, bytes, progress).await,
        }
    }

    pub async fn check_key(&self) -> EResult<()> {
        match self {
            EnhancedParser::Mineru(c) => c.check_key().await,
            EnhancedParser::Paddle(c) => c.check_key().await,
        }
    }
}

/// Remaining quota of a parsing service. Neither service reports it yet.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParserQuota {
    pub remaining: f64,
    pub total: Option<f64>,
    /// e.g. "页".
    pub unit: String,
}

/// Outcome of the settings page's "测试" button.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParserTest {
    pub ok: bool,
    pub message: String,
    /// Always `None`: neither service offers a quota query.
    pub quota: Option<ParserQuota>,
    pub quota_note: String,
    /// Where to check usage and quota.
    pub console_url: String,
}

/// A parsing service as the settings page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParserInfo {
    pub kind: ParserKind,
    pub name: String,
    pub has_key: bool,
    /// Service home page, to sign up.
    pub site_url: String,
    /// Where to create the key.
    pub key_url: String,
    pub docs_url: String,
    pub console_url: String,
    pub quota_note: String,
}

struct Links {
    site: &'static str,
    key: &'static str,
    docs: &'static str,
    console: &'static str,
}

fn links(kind: ParserKind) -> Links {
    match kind {
        ParserKind::Paddleocr => Links {
            site: "https://aistudio.baidu.com/paddleocr",
            key: "https://aistudio.baidu.com/account/accessToken",
            docs: "https://ai.baidu.com/ai-doc/AISTUDIO/Kmfl2ycs0",
            console: "https://aistudio.baidu.com/account/accessToken",
        },
        _ => Links {
            site: "https://mineru.net",
            key: "https://mineru.net/apiManage/token",
            docs: "https://mineru.net/apiManage/docs",
            console: "https://mineru.net/apiManage/token",
        },
    }
}

fn online(kind: ParserKind) -> Result<&'static str> {
    kind.secret_name()
        .ok_or_else(|| Error::Invalid("普通模式不需要 Key".into()))
}

impl Core {
    /// Service addresses and waits for enhanced parsing (tests shorten them).
    pub fn set_enhanced_options(&self, options: EnhancedOptions) {
        *self.enhanced.lock().unwrap() = options;
    }

    /// The online parsing services with their links and whether a key is saved.
    pub fn parser_infos(&self) -> Result<Vec<ParserInfo>> {
        [ParserKind::Mineru, ParserKind::Paddleocr]
            .into_iter()
            .map(|kind| self.parser_info(kind))
            .collect()
    }

    fn parser_info(&self, kind: ParserKind) -> Result<ParserInfo> {
        let l = links(kind);
        Ok(ParserInfo {
            kind,
            name: kind.label().into(),
            has_key: self.has_parser_key(kind)?,
            site_url: l.site.into(),
            key_url: l.key.into(),
            docs_url: l.docs.into(),
            console_url: l.console.into(),
            quota_note: NO_QUOTA_NOTE.into(),
        })
    }

    /// Saves a parsing service's key in the secret store.
    pub fn set_parser_key(&self, kind: ParserKind, key: &str) -> Result<ParserInfo> {
        let name = online(kind)?;
        let key = key.trim();
        if key.is_empty() {
            return Err(Error::Invalid("Key 不能为空".into()));
        }
        self.secrets().set(name, key)?;
        self.parser_info(kind)
    }

    pub fn clear_parser_key(&self, kind: ParserKind) -> Result<()> {
        self.secrets().delete(online(kind)?)
    }

    pub fn has_parser_key(&self, kind: ParserKind) -> Result<bool> {
        Ok(match kind.secret_name() {
            Some(name) => self.secrets().get(name)?.is_some_and(|k| !k.is_empty()),
            None => false,
        })
    }

    fn parser_client(&self, kind: ParserKind, key: &str) -> Result<EnhancedParser> {
        let settings = self.settings()?.kb;
        let options = self.enhanced.lock().unwrap().clone();
        let http = http_client()?;
        Ok(match kind {
            ParserKind::Builtin => return Err(Error::Invalid("普通模式不需要连接测试".into())),
            ParserKind::Mineru => EnhancedParser::Mineru(MineruClient {
                base_url: options.mineru_base_url,
                poll: options.poll,
                max_bytes: options.max_bytes,
                ocr: settings.mineru_ocr,
                formula: settings.mineru_formula,
                ..MineruClient::new(http, key, &settings.mineru_model)
            }),
            ParserKind::Paddleocr => EnhancedParser::Paddle(PaddleClient {
                poll: options.poll,
                max_bytes: options.max_bytes,
                ..PaddleClient::new(
                    http,
                    &settings.paddleocr_base_url,
                    key,
                    &settings.paddleocr_model,
                )
            }),
        })
    }

    /// The online parser for imports: the one chosen in the settings, when
    /// its key is saved; `None` means the built-in parsers.
    pub fn enhanced_parser(&self) -> Result<Option<EnhancedParser>> {
        let kind = self.settings()?.kb.parser;
        let Some(name) = kind.secret_name() else {
            return Ok(None);
        };
        match self.secrets().get(name)?.filter(|k| !k.trim().is_empty()) {
            Some(key) => Ok(Some(self.parser_client(kind, &key)?)),
            None => Ok(None),
        }
    }

    /// Tests a service's key: `key` when given (not saved yet), else the
    /// saved one.
    pub async fn test_parser(&self, kind: ParserKind, key: Option<&str>) -> Result<ParserTest> {
        let name = online(kind)?;
        let key = match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => Some(k.to_string()),
            None => self.secrets().get(name)?,
        };
        let l = links(kind);
        let result = |ok: bool, message: String| ParserTest {
            ok,
            message,
            quota: None,
            quota_note: NO_QUOTA_NOTE.into(),
            console_url: l.console.into(),
        };
        let Some(key) = key else {
            return Ok(result(false, "请先填写 Key".into()));
        };
        let client = self.parser_client(kind, &key)?;
        Ok(match client.check_key().await {
            Ok(()) => result(true, format!("连接正常，{} Key 有效", kind.label())),
            Err(EnhancedError::InvalidKey(m)) => result(false, format!("Key 无效：{m}")),
            Err(EnhancedError::Network(m)) => {
                result(false, format!("无法连接 {}：{m}", kind.label()))
            }
            Err(e) => result(false, e.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_body_has_fields_and_file() {
        let (ct, body) = multipart(&[("model", "M")], ("file", "a\"b.pdf", b"%PDF"));
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("name=\"model\"\r\n\r\nM\r\n"));
        assert!(text.contains("filename=\"a_b.pdf\""));
        assert!(text.ends_with(&format!("\r\n%PDF\r\n--{boundary}--\r\n")));
    }

    #[test]
    fn joins_jsonl_pages() {
        let jsonl = r#"{"result":{"layoutParsingResults":[{"markdown":{"text":"第一页"}},{"markdown":{"text":"第二页"}}]}}

{"result":{"layoutParsingResults":[{"markdown":{"text":" 第三页 "}}]}}"#;
        assert_eq!(
            markdown_from_jsonl(jsonl).unwrap(),
            "第一页\n\n第二页\n\n第三页"
        );
        assert!(markdown_from_jsonl("not json").is_err());
    }

    #[test]
    fn codes_and_messages() {
        assert!(code_ok(&json!(0)) && code_ok(&json!("0")) && code_ok(&Value::Null));
        assert!(!code_ok(&json!(-500)) && !code_ok(&json!("A0202")));
        let body = json!({"errorMsg": "job not found"});
        assert_eq!(message_of(Some(&body), ""), "job not found");
        assert_eq!(message_of(None, " oops "), "oops");
    }
}
