//! Search APIs built for AI agents, the way ZCode (智谱 web-search-prime),
//! pi's web-access extension (Exa, Brave, Tavily…) and other agent apps
//! search: a keyed JSON API instead of scraping a search engine's pages,
//! which refuse or mislead automated clients. Keys live in the secret store
//! under [`SearchService::secret_name`] and are sent only to that service.

use std::sync::Mutex;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::Core;
use crate::error::{Error, Result};

/// A search API the user holds a key for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchService {
    /// Only the government sites' own search and the search engines.
    #[default]
    None,
    /// 智谱 Web Search API (`search_std`), the service behind ZCode's search.
    Zhipu,
    /// 博查 Web Search API, common in Chinese agent apps.
    Bocha,
    /// Tavily, used by many open-source agents.
    Tavily,
}

impl SearchService {
    pub const ALL: [SearchService; 3] = [Self::Zhipu, Self::Bocha, Self::Tavily];

    pub fn secret_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Zhipu => Some("search-api:zhipu"),
            Self::Bocha => Some("search-api:bocha"),
            Self::Tavily => Some("search-api:tavily"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "不使用",
            Self::Zhipu => "智谱搜索",
            Self::Bocha => "博查搜索",
            Self::Tavily => "Tavily",
        }
    }

    fn default_base(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Zhipu => "https://open.bigmodel.cn",
            Self::Bocha => "https://api.bochaai.com",
            Self::Tavily => "https://api.tavily.com",
        }
    }
}

/// What the settings page shows for each service.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchServiceInfo {
    pub kind: SearchService,
    pub name: &'static str,
    pub has_key: bool,
    pub key_url: &'static str,
    pub note: &'static str,
}

/// One hit from a search API: (url, title, snippet).
pub type Hit = (String, String, String);

/// Service addresses; tests point them at a mock server.
#[derive(Default)]
pub struct SearchApiState {
    bases: Mutex<Vec<(SearchService, String)>>,
}

/// Hits from a 智谱 Web Search response.
pub fn parse_zhipu(v: &Value) -> Vec<Hit> {
    hits(v.get("search_result"), "link", "title", &["content"])
}

/// Hits from a 博查 Web Search response (with or without the `data` wrapper).
pub fn parse_bocha(v: &Value) -> Vec<Hit> {
    let pages = v
        .pointer("/data/webPages/value")
        .or_else(|| v.pointer("/webPages/value"));
    hits(pages, "url", "name", &["summary", "snippet"])
}

/// Hits from a Tavily search response.
pub fn parse_tavily(v: &Value) -> Vec<Hit> {
    hits(v.get("results"), "url", "title", &["content"])
}

fn hits(list: Option<&Value>, url: &str, title: &str, snippet: &[&str]) -> Vec<Hit> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let text = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or_default();
            let link = text(url).trim();
            if link.is_empty() {
                return None;
            }
            let snippet = snippet
                .iter()
                .map(|k| text(k))
                .find(|s| !s.trim().is_empty())
                .unwrap_or_default();
            let snippet: String = snippet.chars().take(400).collect();
            Some((link.to_string(), text(title).to_string(), snippet))
        })
        .collect()
}

impl Core {
    /// The search APIs and whether a key is saved for each.
    pub fn search_services(&self) -> Result<Vec<SearchServiceInfo>> {
        SearchService::ALL
            .iter()
            .map(|&kind| {
                let (key_url, note) = match kind {
                    SearchService::Zhipu => (
                        "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
                        "与智谱大模型共用 API Key，基础版约 0.01 元/次",
                    ),
                    SearchService::Bocha => {
                        ("https://open.bochaai.com", "国内网页搜索 API，按次计费")
                    }
                    SearchService::Tavily => (
                        "https://app.tavily.com",
                        "海外服务，每月有免费额度，国内网站覆盖较少",
                    ),
                    SearchService::None => ("", ""),
                };
                Ok(SearchServiceInfo {
                    kind,
                    name: kind.label(),
                    has_key: self.search_key(kind)?.is_some(),
                    key_url,
                    note,
                })
            })
            .collect()
    }

    pub fn set_search_key(&self, kind: SearchService, key: &str) -> Result<()> {
        let name = kind
            .secret_name()
            .ok_or_else(|| Error::Invalid("请先选择搜索服务".into()))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(Error::Invalid("Key 不能为空".into()));
        }
        self.secrets().set(name, key)
    }

    pub fn clear_search_key(&self, kind: SearchService) -> Result<()> {
        match kind.secret_name() {
            Some(name) => self.secrets().delete(name),
            None => Ok(()),
        }
    }

    fn search_key(&self, kind: SearchService) -> Result<Option<String>> {
        Ok(match kind.secret_name() {
            Some(name) => self.secrets().get(name)?.filter(|k| !k.trim().is_empty()),
            None => None,
        })
    }

    /// Points a search API somewhere else (tests).
    #[doc(hidden)]
    pub fn set_search_api_base(&self, kind: SearchService, base: &str) {
        let mut bases = self.search_api.bases.lock().unwrap();
        bases.retain(|(k, _)| *k != kind);
        bases.push((kind, base.trim_end_matches('/').to_string()));
    }

    fn search_api_base(&self, kind: SearchService) -> String {
        self.search_api
            .bases
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or_else(|| kind.default_base().to_string(), |(_, b)| b.clone())
    }

    /// Whether `kind` is chosen and has a key.
    pub(crate) fn search_api_ready(&self, kind: SearchService) -> bool {
        self.search_key(kind).ok().flatten().is_some()
    }

    /// Searches `query` with the chosen API. `sites` narrows it to domains
    /// where the service supports that.
    pub(crate) async fn search_api(
        &self,
        http: &reqwest::Client,
        kind: SearchService,
        query: &str,
        sites: &[&str],
    ) -> Result<Vec<Hit>> {
        let key = self
            .search_key(kind)?
            .ok_or_else(|| Error::Invalid(format!("{}还没有保存 Key", kind.label())))?;
        let base = self.search_api_base(kind);
        let (path, body) = match kind {
            SearchService::None => return Ok(Vec::new()),
            SearchService::Zhipu => (
                "/api/paas/v4/web_search",
                // The query is capped at 70 characters; one domain filter at most.
                json!({
                    "search_query": query.chars().take(70).collect::<String>(),
                    "search_engine": "search_std",
                    "search_intent": false,
                    "count": 15,
                    "content_size": "medium",
                }),
            ),
            SearchService::Bocha => (
                "/v1/web-search",
                json!({ "query": query, "summary": true, "count": 15, "freshness": "noLimit" }),
            ),
            SearchService::Tavily => (
                "/search",
                json!({ "query": query, "max_results": 15, "search_depth": "basic" }),
            ),
        };
        let mut body = body;
        if !sites.is_empty() {
            match kind {
                SearchService::Zhipu if sites.len() == 1 => {
                    body["search_domain_filter"] = sites[0].into();
                }
                SearchService::Bocha => body["include"] = sites.join("|").into(),
                SearchService::Tavily => body["include_domains"] = json!(sites),
                _ => {}
            }
        }
        let url =
            Url::parse(&format!("{base}{path}")).map_err(|e| Error::Invalid(e.to_string()))?;
        let response = http
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Invalid(format!("无法访问：{}", e.without_url())))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| Error::Invalid(e.without_url().to_string()))?;
        if !status.is_success() {
            let detail: String = text.chars().take(160).collect();
            return Err(Error::Invalid(match status.as_u16() {
                401 | 403 => format!("Key 无效或没有权限（HTTP {}）", status.as_u16()),
                402 | 429 => format!("额度不足或请求过于频繁（HTTP {}）", status.as_u16()),
                code => format!("HTTP {code}：{detail}"),
            }));
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|_| Error::Invalid("返回内容无法解析".into()))?;
        Ok(match kind {
            SearchService::Zhipu => parse_zhipu(&v),
            SearchService::Bocha => parse_bocha(&v),
            SearchService::Tavily => parse_tavily(&v),
            SearchService::None => Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_services_results() {
        let zhipu = json!({"search_result": [{"title": "通知", "link": "https://www.shanghai.gov.cn/a", "content": "沪府发〔2026〕16号"}]});
        assert_eq!(
            parse_zhipu(&zhipu),
            [(
                "https://www.shanghai.gov.cn/a".into(),
                "通知".into(),
                "沪府发〔2026〕16号".into()
            )]
        );
        let bocha = json!({"code": 200, "data": {"webPages": {"value": [{"name": "规划", "url": "https://b", "snippet": "短", "summary": "长摘要"}]}}});
        assert_eq!(parse_bocha(&bocha)[0].2, "长摘要");
        let tavily = json!({"results": [{"title": "T", "url": "https://t", "content": "c"}, {"title": "no url"}]});
        assert_eq!(parse_tavily(&tavily).len(), 1);
    }
}
