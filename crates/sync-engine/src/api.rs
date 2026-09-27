use crate::{auth::StoredAuth, config::Config};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Response};
use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct AuthenticationRequired;
impl std::fmt::Display for AuthenticationRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sign in to Listenbox to continue.")
    }
}
impl std::error::Error for AuthenticationRequired {}

#[derive(Debug)]
pub struct PaymentRequired(pub String);
impl std::fmt::Display for PaymentRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP 402: {}", self.0)
    }
}
impl std::error::Error for PaymentRequired {}

#[derive(Clone)]
pub struct Api {
    pub config: Config,
    pub credential: Option<String>,
    pub http: ClientWithMiddleware,
    pub cancel: CancellationToken,
}

impl Api {
    pub fn new(config: Config, cancel: CancellationToken) -> Result<Self> {
        let credential = std::fs::read(config.directory.join("auth.json"))
            .ok()
            .filter(|raw| raw.len() <= 64 << 10)
            .and_then(|raw| serde_json::from_slice::<StoredAuth>(&raw).ok())
            .filter(|auth| crate::config::credential_valid(&auth.api_key))
            .map(|auth| auth.api_key);
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(30 * 60));
        if let Some(path) = std::env::var_os("SSL_CERT_FILE") {
            for certificate in reqwest::Certificate::from_pem_bundle(&std::fs::read(path)?)? {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(Self {
            config,
            credential,
            http: reqwest_middleware::ClientBuilder::new(builder.build()?).build(),
            cancel,
        })
    }

    pub fn client(&self) -> crate::publicapi::Client {
        crate::publicapi::Client::new(
            self.http.clone(),
            self.config.api_origin.clone(),
            self.credential.clone(),
        )
    }

    pub async fn wait<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("operation interrupted; resumable source upload state preserved"),
            result = work => result,
        }
    }

    pub async fn send(&self, request: RequestBuilder) -> Result<Response> {
        self.wait(async {
            request
                .send()
                .await
                .map_err(|error| error.without_url())
                .context("send HTTP request")
        })
        .await
    }

    pub async fn bytes(&self, response: Response, limit: usize) -> Result<Vec<u8>> {
        self.wait(read_bounded(response, limit)).await
    }

    pub async fn decode<T: DeserializeOwned>(&self, response: Response) -> Result<T> {
        serde_json::from_slice(&self.bytes(response, 4 << 20).await?).context("decode API response")
    }

    pub async fn json(&self, request: RequestBuilder, statuses: &[u16]) -> Result<Value> {
        let response = self.send(request).await?;
        self.accept(response, statuses).await
    }

    pub async fn accept(&self, response: Response, statuses: &[u16]) -> Result<Value> {
        if !statuses.contains(&response.status().as_u16()) {
            return Err(self.response_error(response).await);
        }
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        self.decode(response).await
    }

    pub async fn response_error(&self, response: Response) -> anyhow::Error {
        let status = response.status().as_u16();
        if status == 401 {
            return AuthenticationRequired.into();
        }
        let raw = self.bytes(response, 64 << 10).await.unwrap_or_default();
        let message = serde_json::from_slice::<Value>(&raw)
            .ok()
            .and_then(|body| {
                body.get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&raw).trim().to_owned());
        if status == 402 {
            return PaymentRequired(message).into();
        }
        let hint = match status {
            403 => "permission denied",
            _ => &message,
        };
        anyhow::anyhow!("HTTP {status}: {hint}")
    }

    pub fn trace(&self, response: &Response, stdout: bool) -> Result<()> {
        if self.config.print_trace_ids {
            let trace = response
                .headers()
                .get("X-Trace-Id")
                .context("response missing X-Trace-Id")?
                .to_str()?;
            crate::config::check_trace(trace)?;
            if stdout {
                println!("Trace ID: {trace}");
            } else {
                eprintln!("Trace ID: {trace}");
            }
        }
        Ok(())
    }
}

pub async fn read_bounded(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= limit.saturating_sub(body.len()),
            "HTTP response exceeds {limit} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("response missing {key}"))
}

pub fn number(value: &Value, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .with_context(|| format!("response missing numeric {key}"))
}
