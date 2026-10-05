use crate::{auth::StoredAuth, config::Config, publicapi as p};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Response};
use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use serde_json::Value;
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

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

#[derive(Debug)]
pub struct SyncStopped;
impl std::fmt::Display for SyncStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The server stopped YouTube importing for this show. Progress is saved; refresh the podcast list.")
    }
}
impl std::error::Error for SyncStopped {}

#[derive(Debug)]
pub(crate) struct HttpFailure {
    pub status: u16,
    message: String,
}

impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.message)
    }
}
impl std::error::Error for HttpFailure {}

#[derive(Clone)]
pub struct Api {
    pub config: Config,
    pub credential: Option<String>,
    pub http: ClientWithMiddleware,
    pub(crate) upload_http: ClientWithMiddleware,
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
        Ok(Self {
            config,
            credential,
            http: Self::http_client(false)?,
            upload_http: reqwest_middleware::ClientBuilder::new(
                Self::transport_builder()?.build()?,
            )
            .build(),
            cancel,
        })
    }

    pub(crate) fn http_client(no_redirects: bool) -> Result<ClientWithMiddleware> {
        let mut builder = Self::http_client_builder()?;
        if no_redirects {
            builder = builder.redirect(reqwest::redirect::Policy::none());
        }
        Ok(reqwest_middleware::ClientBuilder::new(builder.build()?).build())
    }

    pub(crate) fn http_client_builder() -> Result<reqwest::ClientBuilder> {
        Ok(Self::transport_builder()?.timeout(Duration::from_secs(30 * 60)))
    }

    fn transport_builder() -> Result<reqwest::ClientBuilder> {
        let mut builder = Client::builder().connect_timeout(Duration::from_secs(30));
        if let Some(path) = std::env::var_os("SSL_CERT_FILE") {
            for certificate in reqwest::Certificate::from_pem_bundle(&std::fs::read(path)?)? {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(builder)
    }

    pub fn client(&self) -> p::Client<&Self> {
        crate::publicapi::Client::new(
            self.http.clone(),
            self.config.api_origin.clone(),
            self.credential.clone(),
        )
        .with_transport(self)
    }

    pub async fn team_id(&self) -> Result<String> {
        match self.client().whoami().await? {
            p::WhoamiResponse::Status200(account) => Ok(account.team_id),
            response => Err(self.response_error(response).await),
        }
    }

    pub async fn wait<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => bail!("operation interrupted; resumable source upload state preserved"),
            result = work => result,
        }
    }

    pub async fn send(&self, mut request: RequestBuilder) -> Result<Response> {
        let mut extensions = std::mem::take(request.extensions());
        let (client, request) = request.build_split();
        let request = request
            .map_err(|error| error.without_url())
            .context("build HTTP request")?;
        let diagnostic = (!request
            .url()
            .host_str()
            .is_some_and(|host| host == "googlevideo.com" || host.ends_with(".googlevideo.com")))
        .then(|| {
            let origin = request.url().origin().ascii_serialization();
            // Signed media URLs may contain credentials in their paths and query strings.
            let path = if origin != self.config.api_origin {
                ""
            } else if request.url().path().starts_with("/cli/authorizations/") {
                "/cli/authorizations/{code}/events"
            } else {
                request.url().path()
            };
            format!("{} {origin}{path}", request.method())
        });
        let started = Instant::now();
        let span = diagnostic
            .as_ref()
            .map_or_else(tracing::Span::none, |request| {
                tracing::info_span!("http.request", request)
            });
        let result = self
            .wait(async {
                client
                    .execute_with_extensions(request, &mut extensions)
                    .await
                    .map_err(|error| error.without_url())
                    .context("send HTTP request")
            })
            .instrument(span)
            .await;
        if let Some(diagnostic) = diagnostic {
            let elapsed = started.elapsed().as_millis();
            match &result {
                Ok(response) => {
                    let trace = response
                        .headers()
                        .get("X-Trace-Id")
                        .and_then(|value| value.to_str().ok())
                        .filter(|value| crate::config::check_trace(value).is_ok())
                        .unwrap_or("-");
                    if cfg!(debug_assertions) {
                        eprintln!(
                            "HTTP {diagnostic} -> {} ({elapsed}ms) trace_id={trace}",
                            response.status().as_u16()
                        );
                    }
                    tracing::info!(request = %diagnostic, status = response.status().as_u16(),
                        elapsed_ms = elapsed as u64, trace_id = trace, "HTTP completed");
                }
                Err(error) => {
                    if cfg!(debug_assertions) {
                        eprintln!("HTTP {diagnostic} -> failed ({elapsed}ms): {error:#}");
                    }
                    tracing::info!(request = %diagnostic, elapsed_ms = elapsed as u64,
                        cancelled = self.cancel.is_cancelled(), error = %crate::redact(&format!("{error:#}")), "HTTP failed");
                }
            }
        }
        result
    }

    pub async fn bytes(&self, response: Response, limit: usize) -> Result<Vec<u8>> {
        self.wait(read_bounded(response, limit)).await
    }

    pub async fn response_error<R: p::Response>(&self, response: R) -> anyhow::Error {
        let status = response.status();
        if status == 423 {
            return SyncStopped.into();
        }
        if status == 401 {
            return AuthenticationRequired.into();
        }
        let error = match self.wait(async { Ok(response.into_error().await) }).await {
            Ok(error) => error,
            Err(error) => return error,
        };
        let p::Error::Http { body: raw, .. } = error else {
            return anyhow::Error::new(error).context(HttpFailure {
                status,
                message: "Could not read error response".into(),
            });
        };
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
        HttpFailure {
            status,
            message: hint.to_owned(),
        }
        .into()
    }
}

impl p::Transport for &Api {
    type Error = anyhow::Error;

    async fn execute<R: p::Response>(&self, request: RequestBuilder, limit: usize) -> Result<R> {
        self.wait(async {
            R::decode(self.send(request).await?, limit)
                .await
                .map_err(Into::into)
        })
        .await
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
