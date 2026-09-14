use crate::error::ErrorKind;
use anyhow::Context as _;
use async_trait::async_trait;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

pub const MAX_RESPONSE_BYTES: usize = 2_000_000;

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(
        &self,
        request: HttpRequest,
        timeout: Duration,
    ) -> anyhow::Result<HttpResponse>;
}

#[derive(Clone)]
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("metamatch-backend/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client configuration must be valid"),
        }
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn execute(
        &self,
        request: HttpRequest,
        timeout: Duration,
    ) -> anyhow::Result<HttpResponse> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .context(ErrorKind::UpstreamHttpError)?;
        let mut builder = self.client.request(method, request.url);
        for (key, value) in request.headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = tokio::time::timeout(timeout, builder.send())
            .await
            .context(ErrorKind::UpstreamTimeout)
            .context("HTTP response headers")?
            .map_err(|error| {
                if error.is_timeout() {
                    anyhow::Error::new(error.without_url())
                        .context(ErrorKind::UpstreamTimeout)
                        .context("HTTP send timeout")
                } else {
                    anyhow::Error::new(error.without_url())
                        .context(ErrorKind::UpstreamHttpError)
                        .context("HTTP transport")
                }
            })?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                anyhow::Error::new(error.without_url())
                    .context(ErrorKind::UpstreamHttpError)
                    .context("HTTP response body")
            })?;
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                anyhow::bail!(ErrorKind::UpstreamResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse { status, body })
    }
}

pub async fn json_request(
    client: &dyn HttpClient,
    request: HttpRequest,
    timeout: Duration,
) -> anyhow::Result<Value> {
    // Only provider host/path are used here; RPC credentials never pass through this client.
    let endpoint = url::Url::parse(&request.url)
        .ok()
        .map(|url| {
            format!(
                "{} {}{}",
                request.method,
                url.host_str().unwrap_or_default(),
                url.path()
            )
        })
        .unwrap_or_else(|| request.method.clone());
    let response = client
        .execute(request, timeout)
        .await
        .with_context(|| endpoint.clone())?;
    if !(200..300).contains(&response.status) {
        let source = UpstreamHttpError {
            status: response.status,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        };
        return Err(anyhow::Error::new(source)
            .context(if response.status == 429 {
                ErrorKind::UpstreamRateLimited
            } else {
                ErrorKind::UpstreamHttpError
            })
            .context(endpoint));
    }
    if response.body.is_empty() {
        anyhow::bail!(ErrorKind::UpstreamEmpty);
    }
    serde_json::from_slice(&response.body)
        .context(ErrorKind::UpstreamInvalidJson)
        .with_context(|| format!("{endpoint}: JSON decode"))
}

pub async fn json_request_as<T: DeserializeOwned>(
    client: &dyn HttpClient,
    request: HttpRequest,
    timeout: Duration,
) -> anyhow::Result<T> {
    let raw = json_request(client, request, timeout).await?;
    serde_path_to_error::deserialize(raw).map_err(|error| {
        let path = error.path().to_string();
        anyhow::Error::new(error)
            .context(ErrorKind::UpstreamInvalidResponse)
            .context(format!("decode {} at {path}", std::any::type_name::<T>()))
    })
}

#[derive(Debug, thiserror::Error)]
#[error("upstream HTTP {status}: {body}")]
pub struct UpstreamHttpError {
    pub status: u16,
    pub body: String,
}

pub fn url_with_params(base: &str, params: &[(&str, &str)]) -> anyhow::Result<String> {
    let mut url = url::Url::parse(base).context(ErrorKind::UpstreamHttpError)?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

pub fn auth_headers(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}
