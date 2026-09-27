//! Application-facing operations shared by native interfaces. Storage stays private.
use crate::{
    api::Api,
    auth,
    config::Config,
    downloads::DownloadManager,
    publicapi as p,
    sync::{Engine, Report},
};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct Catalog {
    pub teams: Vec<p::ClientTeam>,
    pub shows: Vec<p::Show>,
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
    pub fn downloads(&self) -> DownloadManager {
        self.engine.downloads.clone()
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
    pub fn dashboard_url(&self) -> &str {
        &self.config.dashboard_origin
    }
    pub fn show_url(&self, show: &p::Show) -> Result<String> {
        self.config.show_url(&show.team_id, &show.id)
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
        let (teams, shows) = tokio::try_join!(
            api.json(api.client().list_client_teams(), &[200]),
            api.json(api.client().list_shows(), &[200])
        )?;
        Ok(Catalog {
            teams: serde_json::from_value(teams)?,
            shows: serde_json::from_value(shows)?,
        })
    }
    pub async fn set_source(
        &self,
        slug: &str,
        url: Option<String>,
        cancel: CancellationToken,
    ) -> Result<p::Show> {
        crate::sync::set_source(&self.api(cancel)?, slug, url).await
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
