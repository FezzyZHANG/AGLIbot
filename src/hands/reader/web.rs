use super::{Reader, UnreadableInput, bounded, remote_failure};
use crate::error_report::Fault;
use anyhow::{Context, Result, anyhow, ensure};
use reqwest::{Method, RequestBuilder, Url};
use serde_json::{Value, json};
use std::time::Duration;

const WEB_CALL_USD: f64 = 0.00075;

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub summary: String,
    pub content: String,
    pub rerank_score: Option<f64>,
    pub authority: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct FetchPage {
    pub url: String,
    pub title: String,
    pub content: String,
    pub http_status: u16,
}

#[derive(Debug)]
pub struct FetchHttpError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for FetchHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.message)
    }
}

impl std::error::Error for FetchHttpError {}

impl Reader {
    async fn retry(request: impl Fn() -> RequestBuilder) -> Result<reqwest::Response> {
        let send = || async {
            request()
                .send()
                .await
                .map_err(|_| anyhow!("DeepSeek 请求未完成"))
        };
        let response = send().await?;
        let status = response.status();
        if status.is_server_error() || matches!(status.as_u16(), 408 | 429) {
            drop(response);
            tokio::time::sleep(Duration::from_millis(500)).await;
            return send().await;
        }
        Ok(response)
    }

    pub async fn search(
        &self,
        query: &str,
        count: usize,
        ori_question: Option<&str>,
        need_content: bool,
    ) -> Result<Vec<SearchHit>> {
        let query: String = query.trim().chars().take(128).collect();
        if query.is_empty() {
            return Err(UnreadableInput("搜索内容为空".into()).into());
        }
        let mut payload =
            json!({"query": query, "count": count.clamp(1, 50), "need_content": need_content});
        if let Some(question) = ori_question {
            payload["ori_question"] = json!(question);
        }
        self.metered(
            "search",
            async {
                let response = Self::retry(|| {
                    self.request(Method::POST, "/v1/tools/search")
                        .json(&payload)
                })
                .await?;
                let v = Self::response_json(response).await?;
                Ok(v["results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|hit| {
                        let url = hit["url"].as_str()?.trim();
                        if url.is_empty() {
                            return None;
                        }
                        let field = |name: &str| hit[name].as_str().unwrap_or_default().to_owned();
                        Some(SearchHit {
                            title: field("title"),
                            url: url.into(),
                            snippet: field("snippet"),
                            summary: field("summary"),
                            content: field("content"),
                            rerank_score: hit["rerank_score"].as_f64(),
                            authority: hit["authority"].as_f64(),
                        })
                    })
                    .collect())
            },
            |hits: &Vec<SearchHit>| if hits.is_empty() { 0.0 } else { WEB_CALL_USD },
        )
        .await
    }

    pub async fn fetch(&self, url: &str) -> Result<String> {
        let page = self.fetch_page(url).await?;
        bounded(&format!("{}\n{}", page.title, page.content))
    }

    pub async fn fetch_page(&self, url: &str) -> Result<FetchPage> {
        let url_for_check = Url::parse(url).map_err(|_| anyhow!("无效链接"))?;
        ensure!(
            !contains_access_token(&url_for_check),
            "含访问凭据的链接不能提交至网页读取服务"
        );
        let (url, _) = crate::channels::public_url(url).await?;
        self.metered("fetch", self.fetch_public(&url), |_| WEB_CALL_USD)
            .await
            .inspect_err(|e| remote_failure(e, Fault::WebRead))
    }

    async fn fetch_public(&self, url: &Url) -> Result<FetchPage> {
        let mut response = Self::retry(|| {
            self.request(Method::POST, "/v1/tools/fetch")
                .json(&json!({"url": url.as_str()}))
        })
        .await?;
        let transport = response.status();
        if !transport.is_success() {
            return Err(FetchHttpError {
                status: transport.as_u16(),
                message: "DeepSeek 网页读取请求失败".into(),
            }
            .into());
        }
        let v = Self::decode_json(&mut response).await?;
        let statuses = ["status_code", "http_status"].map(|field| {
            let status = &v[field];
            status
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .or_else(|| status.as_str()?.parse().ok())
        });
        if let Some(status) = statuses.into_iter().flatten().find(|&status| status >= 400) {
            return Err(FetchHttpError {
                status,
                message: "目标网页不可用".into(),
            }
            .into());
        }
        ensure!(
            v.get("error").is_none_or(Value::is_null),
            "DeepSeek 处理失败"
        );
        let http_status = statuses
            .into_iter()
            .flatten()
            .next()
            .unwrap_or(transport.as_u16());
        if !(200..300).contains(&http_status) {
            return Err(FetchHttpError {
                status: http_status,
                message: "网页读取未返回成功状态".into(),
            }
            .into());
        }
        let content = bounded(v["content"].as_str().context("缺少网页正文")?)?;
        let title = v["title"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
        Ok(FetchPage {
            url: url.to_string(),
            title,
            content,
            http_status,
        })
    }
}

pub(super) fn contains_access_token(url: &reqwest::Url) -> bool {
    url.query_pairs().any(|(name, _)| {
        let name = name.to_ascii_lowercase().replace('-', "_");
        matches!(
            name.as_str(),
            "key"
                | "apikey"
                | "auth"
                | "authorization"
                | "signature"
                | "sig"
                | "jwt"
                | "password"
                | "passwd"
                | "sessionid"
                | "credential"
                | "secret"
        ) || name.ends_with("token")
            || name.ends_with("_key")
            || name.starts_with("x_amz_")
            || name.starts_with("x_goog_")
    })
}
