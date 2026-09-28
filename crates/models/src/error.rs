//! Error type shared by every adapter. Messages are user-facing (Chinese) and
//! never contain the API key.

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors returned by [`crate::Client`]. `Display` gives a message suitable for
/// showing to the user, including the endpoint that failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The server answered with a non-success HTTP status.
    #[error("{message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// User-facing message with the endpoint and a truncated response body.
        message: String,
    },
    /// The request never got an HTTP answer (DNS, connect, TLS, timeout...).
    #[error("{0}")]
    Network(String),
    /// The response could not be understood.
    #[error("{0}")]
    Decode(String),
    /// The provider or request is misconfigured (bad base URL, unsupported role...).
    #[error("{0}")]
    InvalidConfig(String),
}

impl Error {
    /// HTTP status code for [`Error::Http`], `None` otherwise.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub(crate) fn decode(endpoint: &str, detail: impl std::fmt::Display) -> Self {
        Error::Decode(format!("响应解析失败（{endpoint}）：{detail}"))
    }

    pub(crate) fn http(status: u16, endpoint: &str, body: &str) -> Self {
        let summary = status_summary(status);
        let body = truncate(body.trim(), MAX_BODY_CHARS);
        let message = if body.is_empty() {
            format!("{summary}。接口：{endpoint}")
        } else {
            format!("{summary}。接口：{endpoint}。服务返回：{body}")
        };
        Error::Http { status, message }
    }
}

const MAX_BODY_CHARS: usize = 500;

fn status_summary(status: u16) -> String {
    match status {
        400 => "请求参数错误（400）".into(),
        401 => "认证失败（401），请检查 API Key".into(),
        402 => "账户余额不足（402），请检查服务商账户".into(),
        403 => "无权访问（403），请检查 API Key 权限或账户状态".into(),
        404 => "模型不存在（404），请检查模型名称和 Base URL".into(),
        408 => "请求超时（408）".into(),
        413 => "请求内容过大（413），请减少输入".into(),
        422 => "请求参数无效（422）".into(),
        429 => "请求过多（429），请稍后重试或检查额度".into(),
        500..=599 => format!("服务端错误（{status}），请稍后重试"),
        _ => format!("请求失败（{status}）"),
    }
}

/// Truncates to `max` characters, appending an ellipsis when cut.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}
