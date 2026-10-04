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
    pub async fn sync_state(&self, slug: String) -> Result<SyncState> {
        if !self.has_credentials() {
            return Err(crate::api::AuthenticationRequired.into());
        }
        let (engine, config) = (self.engine.clone(), self.config.clone());
        tokio::task::spawn_blocking(move || {
            let database = engine.database(&config)?;
            Ok(SyncState {
                items: database.items(&config.api_origin, &slug)?,
                last_synced: database.last_synced(&config.api_origin, &slug)?,
            })
        })
        .await?
    }
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
            client.list_client_teams(),
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
        Ok(Catalog {
            teams,
            import_team: Some(account.team_id),
            shows,
        })
    }
    pub async fn import_collection(
        &self,
        source: &str,
        kind: p::ShowSourceKind,
        cancel: CancellationToken,
        event: impl FnMut(crate::youtube::ImportEvent) + Send + 'static,
    ) -> Result<(p::Show, Report)> {
        let source = crate::youtube::collection_source(source)?;
        let api = self.api(cancel)?;
        let engine = self.engine.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime.block_on(crate::youtube::import(
                &api, &engine, &source, None, kind, event,
            ))
        })
        .await?
    }
    pub async fn next_scan(&self, cancel: &CancellationToken) -> bool {
        self.engine.next_scan(cancel).await
    }

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
        // YouTube's embedded JS values have one thread owner. No JS handle crosses this boundary.
        tokio::task::spawn_blocking(move || {
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
