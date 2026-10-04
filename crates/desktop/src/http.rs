//! Adapt the shared image transport to GPUI without creating another runtime.
use futures_util::{AsyncReadExt, FutureExt, TryStreamExt, future::BoxFuture};
use gpui_kit::http_client::{
    AsyncBody, HttpClient, RedirectPolicy, Request, Response, Url, http::HeaderValue,
};
use listenbox_sync_engine::image_http::ImageHttpClient;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::runtime::Handle;

pub struct DesktopHttpClient {
    clients: Mutex<HashMap<RedirectPolicy, ImageHttpClient>>,
    directory: PathBuf,
    runtime: Handle,
}

impl DesktopHttpClient {
    pub fn new(profile: &Path, runtime: Handle) -> anyhow::Result<Self> {
        let directory = profile.join("http-cache");
        ImageHttpClient::trim_cache(&directory)?;
        let client = Self {
            clients: Mutex::new(HashMap::new()),
            directory,
            runtime,
        };
        client.client(RedirectPolicy::NoFollow)?;
        client.client(RedirectPolicy::FollowAll)?;
        Ok(client)
    }

    fn client(&self, redirects: RedirectPolicy) -> anyhow::Result<ImageHttpClient> {
        let mut clients = self.clients.lock();
        if let Some(client) = clients.get(&redirects) {
            return Ok(client.clone());
        }
        // A cached final response must never satisfy a caller that needs the
        // redirect itself, or one with a different maximum number of hops.
        let (policy, store) = match redirects {
            RedirectPolicy::NoFollow => (reqwest::redirect::Policy::none(), "no-follow".into()),
            RedirectPolicy::FollowAll => (reqwest::redirect::Policy::default(), "follow".into()),
            RedirectPolicy::FollowLimit(limit) => (
                reqwest::redirect::Policy::limited(limit as usize),
                format!("follow-{limit}"),
            ),
        };
        let client = ImageHttpClient::new(&self.directory.join(store), policy)?;
        clients.insert(redirects, client.clone());
        Ok(client)
    }
}

impl HttpClient for DesktopHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }
    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let (head, mut body) = request.into_parts();
        let client = self.client(
            head.extensions
                .get::<RedirectPolicy>()
                .cloned()
                .unwrap_or_default(),
        );
        let runtime = self.runtime.clone();
        async move {
            let client = client?;
            let mut bytes = Vec::new();
            body.read_to_end(&mut bytes).await?;
            let mut request = reqwest::Request::new(head.method, head.uri.to_string().parse()?);
            *request.headers_mut() = head.headers;
            *request.body_mut() = Some(bytes.into());
            let response = runtime
                .spawn(async move { client.send(request).await })
                .await??;
            let mut builder = Response::builder()
                .status(response.status())
                .version(response.version());
            *builder.headers_mut().expect("valid HTTP response") = response.headers().clone();
            let body = response
                .bytes_stream()
                .map_err(std::io::Error::other)
                .into_async_read();
            Ok(builder.body(AsyncBody::from_reader(body))?)
        }
        .boxed()
    }
}
