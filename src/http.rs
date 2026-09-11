use crate::domain::Fault;
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
    async fn execute(&self, request: HttpRequest, timeout: Duration)
    -> Result<HttpResponse, Fault>;
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
    ) -> Result<HttpResponse, Fault> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| Fault::new("UPSTREAM_HTTP_ERROR"))?;
        let mut builder = self.client.request(method, request.url);
        for (key, value) in request.headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = tokio::time::timeout(timeout, builder.send())
            .await
            .map_err(|_| Fault::with_status("UPSTREAM_TIMEOUT", 504))?
            .map_err(|error| {
                if error.is_timeout() {
                    Fault::with_status("UPSTREAM_TIMEOUT", 504)
                } else {
                    Fault::with_status("UPSTREAM_HTTP_ERROR", 502)
                }
            })?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Fault::with_status("UPSTREAM_HTTP_ERROR", 502))?;
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(Fault::with_status("UPSTREAM_RESPONSE_TOO_LARGE", 502));
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
) -> Result<Value, Fault> {
    let response = client.execute(request, timeout).await?;
    if !(200..300).contains(&response.status) {
        return Err(Fault::with_status(
            if response.status == 429 {
                "UPSTREAM_RATE_LIMITED"
            } else {
                "UPSTREAM_HTTP_ERROR"
            },
            502,
        ));
    }
    if response.body.is_empty() {
        return Err(Fault::with_status("UPSTREAM_EMPTY", 502));
    }
    serde_json::from_slice(&response.body)
        .map_err(|_| Fault::with_status("UPSTREAM_INVALID_JSON", 502))
}

pub async fn json_request_as<T: DeserializeOwned>(
    client: &dyn HttpClient,
    request: HttpRequest,
    timeout: Duration,
) -> Result<T, Fault> {
    let raw = json_request(client, request, timeout).await?;
    serde_json::from_value(raw).map_err(|_| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))
}

pub fn url_with_params(base: &str, params: &[(&str, &str)]) -> Result<String, Fault> {
    let mut url =
        url::Url::parse(base).map_err(|_| Fault::with_status("UPSTREAM_HTTP_ERROR", 502))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockHttp {
        response: HttpResponse,
    }

    #[async_trait]
    impl HttpClient for MockHttp {
        async fn execute(
            &self,
            _request: HttpRequest,
            _timeout: Duration,
        ) -> Result<HttpResponse, Fault> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn json_request_redacts_upstream_body_and_caps_json() {
        let client = Arc::new(MockHttp {
            response: HttpResponse {
                status: 429,
                body: b"secret".to_vec(),
            },
        });
        let error = json_request(
            client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url: "https://example.test".into(),
                headers: HashMap::new(),
                body: None,
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "UPSTREAM_RATE_LIMITED");

        let client = MockHttp {
            response: HttpResponse {
                status: 200,
                body: b"not-json".to_vec(),
            },
        };
        let error = json_request(
            &client,
            HttpRequest {
                method: "GET".into(),
                url: "https://example.test".into(),
                headers: HashMap::new(),
                body: None,
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "UPSTREAM_INVALID_JSON");
    }

    #[test]
    fn url_parameters_are_encoded_by_url() {
        let url = url_with_params(
            "https://example.test/quote",
            &[("amount", "1 2"), ("token", "0xabc")],
        )
        .unwrap();
        assert!(url.contains("amount=1+2"));
        assert!(url.contains("token=0xabc"));
    }
}
