//! HTTP client shared by all adapters: auth, retries and error mapping.

use std::time::Duration;

use reqwest::Method;
use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result, truncate};
use crate::provider::{Provider, ProviderKind};

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Tunables for [`Client`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// TCP/TLS connect timeout. Default 15 s.
    pub connect_timeout: Duration,
    /// Whole-request timeout. Default 300 s.
    pub request_timeout: Duration,
    /// Attempts for 429 / 5xx responses, including the first. Default 3.
    pub max_attempts: u32,
    /// First backoff delay; doubled on each retry. Default 1 s.
    pub retry_base_delay: Duration,
    /// Largest `Retry-After` honoured; a longer one fails immediately. Default 30 s.
    pub max_retry_after: Duration,
    /// Texts per embeddings request. Default 32.
    pub embed_batch_size: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(300),
            max_attempts: 3,
            retry_base_delay: Duration::from_secs(1),
            max_retry_after: Duration::from_secs(30),
            embed_batch_size: 32,
        }
    }
}

/// Async client for all model roles. Cheap to clone (shares the connection pool).
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    config: ClientConfig,
}

impl Client {
    /// Creates a client with [`ClientConfig::default`].
    pub fn new() -> Result<Self> {
        Self::with_config(ClientConfig::default())
    }

    /// Creates a client with custom timeouts, retry and batching settings.
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent(concat!("AutoPassDoc/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::InvalidConfig(format!("HTTP 客户端初始化失败：{e}")))?;
        Ok(Self { http, config })
    }

    /// The active configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub(crate) async fn get_json(&self, p: &Provider, url: &str) -> Result<Value> {
        Ok(self.execute(p, Method::GET, url, None).await?)
    }

    pub(crate) async fn post_json<T: Serialize + ?Sized>(
        &self,
        p: &Provider,
        url: &str,
        body: &T,
    ) -> Result<Value> {
        Ok(self.post_json_raw(p, url, body).await?)
    }

    /// Like [`Self::post_json`] but keeps the raw error body for inspection.
    pub(crate) async fn post_json_raw<T: Serialize + ?Sized>(
        &self,
        p: &Provider,
        url: &str,
        body: &T,
    ) -> Result<Value, Failure> {
        let body = serde_json::to_vec(body)
            .map_err(|e| Error::InvalidConfig(format!("请求序列化失败：{e}")))?;
        self.execute(p, Method::POST, url, Some(body)).await
    }

    /// Sends a request, retrying 429 / 5xx with exponential backoff, and parses
    /// the JSON body of a successful response.
    async fn execute(
        &self,
        p: &Provider,
        method: Method,
        url: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Value, Failure> {
        let endpoint = endpoint_label(method.as_str(), url);
        let mut attempt = 1;
        loop {
            let mut req = self.http.request(method.clone(), url);
            req = match (p.kind, p.key()) {
                (ProviderKind::Anthropic, key) => {
                    let req = req.header("anthropic-version", ANTHROPIC_VERSION);
                    match key {
                        Some(k) => req.header("x-api-key", k),
                        None => req,
                    }
                }
                (_, Some(k)) => req.bearer_auth(k),
                (_, None) => req,
            };
            if let Some(body) = &body {
                req = req
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone());
            }

            let resp = req
                .send()
                .await
                .map_err(|e| network_error(&endpoint, &e, p.key()))?;
            let status = resp.status();
            if status.is_success() {
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| network_error(&endpoint, &e, p.key()))?;
                return serde_json::from_slice(&bytes).map_err(|e| {
                    let text = redact(&String::from_utf8_lossy(&bytes), p.key());
                    let detail = format!("不是有效的 JSON（{e}）：{}", truncate(text.trim(), 200));
                    Error::decode(&endpoint, detail).into()
                });
            }

            let retry_after = parse_retry_after(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            // An empty answer is left to the caller, which retries it with a larger output limit.
            let retryable = (status.as_u16() == 429 || status.is_server_error())
                && !text.to_lowercase().contains("empty response");
            if retryable && attempt < self.config.max_attempts {
                let backoff = self.config.retry_base_delay * 2u32.saturating_pow(attempt - 1);
                let delay = match retry_after {
                    Some(d) if d > self.config.max_retry_after => None,
                    Some(d) => Some(d),
                    None => Some(backoff),
                };
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
            }
            return Err(Failure::Status {
                status: status.as_u16(),
                endpoint,
                body: redact(&text, p.key()),
            });
        }
    }
}

/// A failed request, keeping the raw (redacted) body of HTTP errors so callers
/// can inspect it before it becomes a user-facing [`Error`].
#[derive(Debug)]
pub(crate) enum Failure {
    Status {
        status: u16,
        endpoint: String,
        body: String,
    },
    Other(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Other(e)
    }
}

impl From<Failure> for Error {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Status {
                status,
                endpoint,
                body,
            } => Error::http(status, &endpoint, &body),
            Failure::Other(e) => e,
        }
    }
}

/// `POST /v1/chat/completions`: method plus URL path, without host or query.
pub(crate) fn endpoint_label(method: &str, url: &str) -> String {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    let path = url[after_scheme..]
        .find('/')
        .map_or("/", |i| &url[after_scheme + i..]);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    format!("{method} {path}")
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let secs: f64 = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

fn network_error(endpoint: &str, e: &reqwest::Error, key: Option<&str>) -> Error {
    // The root cause ("Connection refused", "invalid peer certificate"...) is the useful part.
    let mut root: &dyn std::error::Error = e;
    while let Some(s) = root.source() {
        root = s;
    }
    let detail = redact(&root.to_string(), key);
    Error::Network(if e.is_timeout() {
        format!("请求超时（{endpoint}），请检查网络或稍后重试：{detail}")
    } else if e.is_connect() {
        format!("无法连接服务器（{endpoint}），请检查 Base URL 和网络：{detail}")
    } else {
        format!("网络错误（{endpoint}）：{detail}")
    })
}

/// Replaces every occurrence of the API key with `***`.
pub(crate) fn redact(text: &str, key: Option<&str>) -> String {
    match key {
        Some(k) if !k.is_empty() => text.replace(k, "***"),
        _ => text.to_string(),
    }
}
