//! Update check (S-2): the latest GitHub release compared with the running
//! version.

use std::cmp::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The latest published (non-draft, non-prerelease) release of the app.
pub const RELEASES_URL: &str = "https://api.github.com/repos/cyDione/AutoPassDoc/releases/latest";

const NO_RELEASE: &str = "还没有发布版";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAsset {
    pub name: String,
    /// Direct download link.
    pub url: String,
    /// Bytes.
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    /// Version of the latest release without the leading `v`; `None` when
    /// nothing has been released yet.
    pub latest: Option<String>,
    pub has_update: bool,
    /// Release title.
    pub name: Option<String>,
    /// Release notes (Markdown).
    pub notes: String,
    /// The release page, to open in the browser.
    pub url: Option<String>,
    /// RFC 3339.
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Asks `api_url` (normally [`RELEASES_URL`]) for the latest release and
/// compares it with `current`.
pub async fn check_update(current: &str, api_url: &str) -> Result<UpdateInfo> {
    let failed = |e: String| Error::Invalid(format!("检查更新失败：{e}"));
    let client = reqwest::Client::builder()
        .user_agent("AutoPassDoc")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let response = client
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            failed(if e.is_timeout() {
                "连接 GitHub 超时，请检查网络".into()
            } else {
                format!("无法连接 GitHub（{e}）")
            })
        })?;
    let current = current.trim().trim_start_matches(['v', 'V']).to_string();
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateInfo {
            current,
            latest: None,
            has_update: false,
            name: None,
            notes: NO_RELEASE.into(),
            url: None,
            published_at: None,
            assets: Vec::new(),
        });
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(failed("GitHub 限制了访问次数，请稍后再试".into()));
    }
    if !status.is_success() {
        return Err(failed(format!("GitHub 返回 {status}")));
    }
    let release: Release = response
        .json()
        .await
        .map_err(|e| failed(format!("无法读取发布信息（{e}）")))?;
    let latest = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    Ok(UpdateInfo {
        has_update: compare_versions(&latest, &current) == Ordering::Greater,
        current,
        latest: Some(latest),
        name: release.name.filter(|n| !n.trim().is_empty()),
        notes: release.body.unwrap_or_default(),
        url: release.html_url,
        published_at: release.published_at,
        assets: release
            .assets
            .into_iter()
            .map(|a| ReleaseAsset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    })
}

/// Compares versions like `1.2.3`, `v1.2`, `1.2.3-beta.2` (semantic
/// versioning precedence; build metadata after `+` is ignored, missing
/// numbers count as 0, a pre-release sorts before its release).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a_core, a_pre) = split_version(a);
    let (b_core, b_pre) = split_version(b);
    let len = a_core.len().max(b_core.len());
    for i in 0..len {
        let (x, y) = (
            a_core.get(i).copied().unwrap_or(0),
            b_core.get(i).copied().unwrap_or(0),
        );
        match x.cmp(&y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => compare_pre(x, y),
    }
}

fn split_version(v: &str) -> (Vec<u64>, Option<&str>) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split_once('+').map_or(v, |(v, _)| v);
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let numbers = core
        .split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect();
    (numbers, pre)
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn compares_versions() {
        use Ordering::*;
        assert_eq!(compare_versions("0.2.0", "0.1.0"), Greater);
        assert_eq!(compare_versions("v0.1.0", "0.1"), Equal);
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Greater);
        assert_eq!(compare_versions("1.0.0-beta.1", "1.0.0"), Less);
        assert_eq!(compare_versions("1.0.0-beta.2", "1.0.0-beta.10"), Less);
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0-alpha.3"), Greater);
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0-rc.1.1"), Less);
        assert_eq!(compare_versions("1.0.1+build5", "1.0.1"), Equal);
    }

    const API: &str = "/repos/cyDione/AutoPassDoc/releases/latest";

    async fn server(status: u16, body: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(API))
            .and(header("user-agent", "AutoPassDoc"))
            .and(header("accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn reports_a_newer_release() {
        let server = server(
            200,
            json!({
                "tag_name": "v0.2.0",
                "name": "AutoPassDoc 0.2.0",
                "body": "### 新增\n- 数据备份",
                "html_url": "https://github.com/cyDione/AutoPassDoc/releases/tag/v0.2.0",
                "published_at": "2026-10-10T08:00:00Z",
                "assets": [{
                    "name": "AutoPassDoc_0.2.0_x64-setup.exe",
                    "browser_download_url": "https://github.com/cyDione/AutoPassDoc/releases/download/v0.2.0/AutoPassDoc_0.2.0_x64-setup.exe",
                    "size": 12345
                }]
            }),
        )
        .await;
        let url = format!("{}{API}", server.uri());
        let info = check_update("0.1.0", &url).await.unwrap();
        assert!(info.has_update);
        assert_eq!(info.latest.as_deref(), Some("0.2.0"));
        assert_eq!(info.current, "0.1.0");
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].size, 12345);
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["hasUpdate"], true);
        assert_eq!(v["publishedAt"], "2026-10-10T08:00:00Z");
        assert!(v["assets"][0]["url"].as_str().unwrap().ends_with(".exe"));

        let info = check_update("v0.2.0", &url).await.unwrap();
        assert!(!info.has_update);
    }

    #[tokio::test]
    async fn no_release_yet_and_errors() {
        let server = server(404, json!({"message": "Not Found"})).await;
        let info = check_update("0.1.0", &format!("{}{API}", server.uri()))
            .await
            .unwrap();
        assert!(!info.has_update);
        assert_eq!(info.latest, None);
        assert_eq!(info.notes, NO_RELEASE);

        let server = server_status(403).await;
        let err = check_update("0.1.0", &format!("{}{API}", server.uri()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("限制"), "{err}");
        let server = server_status(500).await;
        let err = check_update("0.1.0", &format!("{}{API}", server.uri()))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("检查更新失败"), "{err}");
    }

    async fn server_status(status: u16) -> MockServer {
        server(status, json!({"message": "x"})).await
    }
}
