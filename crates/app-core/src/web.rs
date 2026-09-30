//! Looking things up on the web for "【待补充…】" gaps and cited documents:
//! the chat model's own web search first, then a search engine restricted to
//! whitelisted government and standards sites fetched from this machine.
//! Files found this way can be downloaded into the knowledge base.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use models::{ChatRequest, Message, WebSearch};
use regex::Regex;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::{Core, RoleName};
use crate::error::{Error, Result};
use crate::knowledge::ImportResult;

/// Largest page read when looking for attachments.
const MAX_PAGE_BYTES: usize = 2 << 20;
/// Largest file downloaded into the knowledge base.
const MAX_DOWNLOAD_BYTES: usize = 100 << 20;
/// Result pages scanned for attachment links.
const PAGES_TO_SCAN: usize = 3;
const MAX_RESULTS: usize = 8;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 AutoPassDoc";
/// Attachment extensions offered for download.
const FILE_TYPES: &[&str] = &[
    "pdf", "docx", "doc", "xlsx", "xls", "wps", "ofd", "txt", "md",
];
/// Extensions the knowledge base can import.
const IMPORTABLE: &[&str] = &["pdf", "docx", "txt", "md"];

/// When to search, and where.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebMode {
    /// The chat model's search, falling back to the whitelist search.
    #[default]
    Auto,
    /// Only the chat model's search.
    Model,
    /// Only the whitelist search from this machine.
    Local,
    /// No web access.
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    #[default]
    Bing,
    Baidu,
}

impl SearchEngine {
    fn label(self) -> &'static str {
        match self {
            Self::Bing => "必应",
            Self::Baidu => "百度",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WebSettings {
    pub mode: WebMode,
    /// How to switch on the chat model's search; `None` detects it from the provider.
    pub model_search: Option<WebSearch>,
    pub engine: SearchEngine,
    /// Hosts the local search may return and fetch, matched as domain suffixes.
    pub whitelist: Vec<String>,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            mode: WebMode::Auto,
            model_search: None,
            engine: SearchEngine::Bing,
            whitelist: DEFAULT_WHITELIST.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// See technical design 13.8. `gov.cn` covers every government site; the
/// rest are authoritative sites outside it, and a few named ones so users can
/// see what is included.
pub const DEFAULT_WHITELIST: &[&str] = &[
    "gov.cn",
    "www.gov.cn",
    "flk.npc.gov.cn",
    "www.npc.gov.cn",
    "www.moj.gov.cn",
    "std.samr.gov.cn",
    "openstd.samr.gov.cn",
    "hbba.sacinfo.org.cn",
    "dbba.sacinfo.org.cn",
    "www.mohurd.gov.cn",
    "www.ccsn.org.cn",
    "www.ndrc.gov.cn",
    "www.mof.gov.cn",
    "www.mee.gov.cn",
    "www.mnr.gov.cn",
    "www.mwr.gov.cn",
    "www.mot.gov.cn",
    "www.miit.gov.cn",
    "www.mem.gov.cn",
    "www.nea.gov.cn",
    "www.stats.gov.cn",
    "www.ccgp.gov.cn",
    "www.shanghai.gov.cn",
    "fgw.sh.gov.cn",
    "zjw.sh.gov.cn",
    "ghzyj.sh.gov.cn",
    "sthj.sh.gov.cn",
    "tjj.sh.gov.cn",
    "www.spcsc.sh.cn",
    "www.shcm.gov.cn",
    "www.pudong.gov.cn",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultKind {
    Page,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebResult {
    pub title: String,
    pub url: String,
    pub site: String,
    pub snippet: String,
    pub kind: ResultKind,
    /// Lowercase extension of a file result.
    pub file_type: Option<String>,
    /// The knowledge base can import it.
    pub importable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub results: Vec<WebResult>,
    /// "model" or "local".
    pub via: &'static str,
    /// What was tried and why it fell back.
    pub notes: Vec<String>,
}

/// HTTP client and what the searches have seen.
pub struct WebState {
    http: reqwest::Client,
    /// Only links shown to the user may be downloaded.
    seen: Mutex<HashSet<String>>,
    /// Disallowed path prefixes per host, from robots.txt.
    robots: Mutex<HashMap<String, Vec<String>>>,
    /// Search engine addresses; tests point them at a mock server.
    bases: Mutex<(String, String)>,
}

impl WebState {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(reqwest::header::HeaderMap::from_iter([(
                reqwest::header::ACCEPT_LANGUAGE,
                reqwest::header::HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.5"),
            )]))
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| Error::Setup(e.to_string()))?;
        Ok(Self {
            http,
            seen: Mutex::default(),
            robots: Mutex::default(),
            bases: Mutex::new(("https://cn.bing.com".into(), "https://www.baidu.com".into())),
        })
    }
}

/// True when `host` is a whitelisted domain or one of its subdomains.
pub fn allowed(host: &str, whitelist: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    whitelist.iter().any(|w| {
        let w = w
            .trim()
            .trim_start_matches("*.")
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let w = w.split(['/', ':']).next().unwrap_or_default();
        !w.is_empty() && (host == w || host.ends_with(&format!(".{w}")))
    })
}

fn file_type(url: &Url) -> Option<String> {
    let last = url.path_segments()?.next_back()?.to_ascii_lowercase();
    let ext = last.rsplit_once('.')?.1;
    FILE_TYPES.contains(&ext).then(|| ext.to_string())
}

fn result(title: &str, url: Url, snippet: &str) -> WebResult {
    let file_type = file_type(&url);
    WebResult {
        title: clean_text(title),
        site: url.host_str().unwrap_or_default().to_string(),
        snippet: clean_text(snippet),
        kind: if file_type.is_some() {
            ResultKind::File
        } else {
            ResultKind::Page
        },
        importable: file_type
            .as_deref()
            .is_some_and(|t| IMPORTABLE.contains(&t)),
        file_type,
        url: url.to_string(),
    }
}

fn http_url(s: &str) -> Option<Url> {
    Url::parse(s.trim())
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Strips tags, decodes entities and collapses whitespace.
pub fn clean_text(html: &str) -> String {
    let text = TAG.replace_all(html, "");
    SPACE
        .replace_all(&decode_entities(&text), " ")
        .trim()
        .to_string()
}

fn decode_entities(s: &str) -> String {
    static ENTITY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
    ENTITY
        .replace_all(s, |c: &regex::Captures| {
            let e = &c[1];
            let decoded = if let Some(hex) = e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(dec) = e.strip_prefix('#') {
                dec.parse().ok().and_then(char::from_u32)
            } else {
                match e {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" | "ensp" | "emsp" => Some(' '),
                    "ldquo" => Some('“'),
                    "rdquo" => Some('”'),
                    "middot" => Some('·'),
                    _ => None,
                }
            };
            decoded.map_or_else(|| c[0].to_string(), String::from)
        })
        .into_owned()
}

/// Results from a Bing results page. Bing wraps most links in its own
/// `/ck/a?…&u=a1<base64url>` click tracker; those are unwrapped here, since
/// the tracker's host is never on the whitelist.
pub fn parse_bing(html: &str) -> Vec<(String, String, String)> {
    static BLOCK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<li class="b_algo\b[^"]*"(.*?)</li>"#).unwrap());
    static LINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<h2[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap()
    });
    static SNIPPET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<p[^>]*>(.*?)</p>"#).unwrap());
    BLOCK
        .captures_iter(html)
        .filter_map(|b| {
            let block = &b[1];
            let link = LINK.captures(block)?;
            let snippet = SNIPPET
                .captures(block)
                .map(|s| s[1].to_string())
                .unwrap_or_default();
            let href = decode_entities(&link[1]);
            let href = unwrap_bing_link(&href).unwrap_or(href);
            Some((href, link[2].to_string(), snippet))
        })
        .collect()
}

/// The target of a Bing `/ck/a` click-tracking link.
pub fn unwrap_bing_link(href: &str) -> Option<String> {
    use base64::Engine as _;
    let url = Url::parse(href).ok()?;
    if !url.host_str()?.ends_with("bing.com") || url.path() != "/ck/a" {
        return None;
    }
    let (_, u) = url.query_pairs().find(|(k, _)| k == "u")?;
    let encoded = u.strip_prefix("a1")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .ok()?;
    let target = String::from_utf8(bytes).ok()?;
    http_url(&target).map(|u| u.to_string())
}

/// Results from a Baidu results page. Each result carries its real address
/// in `mu`; the title link is a Baidu redirect, used only when `mu` is missing.
pub fn parse_baidu(html: &str) -> Vec<(String, String, String)> {
    static START: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"<div[^>]*class="[^"]*\bc-container\b[^"]*"[^>]*>"#).unwrap()
    });
    static MU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bmu="([^"]+)""#).unwrap());
    static LINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<h3[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap()
    });
    static SNIPPET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<(?:span|div)[^>]*class="[^"]*(?:content-right|c-abstract|c-span-last)[^"]*"[^>]*>(.*?)</(?:span|div)>"#)
            .unwrap()
    });
    let starts: Vec<_> = START.find_iter(html).collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let end = starts.get(i + 1).map_or(html.len(), |n| n.start());
            let block = &html[m.start()..end];
            let link = LINK.captures(block)?;
            let real = MU
                .captures(m.as_str())
                .map(|c| decode_entities(&c[1]))
                .filter(|u| {
                    http_url(u)
                        .is_some_and(|u| !u.host_str().unwrap_or_default().ends_with("baidu.com"))
                });
            let snippet = SNIPPET
                .captures(block)
                .map(|s| s[1].to_string())
                .unwrap_or_default();
            Some((
                real.unwrap_or_else(|| decode_entities(&link[1])),
                link[2].to_string(),
                snippet,
            ))
        })
        .collect()
}

/// Attachment links on a page: (absolute url, link text).
pub fn attachments(page: &Url, html: &str) -> Vec<(Url, String)> {
    static A: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<a\s[^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap()
    });
    let mut seen = HashSet::new();
    A.captures_iter(html)
        .filter_map(|c| {
            let url = page.join(decode_entities(&c[1]).trim()).ok()?;
            file_type(&url)?;
            seen.insert(url.to_string())
                .then(|| (url, clean_text(&c[2])))
        })
        .collect()
}

/// Whether `text` (lowercase) mentions the query at all: a whole term, or
/// for Chinese, which has no spaces between words, any two characters in a
/// row from a term.
fn mentions(text: &str, terms: &[String]) -> bool {
    terms.iter().any(|t| {
        let chars: Vec<char> = t.chars().collect();
        text.contains(t.as_str())
            || (chars.len() > 2
                && chars
                    .windows(2)
                    .any(|w| !w[0].is_ascii() && text.contains(&w.iter().collect::<String>())))
    })
}

/// The JSON array in a model answer, tolerating code fences and chatter.
fn json_array(text: &str) -> Option<Vec<Value>> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    (end > start)
        .then(|| serde_json::from_str::<Vec<Value>>(&text[start..=end]).ok())
        .flatten()
}

const SEARCH_PROMPT: &str = "你是资料检索助手，帮助撰写政府项目报告的人查找可靠来源。请联网搜索用户要找的内容，优先政府官网、国家法律法规数据库、标准信息平台和统计部门发布的原文。

只输出一个 JSON 数组，不要输出其他文字，最多 6 条：
[{\"title\": \"网页或文件标题\", \"url\": \"实际访问到的网址\", \"snippet\": \"与所找内容相关的一两句原文摘录\"}]
url 必须是搜索中真实出现的地址，不要编造；找不到就输出 []。";

impl Core {
    /// Web settings saved by the user.
    fn web_settings(&self) -> Result<WebSettings> {
        Ok(self.settings()?.web)
    }

    /// Points the search engines somewhere else (tests).
    #[doc(hidden)]
    pub fn set_search_bases(&self, bing: &str, baidu: &str) {
        *self.web.bases.lock().unwrap() = (
            bing.trim_end_matches('/').into(),
            baidu.trim_end_matches('/').into(),
        );
    }

    /// Searches the web for `query`, by the chat model when it can, else
    /// through the whitelist.
    pub async fn web_search(&self, query: &str) -> Result<SearchOutcome> {
        let query = query.trim();
        if query.is_empty() {
            return Err(Error::Invalid("请输入要查找的内容".into()));
        }
        let settings = self.web_settings()?;
        let mut notes = Vec::new();
        if settings.mode == WebMode::Off {
            return Err(Error::Invalid("联网搜索已在设置中关闭".into()));
        }
        if matches!(settings.mode, WebMode::Auto | WebMode::Model) {
            match self.model_search(query, &settings).await {
                Ok(results) if !results.is_empty() => {
                    self.remember(&results);
                    return Ok(SearchOutcome {
                        results,
                        via: "model",
                        notes,
                    });
                }
                Ok(_) => notes.push("大语言模型联网搜索没有找到结果".into()),
                Err(e) => notes.push(e.to_string()),
            }
            if settings.mode == WebMode::Model {
                return Ok(SearchOutcome {
                    results: Vec::new(),
                    via: "model",
                    notes,
                });
            }
            notes.push("已改用本机白名单搜索".into());
        }
        let results = self.local_search(query, &settings, &mut notes).await?;
        self.remember(&results);
        Ok(SearchOutcome {
            results,
            via: "local",
            notes,
        })
    }

    fn remember(&self, results: &[WebResult]) {
        self.web
            .seen
            .lock()
            .unwrap()
            .extend(results.iter().map(|r| r.url.clone()));
    }

    async fn model_search(&self, query: &str, settings: &WebSettings) -> Result<Vec<WebResult>> {
        let chat = self.require(RoleName::Chat)?;
        let kind = settings
            .model_search
            .or_else(|| WebSearch::detect(&chat.provider))
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "当前大语言模型服务（{}）不支持联网搜索",
                    chat.provider.name
                ))
            })?;
        let mut req = ChatRequest::new(
            chat.model.clone(),
            vec![
                Message::system(SEARCH_PROMPT),
                Message::user(format!("要查找的内容：{query}")),
            ],
        );
        req.profile = chat.profile.clone();
        req.max_tokens = Some(2048.min(chat.profile.max_output_tokens.max(1024)));
        req.temperature = Some(0.1);
        req.web_search = Some(kind);
        let response = self
            .client()
            .chat(&chat.provider, &req)
            .await
            .map_err(|e| Error::Invalid(format!("大语言模型联网搜索失败：{e}")))?;
        let cited: HashSet<&str> = response.citations.iter().map(|c| c.url.as_str()).collect();
        let mut out: Vec<WebResult> = Vec::new();
        let mut push = |r: WebResult| {
            if !out.iter().any(|o| o.url == r.url) {
                out.push(r);
            }
        };
        for item in json_array(&response.content).unwrap_or_default() {
            let Some(url) = item.get("url").and_then(Value::as_str).and_then(http_url) else {
                continue;
            };
            // Links the provider did not report may be made up.
            if !cited.is_empty()
                && !cited.contains(url.as_str())
                && !cited.contains(url.as_str().trim_end_matches('/'))
            {
                continue;
            }
            let text = |k| item.get(k).and_then(Value::as_str).unwrap_or_default();
            push(result(text("title"), url, text("snippet")));
        }
        for c in &response.citations {
            if let Some(url) = http_url(&c.url) {
                push(result(&c.title, url, &c.snippet));
            }
        }
        out.truncate(MAX_RESULTS);
        Ok(out)
    }

    async fn local_search(
        &self,
        query: &str,
        settings: &WebSettings,
        notes: &mut Vec<String>,
    ) -> Result<Vec<WebResult>> {
        let whitelist = &settings.whitelist;
        if whitelist.iter().all(|w| w.trim().is_empty()) {
            return Err(Error::Invalid(
                "白名单为空，请在设置 › 联网搜索中添加网站".into(),
            ));
        }
        // `gov.cn` already covers the named government sites.
        let covers_gov = whitelist.iter().any(|w| w.trim() == "gov.cn");
        let sites: Vec<&str> = whitelist
            .iter()
            .map(|w| w.trim())
            .filter(|w| !w.is_empty() && !(covers_gov && w.ends_with(".gov.cn")))
            .take(8)
            .collect();
        let site_filter = sites
            .iter()
            .map(|s| format!("site:{s}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let filtered = if sites.len() > 1 {
            format!("{query} ({site_filter})")
        } else {
            format!("{query} {site_filter}")
        };
        // Search engines often ignore or mishandle a long `site:` group, so
        // the bare query runs too and its results are held to the whitelist
        // here. If the chosen engine finds nothing, the other one is tried.
        let engines = match settings.engine {
            SearchEngine::Bing => [SearchEngine::Bing, SearchEngine::Baidu],
            SearchEngine::Baidu => [SearchEngine::Baidu, SearchEngine::Bing],
        };
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut results: Vec<WebResult> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for engine in engines {
            for q in [filtered.as_str(), query] {
                if results.len() >= MAX_RESULTS / 2 {
                    break;
                }
                let (url, found) = match self.engine_search(engine, q).await {
                    Ok(r) => r,
                    Err(e) => {
                        notes.push(format!("{}：{e}", engine.label()));
                        break;
                    }
                };
                if found.is_empty() {
                    notes.push(format!(
                        "{}没有返回可识别的结果（可能要求人机验证）",
                        engine.label()
                    ));
                    break;
                }
                let total = found.len();
                for (link, title, snippet) in found {
                    let Some(u) = url
                        .join(&link)
                        .ok()
                        .filter(|u| matches!(u.scheme(), "http" | "https"))
                    else {
                        continue;
                    };
                    // Engines that suspect a bot serve unrelated results.
                    let text = format!("{title}{snippet}").to_lowercase();
                    if u.host_str().is_some_and(|h| allowed(h, whitelist))
                        && mentions(&text, &terms)
                        && seen.insert(u.to_string())
                    {
                        results.push(result(&title, u, &snippet));
                    }
                }
                if results.is_empty() && q == query {
                    notes.push(format!(
                        "{}找到 {total} 条结果，但都不在白名单网站中",
                        engine.label()
                    ));
                }
            }
            if !results.is_empty() {
                break;
            }
        }
        results.truncate(MAX_RESULTS);
        if results.is_empty() {
            notes.push("白名单网站中没有找到结果".into());
            return Ok(results);
        }

        // Pages often link the document itself as an attachment.
        let pages: Vec<Url> = results
            .iter()
            .filter(|r| r.kind == ResultKind::Page)
            .filter_map(|r| Url::parse(&r.url).ok())
            .take(PAGES_TO_SCAN)
            .collect();
        for page in pages {
            if !self.robots_allow(&page).await {
                continue;
            }
            let Ok(html) = self.fetch_text(&page, MAX_PAGE_BYTES).await else {
                continue;
            };
            let parent = results
                .iter()
                .find(|r| r.url == page.as_str())
                .map(|r| r.title.clone())
                .unwrap_or_default();
            for (file, text) in attachments(&page, &html) {
                if !file.host_str().is_some_and(|h| allowed(h, whitelist))
                    || seen.contains(file.as_str())
                {
                    continue;
                }
                seen.insert(file.to_string());
                let title = if text.is_empty() {
                    file.path()
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_string()
                } else {
                    text
                };
                results.push(result(&title, file, &format!("附件，来自：{parent}")));
            }
        }
        Ok(results)
    }

    /// One results page from `engine`: the page address and its
    /// (link, title, snippet) entries.
    async fn engine_search(
        &self,
        engine: SearchEngine,
        q: &str,
    ) -> Result<(Url, Vec<(String, String, String)>)> {
        let (bing, baidu) = self.web.bases.lock().unwrap().clone();
        let bad = |e: String| Error::Invalid(e.to_string());
        match engine {
            SearchEngine::Bing => {
                let url = Url::parse_with_params(
                    &format!("{bing}/search"),
                    &[("q", q), ("ensearch", "0"), ("setlang", "zh-Hans")],
                )
                .map_err(|e| bad(e.to_string()))?;
                let (html, landed) = self.fetch_page(&url).await?;
                let mut found = parse_bing(&html);
                // Outside mainland China cn.bing.com sends searches to the
                // www.bing.com home page, which has no results.
                if found.is_empty() && landed.path() != "/search" {
                    let mut retry = url.clone();
                    retry
                        .set_host(Some("www.bing.com"))
                        .map_err(|e| bad(e.to_string()))?;
                    retry.query_pairs_mut().append_pair("mkt", "zh-CN");
                    found = parse_bing(&self.fetch_page(&retry).await?.0);
                }
                Ok((url, found))
            }
            SearchEngine::Baidu => {
                let url = Url::parse_with_params(
                    &format!("{baidu}/s"),
                    &[("wd", q), ("rn", "20"), ("ie", "utf-8")],
                )
                .map_err(|e| bad(e.to_string()))?;
                let (html, _) = self.fetch_page(&url).await?;
                let mut found = parse_baidu(&html);
                for item in &mut found {
                    let tracked = Url::parse(&item.0)
                        .ok()
                        .is_some_and(|u| u.host_str().is_some_and(|h| h.ends_with("baidu.com")));
                    if tracked && let Some(target) = self.resolve_redirect(&item.0).await {
                        item.0 = target;
                    }
                }
                Ok((url, found))
            }
        }
    }

    /// The redirect target of a search engine's tracking link.
    async fn resolve_redirect(&self, link: &str) -> Option<String> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()
            .ok()?;
        let r = client.get(link).send().await.ok()?;
        r.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    async fn fetch_bytes(
        &self,
        url: &Url,
        limit: usize,
    ) -> Result<(Vec<u8>, reqwest::header::HeaderMap, Url)> {
        let fail = |e: String| {
            Error::Invalid(format!(
                "无法访问 {}：{e}",
                url.host_str().unwrap_or_default()
            ))
        };
        let mut response = self
            .web
            .http
            .get(url.clone())
            .send()
            .await
            .map_err(|e| fail(e.without_url().to_string()))?;
        if !response.status().is_success() {
            return Err(fail(format!("HTTP {}", response.status().as_u16())));
        }
        if response
            .content_length()
            .is_some_and(|n| n as usize > limit)
        {
            return Err(fail(format!("文件超过 {} MB", limit >> 20)));
        }
        let headers = response.headers().clone();
        let landed = response.url().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| fail(e.without_url().to_string()))?
        {
            body.extend_from_slice(&chunk);
            if body.len() > limit {
                return Err(fail(format!("文件超过 {} MB", limit >> 20)));
            }
        }
        Ok((body, headers, landed))
    }

    async fn fetch_text(&self, url: &Url, limit: usize) -> Result<String> {
        Ok(self.fetch_text_at(url, limit).await?.0)
    }

    /// A search results page and the address it ended up at after redirects.
    async fn fetch_page(&self, url: &Url) -> Result<(String, Url)> {
        self.fetch_text_at(url, MAX_PAGE_BYTES).await
    }

    async fn fetch_text_at(&self, url: &Url, limit: usize) -> Result<(String, Url)> {
        let (bytes, headers, landed) = self.fetch_bytes(url, limit).await?;
        let declared = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_ascii_lowercase();
        let gbk = ["gbk", "gb2312", "gb18030"].iter().any(|c| {
            declared.contains(c)
                || head.contains(&format!("charset={c}"))
                || head.contains(&format!("charset=\"{c}"))
        });
        let text = if gbk {
            encoding_rs::GB18030.decode(&bytes).0.into_owned()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        Ok((text, landed))
    }

    /// Whether robots.txt lets any crawler fetch `url`; unreachable robots files allow.
    async fn robots_allow(&self, url: &Url) -> bool {
        let Some(host) = url.host_str().map(str::to_string) else {
            return false;
        };
        let cached = self.web.robots.lock().unwrap().get(&host).cloned();
        let rules = match cached {
            Some(r) => r,
            None => {
                let rules = match url.join("/robots.txt") {
                    Ok(robots) => match self.fetch_text(&robots, 256 << 10).await {
                        Ok(text) => disallowed(&text),
                        Err(_) => Vec::new(),
                    },
                    Err(_) => Vec::new(),
                };
                self.web.robots.lock().unwrap().insert(host, rules.clone());
                rules
            }
        };
        !rules.iter().any(|p| url.path().starts_with(p.as_str()))
    }

    /// Downloads a file found by [`Core::web_search`] and imports it into the
    /// knowledge base.
    pub async fn web_download_to_kb(&self, url: &str) -> Result<ImportResult> {
        if !self.web.seen.lock().unwrap().contains(url) {
            return Err(Error::Invalid("只能下载搜索结果中的文件".into()));
        }
        let parsed = http_url(url).ok_or_else(|| Error::Invalid("链接无效".into()))?;
        let (bytes, headers, _) = self.fetch_bytes(&parsed, MAX_DOWNLOAD_BYTES).await?;
        let name = download_name(&parsed, &headers, &bytes);
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !IMPORTABLE.contains(&ext.as_str()) {
            return Err(Error::Invalid(format!(
                "知识库暂不支持 .{ext} 文件，请用浏览器打开后另存为 Word 或 PDF"
            )));
        }
        let dir = self.data_dir().join("downloads");
        std::fs::create_dir_all(&dir)?;
        let path = unique_path(dir.join(&name));
        std::fs::write(&path, &bytes)?;
        let mut reports = self
            .kb_import(std::slice::from_ref(&path), |_, _, _| {})
            .await;
        reports
            .pop()
            .ok_or_else(|| Error::Invalid("导入失败".into()))
    }
}

/// Disallowed prefixes for `User-agent: *`.
fn disallowed(robots: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut applies = false;
    let mut in_agents = false;
    for line in robots.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match key.as_str() {
            "user-agent" => {
                if !in_agents {
                    applies = false;
                }
                in_agents = true;
                applies |= value == "*";
            }
            "disallow" => {
                in_agents = false;
                if applies && !value.is_empty() {
                    out.push(value.to_string());
                }
            }
            _ => in_agents = false,
        }
    }
    out
}

/// File name from Content-Disposition, else the URL; adds `.pdf` to an
/// extensionless PDF.
fn download_name(url: &Url, headers: &reqwest::header::HeaderMap, bytes: &[u8]) -> String {
    static STAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)filename\*\s*=\s*[^']*''([^;]+)"#).unwrap());
    static PLAIN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)filename\s*=\s*"?([^";]+)"?"#).unwrap());
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let disposition = headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .unwrap_or_default();
    let mut name = STAR
        .captures(&disposition)
        .map(|c| decode(&c[1]))
        .or_else(|| PLAIN.captures(&disposition).map(|c| decode(&c[1])))
        .or_else(|| {
            url.path_segments()
                .and_then(|mut s| s.next_back())
                .map(decode)
        })
        .unwrap_or_default();
    name = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    if name.is_empty() {
        name = "下载的资料".into();
    }
    if !name.contains('.') && bytes.starts_with(b"%PDF") {
        name.push_str(".pdf");
    }
    name
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}（{n}）{ext}")))
        .find(|p| !p.exists())
        .expect("some free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_matches_domain_suffixes() {
        let list: Vec<String> = ["gov.cn", "www.spcsc.sh.cn", " *.sacinfo.org.cn "]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(allowed("www.shanghai.gov.cn", &list));
        assert!(allowed("gov.cn", &list));
        assert!(allowed("hbba.sacinfo.org.cn", &list));
        assert!(allowed("www.spcsc.sh.cn", &list));
        assert!(!allowed("spcsc.sh.cn", &list));
        assert!(!allowed("fakegov.cn", &list));
        assert!(!allowed("gov.cn.evil.com", &list));
    }

    #[test]
    fn parses_bing_results_and_attachments() {
        let html = r#"<ol><li class="b_algo"><h2><a href="https://www.shanghai.gov.cn/nw12344/a.html" h="x">上海市<strong>统计</strong>公报 &amp; 解读</a></h2>
            <div class="b_caption"><p class="b_lineclamp2">2024年全市常住人口&nbsp;2480万人</p></div></li>
            <li class="b_algo"><h2><a href="https://example.com/x">其他</a></h2></li></ol>"#;
        let found = parse_bing(html);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "https://www.shanghai.gov.cn/nw12344/a.html");
        assert_eq!(clean_text(&found[0].1), "上海市统计公报 & 解读");
        assert_eq!(clean_text(&found[0].2), "2024年全市常住人口 2480万人");

        let page = Url::parse("https://tjj.sh.gov.cn/tjgb/2024/index.html").unwrap();
        let links = attachments(
            &page,
            r#"<a href="../files/公报.PDF">2024年统计公报（PDF）</a> <a href="/x.html">页面</a> <a href='https://tjj.sh.gov.cn/a.docx'>附件</a>"#,
        );
        assert_eq!(links.len(), 2);
        assert_eq!(
            links[0].0.as_str(),
            "https://tjj.sh.gov.cn/tjgb/files/%E5%85%AC%E6%8A%A5.PDF"
        );
        assert_eq!(file_type(&links[0].0).as_deref(), Some("pdf"));
    }

    #[test]
    fn unwraps_bing_click_tracking_links() {
        // Markup as Bing served it in 2026-09: every result link goes through /ck/a.
        let html = r#"<li class="b_algo b_vtl_deeplinks" data-id iid=SERP.5384><h2 class=""><a target="_blank" href="https://www.bing.com/ck/a?!&amp;&amp;p=218c&amp;ptn=3&amp;ver=2&amp;hsh=4&amp;u=a1aHR0cHM6Ly93d3cuc2hhbmdoYWkuZ292LmNuL25ldy9hLmh0bWw_eD0x&amp;ntb=1" h="ID=SERP,5132.2">美丽上海</a></h2><p>十五五</p></li>"#;
        let found = parse_bing(html);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "https://www.shanghai.gov.cn/new/a.html?x=1");
        assert_eq!(unwrap_bing_link("https://www.gov.cn/a.html"), None);
    }

    #[test]
    fn reads_real_addresses_from_baidu_results() {
        let html = r#"<div class="result c-container xpath-log new-pmd" srcid="1599" id="1" tpl="se_com_default" mu="https://sthj.sh.gov.cn/a/b.html"><h3 class="c-title t t tts-title"><a href="http://www.baidu.com/link?url=abc" target="_blank"><em>美丽上海</em>建设</a></h3><span class="content-right_1THTn">摘要</span></div>
            <div class="result c-container new-pmd" id="2"><h3 class="t"><a href="http://www.baidu.com/link?url=def">第二条</a></h3></div>"#;
        let found = parse_baidu(html);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, "https://sthj.sh.gov.cn/a/b.html");
        assert_eq!(clean_text(&found[0].1), "美丽上海建设");
        assert_eq!(clean_text(&found[0].2), "摘要");
        assert_eq!(found[1].0, "http://www.baidu.com/link?url=def");
    }

    #[test]
    fn drops_results_that_do_not_mention_the_query() {
        let terms = vec!["美丽上海".to_string(), "十五五".to_string()];
        assert!(mentions("上海市“十五五”生态环境保护规划", &terms));
        assert!(mentions("美丽上海建设三年行动计划", &terms));
        assert!(!mentions("job opportunities - beijing", &terms));
    }

    #[test]
    fn reads_robots_rules() {
        let rules = disallowed(
            "User-agent: Baiduspider\nDisallow: /\n\nUser-agent: *\nDisallow: /admin/\nDisallow:\nAllow: /\n",
        );
        assert_eq!(rules, ["/admin/"]);
    }

    #[test]
    fn names_downloads() {
        let mut headers = reqwest::header::HeaderMap::new();
        let url = Url::parse("https://www.gov.cn/zhengce/%E9%80%9A%E7%9F%A5.pdf").unwrap();
        assert_eq!(download_name(&url, &headers, b"%PDF"), "通知.pdf");
        headers.insert(
            reqwest::header::CONTENT_DISPOSITION,
            "attachment; filename*=UTF-8''%E5%8A%9E%E6%B3%95.docx"
                .parse()
                .unwrap(),
        );
        assert_eq!(download_name(&url, &headers, b""), "办法.docx");
        let bare = Url::parse("https://www.gov.cn/download?id=3").unwrap();
        assert_eq!(
            download_name(&bare, &reqwest::header::HeaderMap::new(), b"%PDF-1.7"),
            "download.pdf"
        );
    }
}
