//! HTTP response caching for public desktop artwork.
use crate::api::Api;
use anyhow::Result;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest::{Client, Method, Request, Response, redirect::Policy};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use std::{fs, path::Path, time::Duration};

#[derive(Clone)]
pub struct ImageHttpClient {
    cached: ClientWithMiddleware,
    direct: Client,
}

impl ImageHttpClient {
    pub fn new(directory: &Path, redirects: Policy) -> Result<Self> {
        let direct = Api::http_client_builder()?
            .timeout(Duration::from_secs(30))
            .redirect(redirects)
            .build()?;
        let cached = ClientBuilder::new(direct.clone())
            .with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager {
                    path: directory.into(),
                },
                options: HttpCacheOptions::default(),
            }))
            .build();
        Ok(Self { cached, direct })
    }

    pub async fn send(&self, request: Request) -> Result<Response> {
        // GPUI also exposes this transport to non-image callers. Only an empty
        // GET may use the shared cache; credentials remain visible to its policy.
        let cacheable = request.method() == Method::GET
            && request
                .body()
                .is_none_or(|body| body.as_bytes().is_some_and(<[u8]>::is_empty));
        if cacheable {
            Ok(self.cached.execute(request).await?)
        } else {
            Ok(self.direct.execute(request).await?)
        }
    }

    /// Trim expendable HTTP data at startup, including all redirect-policy stores.
    pub fn trim_cache(directory: &Path) -> Result<()> {
        let mut pending = vec![directory.to_path_buf()];
        let mut bytes = 0u64;
        while let Some(path) = pending.pop() {
            let entries = match fs::read_dir(path) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    pending.push(entry.path());
                } else {
                    bytes = bytes.saturating_add(entry.metadata()?.len());
                }
                if bytes > 128 << 20 {
                    fs::remove_dir_all(directory)?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}
