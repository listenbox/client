use crate::publicapi as p;
use crate::{
    api::{Api, string},
    config::{credential_valid, write_private_json},
    events::Events,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

#[derive(Serialize, Deserialize)]
pub struct StoredAuth {
    pub api_key: String,
}

const SCOPES: &[&str] = &[
    "team:read",
    "team:manage",
    "show:read",
    "show:create",
    "show:update",
    "episode:create",
    "episode:delete",
    "episode:update",
    "episode:publish",
];

pub async fn login(api: &mut Api) -> Result<()> {
    login_with(api, p::AuthorizationClient::Cli, |url| {
        println!("Verification URL: {url}")
    })
    .await
}

pub async fn login_with(
    api: &mut Api,
    client: p::AuthorizationClient,
    mut open_browser: impl FnMut(&str),
) -> Result<()> {
    let mut response = api
        .request(
            api.client()
                .create_cli_authorization(p::CreateCLIAuthorizationParams {
                    body: serde_json::from_value(json!({"client": client, "scopes": SCOPES}))?,
                }),
        )
        .await?;
    if matches!(response, p::CreateCLIAuthorizationResponse::Status401(()))
        && api.credential.is_some()
    {
        api.credential = None;
        response = api
            .request(
                api.client()
                    .create_cli_authorization(p::CreateCLIAuthorizationParams {
                        body: serde_json::from_value(json!({"client": client, "scopes": SCOPES}))?,
                    }),
            )
            .await?;
    }
    let created = match response {
        p::CreateCLIAuthorizationResponse::Status201(created) => created,
        response => return Err(api.response_error(response).await),
    };
    let credential = &created.credential;
    ensure!(credential_valid(credential), "invalid pending credential");
    let mut verification = Url::parse(&created.verification_url)?;
    ensure!(
        matches!(verification.scheme(), "http" | "https")
            && verification.host_str().is_some()
            && verification.username().is_empty()
            && verification.password().is_none()
            && verification.fragment().is_none(),
        "invalid verification URL"
    );
    let dashboard = Url::parse(&api.config.dashboard_origin)?;
    verification
        .set_scheme(dashboard.scheme())
        .map_err(|()| anyhow::anyhow!("invalid dashboard scheme"))?;
    verification.set_host(dashboard.host_str())?;
    verification
        .set_port(dashboard.port())
        .map_err(|()| anyhow::anyhow!("invalid dashboard port"))?;
    open_browser(verification.as_str());
    let mut events = Events::open(
        api,
        api.client()
            .cli_authorization_events(p::CliAuthorizationEventsParams { code: created.code }),
    )
    .await?;
    let event = events.next(api).await?;
    match string(&event, "type")? {
        "cli.authorization.approved" => {
            api.credential = Some(credential.to_owned());
            let identity = match api.request(api.client().whoami()).await? {
                p::WhoamiResponse::Status200(value) => value,
                response => return Err(api.response_error(response).await),
            };
            ensure!(
                identity.team_id == string(&event, "team_id")?,
                "approved credential team does not match approved team"
            );
            let scopes: Vec<p::ApiKeyScope> = serde_json::from_value(json!(SCOPES))?;
            for scope in scopes {
                ensure!(
                    identity.scopes.contains(&scope),
                    "approved credential missing scope {scope:?}"
                );
            }
            write_private_json(
                &api.config.directory.join("auth.json"),
                &StoredAuth {
                    api_key: credential.into(),
                },
            )
        }
        "cli.authorization.denied" => bail!("Authorization was denied"),
        "cli.authorization.expired" => bail!("Authorization expired"),
        kind => bail!("unexpected authorization event {kind:?}"),
    }
}

/// Call after cancelling and joining application operations, so an in-flight
/// browser authorization cannot restore the credential after logout.
pub fn logout(config: &crate::config::Config) -> Result<()> {
    match std::fs::remove_file(config.directory.join("auth.json")) {
        Ok(()) => {
            #[cfg(unix)]
            std::fs::File::open(&config.directory)?.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Remove the shared Listenbox credential"),
    }
}

pub async fn status(api: &Api) -> Result<()> {
    let identity = match api.request(api.client().whoami()).await? {
        p::WhoamiResponse::Status200(value) => value,
        response => return Err(api.response_error(response).await),
    };
    let email = identity.email.trim();
    ensure!(!email.is_empty(), "identity email is empty");
    let name = identity
        .name
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        println!("Logged in as {email}");
    } else {
        println!("Logged in as {name} <{email}>");
    }
    Ok(())
}
