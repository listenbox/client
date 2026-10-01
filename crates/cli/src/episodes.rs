use crate::EpisodeCommand;
use anyhow::{Result, ensure};
use listenbox_sync_engine::{api::Api, publicapi as p};
use std::collections::HashSet;
pub async fn run(api: &Api, command: EpisodeCommand) -> Result<()> {
    match command {
        EpisodeCommand::List { show, limit } => {
            let mut cursor = None;
            let mut seen = HashSet::new();
            loop {
                let page = match api
                    .client()
                    .list_episodes(p::ListEpisodesParams {
                        show_slug: show.clone(),
                        limit: Some(i64::from(limit)),
                        cursor,
                    })
                    .await?
                {
                    p::ListEpisodesResponse::Status200(value) => value,
                    response => return Err(api.response_error(response).await),
                };
                for episode in page.episodes {
                    let id = episode.id;
                    ensure!(crate::valid_id(&id, "ep_"), "invalid episode ID in list");
                    println!("{id}");
                }
                cursor = page.next_cursor;
                let Some(next) = &cursor else {
                    return Ok(());
                };
                ensure!(
                    !next.is_empty() && seen.insert(next.clone()),
                    "empty or repeated episode list cursor"
                );
            }
        }
        EpisodeCommand::Create(args) => {
            listenbox_sync_engine::episodes::create(
                api,
                listenbox_sync_engine::episodes::EpisodeCreate {
                    show: args.show,
                    title: args.title,
                    description: args.description,
                    file: args.file,
                    publication: match args.publication {
                        crate::Publication::Draft => {
                            listenbox_sync_engine::episodes::Publication::Draft
                        }
                        crate::Publication::Publish => {
                            listenbox_sync_engine::episodes::Publication::Publish
                        }
                    },
                },
            )
            .await
        }
        EpisodeCommand::Delete { episode, yes } => {
            ensure!(yes, "episodes delete: --yes is required");
            let result = match api
                .client()
                .create_episode_deletion(p::CreateEpisodeDeletionParams {
                    episode_id: episode.clone(),
                })
                .await?
            {
                p::CreateEpisodeDeletionResponse::Status202(value) => value,
                response => return Err(api.response_error(response).await),
            };
            crate::commands::delete_events(
                api,
                api.client()
                    .episode_deletion_events(p::EpisodeDeletionEventsParams {
                        episode_deletion_run_id: result.episode_deletion_run_id,
                    })
                    .await?,
                "episode",
                "episode_id",
                &episode,
            )
            .await
        }
    }
}
