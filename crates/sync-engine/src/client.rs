//! Application-facing operations shared by native interfaces. Storage stays private.
use crate::{
    api::Api,
    auth,
    config::Config,
    downloads::DownloadManager,
    publicapi as p,
    sync::{Engine, Report},
};
use anyhow::{Result, ensure};
use futures_util::{StreamExt, stream};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct Catalog {
    pub teams: Vec<p::ClientTeam>,
    pub import_team: Option<String>,
    pub shows: Vec<p::Show>,
}

#[derive(Default)]
pub struct SyncState {
    pub items: Vec<crate::downloads::Download>,
    pub last_synced: Option<std::time::SystemTime>,
}

pub enum SyncEvent {
    ScanStarted(Vec<p::Show>),
    ShowFinished {
        slug: String,
        result: Result<Report>,
        items: Vec<crate::downloads::Download>,
    },
    ScanFinished {
        failed: usize,
    },
}

#[derive(Clone)]
pub struct Client {
    config: Config,
    engine: Engine,
}

impl Client {
    pub fn desktop(config: Config) -> Result<Self> {
        Ok(Self {
            engine: Engine::default(),
            config,
        })
    }
    pub fn cookie_jar(&self) -> crate::cookies::CookieJar {
        crate::cookies::CookieJar::new(&self.config.directory)
    }
    pub fn downloads(&self) -> DownloadManager {
        self.engine.downloads.clone()
    }
    #[tracing::instrument(name = "journal.read", skip_all, fields(show_slug = %slug))]
    pub async fn sync_state(&self, slug: String, cancel: CancellationToken) -> Result<SyncState> {
        if !self.has_credentials() {
            return Err(crate::api::AuthenticationRequired.into());
        }
        let (engine, config) = (self.engine.clone(), self.config.clone());
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            let database = engine.database(&config, &cancel)?;
            Ok(SyncState {
                items: database.items(&config.api_origin, &slug)?,
                last_synced: database.last_synced(&config.api_origin, &slug)?,
            })
        })
        .await?
    }
    #[tracing::instrument(name = "auth.logout", skip_all)]
    pub fn logout(&self) -> Result<()> {
        auth::logout(&self.config)?;
        self.engine.downloads.clear();
        Ok(())
    }
    pub fn has_credentials(&self) -> bool {
        self.api(CancellationToken::new())
            .is_ok_and(|api| api.credential.is_some())
    }
    pub fn upgrade_url(&self, team: &str) -> Result<String> {
        self.config.upgrade_url(team)
    }
    pub fn show_url(&self, show: &p::Show) -> Result<String> {
        self.config.show_url(&show.team_id, &show.id)
    }
    pub async fn episodes(
        &self,
        slug: String,
        cancel: CancellationToken,
    ) -> Result<Vec<p::EpisodeListItem>> {
        let api = self.api(cancel)?;
        if api.credential.is_none() {
            return Err(crate::api::AuthenticationRequired.into());
        }
        let mut episodes = Vec::new();
        let mut ids = HashSet::new();
        let mut cursors = HashSet::new();
        let mut cursor = None;
        loop {
            let page = match api
                .client()
                .list_episodes(p::ListEpisodesParams {
                    show_slug: slug.clone(),
                    cursor,
                    limit: Some(500),
                })
                .await?
            {
                p::ListEpisodesResponse::Status200(value) => value,
                response => return Err(api.response_error(response).await),
            };
            for episode in page.episodes {
                if ids.insert(episode.id.clone()) {
                    episodes.push(episode);
                }
            }
            cursor = page.next_cursor;
            match &cursor {
                None => return Ok(episodes),
                Some(cursor) => ensure!(
                    !cursor.is_empty() && cursors.insert(cursor.clone()),
                    "Episode listing returned an invalid or repeated cursor"
                ),
            }
        }
    }
    fn api(&self, cancel: CancellationToken) -> Result<Api> {
        Api::new(self.config.clone(), cancel)
    }
    pub async fn login(
        &self,
        cancel: CancellationToken,
        open_browser: impl FnMut(&str),
    ) -> Result<()> {
        auth::login_with(
            &mut self.api(cancel)?,
            p::AuthorizationClient::Desktop,
            open_browser,
        )
        .await
    }
    pub async fn catalog(&self, cancel: CancellationToken) -> Result<Catalog> {
        let api = self.api(cancel)?;
        if api.credential.is_none() {
            return Err(crate::api::AuthenticationRequired.into());
        }
        let client = api.client();
        let (teams, shows, account) = tokio::try_join!(
            client.list_client_teams(p::ListClientTeamsParams {
                writable_only: Some(true)
            }),
            client.list_shows(p::ListShowsParams {
                youtube_imports_only: Some(true)
            }),
            client.whoami()
        )?;
        let teams = match teams {
            p::ListClientTeamsResponse::Status200(teams) => teams,
            response => return Err(api.response_error(response).await),
        };
        let shows = match shows {
            p::ListShowsResponse::Status200(shows) => shows,
            response => return Err(api.response_error(response).await),
        };
        let account = match account {
            p::WhoamiResponse::Status200(account) => account,
            response => return Err(api.response_error(response).await),
        };
        let import_team = teams
            .iter()
            .find(|team| team.id == account.team_id)
            .or_else(|| teams.first())
            .map(|team| team.id.clone());
        Ok(Catalog {
            teams,
            import_team,
            shows,
        })
    }
    #[tracing::instrument(name = "youtube.import", skip_all)]
    pub async fn import_collection(
        &self,
        source: &str,
        team: &str,
        kind: p::ShowSourceKind,
        cancel: CancellationToken,
        event: impl FnMut(crate::youtube::ImportEvent) + Send + 'static,
    ) -> Result<(p::Show, Report)> {
        let source = crate::youtube::collection_source(source)?;
        let team = team.to_owned();
        let api = self.api(cancel)?;
        let engine = self.engine.clone();
        let runtime = tokio::runtime::Handle::current();
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            runtime.block_on(crate::youtube::import(
                &api, &engine, &source, &team, None, kind, event,
            ))
        })
        .await?
    }
    pub async fn next_scan(&self, cancel: &CancellationToken) -> bool {
        self.engine.next_scan(cancel).await
    }

    /// Discover all writable YouTube imports across teams on every pass. Each
    /// show uses the existing sync owner and the shared transfer budgets. Joining
    /// the whole stream also drains owned writes after cancellation.
    pub async fn sync_youtube(
        &self,
        show: Option<&str>,
        watch: bool,
        cancel: CancellationToken,
        mut event: impl FnMut(SyncEvent) + Send,
    ) -> Result<()> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let api = self.api(cancel.clone())?;
            let mut shows = match api
                .client()
                .list_shows(p::ListShowsParams {
                    youtube_imports_only: Some(true),
                })
                .await?
            {
                p::ListShowsResponse::Status200(shows) => shows,
                response => return Err(api.response_error(response).await),
            };
            if let Some(slug) = show {
                shows.retain(|show| show.slug == slug);
                ensure!(
                    !shows.is_empty(),
                    "No accessible YouTube import with slug {slug:?}"
                );
            } else {
                // Match desktop automatic sync's paid-plan admission.
                shows.retain(|show| show.has_active_subscription);
            }
            // The previous pass has joined every worker. Drop display history,
            // including sources that have since been unlinked; keep the journal.
            self.engine.downloads.clear();
            event(SyncEvent::ScanStarted(shows.clone()));
            let count = shows.len().max(1);
            let mut work = stream::iter(shows.into_iter().map(|show| {
                let cancel = cancel.clone();
                async move {
                    let result = self.sync_once(&show.slug, cancel).await;
                    (show.slug, result)
                }
            }))
            .buffer_unordered(count);
            let mut failed = 0;
            while let Some((slug, result)) = work.next().await {
                if result.is_err() && !cancel.is_cancelled() {
                    failed += 1;
                }
                let items = self
                    .engine
                    .downloads
                    .not_imported()
                    .into_iter()
                    .filter(|item| item.source_id == slug)
                    .collect();
                event(SyncEvent::ShowFinished {
                    slug,
                    result,
                    items,
                });
            }
            event(SyncEvent::ScanFinished { failed });
            if cancel.is_cancelled() {
                return Ok(());
            }
            if !watch {
                ensure!(
                    failed == 0,
                    "{failed} show(s) could not finish syncing; see the failures above"
                );
                return Ok(());
            }
            if !self.next_scan(&cancel).await {
                return Ok(());
            }
        }
    }

    async fn sync_once(&self, slug: &str, cancel: CancellationToken) -> Result<Report> {
        let api = self.api(cancel)?;
        let engine = self.engine.clone();
        let slug = slug.to_owned();
        let runtime = tokio::runtime::Handle::current();
        let span = tracing::Span::current();
        // No embedded JS handle crosses its owning thread. Await the worker;
        // dropping its future would detach atomic writes during shutdown.
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            runtime.block_on(engine.once(&api, &slug))
        })
        .await?
    }

    #[tracing::instrument(name = "youtube.sync", skip_all, fields(show_slug = slug))]
    pub async fn sync(
        &self,
        slug: &str,
        watch: bool,
        cancel: CancellationToken,
        mut report: impl FnMut(&Report) + Send + 'static,
    ) -> Result<()> {
        let api = self.api(cancel)?;
        let engine = self.engine.clone();
        let slug = slug.to_owned();
        let runtime = tokio::runtime::Handle::current();
        let span = tracing::Span::current();
        // YouTube's embedded JS values have one thread owner. No JS handle crosses this boundary.
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            runtime.block_on(async move {
                if watch {
                    engine.watch(&api, &slug, report).await
                } else {
                    report(&engine.once(&api, &slug).await?);
                    Ok(())
                }
            })
        })
        .await?
    }
}

impl p::Show {
    pub fn youtube_linkage(&self) -> Option<&str> {
        self.youtube.url.as_deref()
    }
}
