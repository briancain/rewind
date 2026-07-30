//! Thin HTTP client over the public service endpoints. It performs the network I/O and returns the
//! raw status + body; the tiers apply the pure [`crate::assertions`] helpers and deserialize the
//! [`crate::models`] DTOs. Keeping the client this thin keeps all decision logic unit-testable.

use std::time::Duration;

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::Endpoints;

/// A raw HTTP response: the status code and the (already-read) body text.
pub struct Resp {
    pub status: u16,
    pub body: String,
}

impl Resp {
    /// Deserialize the body into `T`, with a body excerpt in the error for debuggability.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_str(&self.body).map_err(|e| {
            format!(
                "failed to decode {}: {e} (status {}, body: {})",
                std::any::type_name::<T>(),
                self.status,
                excerpt(&self.body)
            )
        })
    }
}

fn excerpt(body: &str) -> String {
    const MAX: usize = 300;
    if body.len() <= MAX {
        body.to_string()
    } else {
        format!("{}…", &body[..MAX])
    }
}

/// HTTP client bound to a set of service endpoints.
pub struct RewindClient {
    http: reqwest::Client,
    pub endpoints: Endpoints,
}

impl RewindClient {
    pub fn new(endpoints: Endpoints) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("rewind-canary/0.1")
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self { http, endpoints })
    }

    /// Perform a request and read the body. A transport error (DNS/TLS/timeout) is an `Err`; an
    /// HTTP error status is a normal `Ok(Resp)` so the caller can assert on it.
    pub async fn send(
        &self,
        method: Method,
        url: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<Resp, String> {
        let mut req = self.http.request(method.clone(), url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = &body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("{method} {url}: transport error: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("{method} {url}: failed to read body: {e}"))?;
        Ok(Resp { status, body })
    }

    pub async fn get(&self, url: &str, token: Option<&str>) -> Result<Resp, String> {
        self.send(Method::GET, url, token, None).await
    }

    pub async fn post(
        &self,
        url: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<Resp, String> {
        self.send(Method::POST, url, token, body).await
    }

    pub async fn delete(&self, url: &str, token: Option<&str>) -> Result<Resp, String> {
        self.send(Method::DELETE, url, token, None).await
    }
}
