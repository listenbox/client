use crate::{MemberCommand, ShowCommand};
use anyhow::{Context, Result, bail, ensure};
use listenbox_sync_engine::{
    api::{Api, number, string},
    events::{Events, Progress},
    publicapi as p,
};
use serde_json::json;

pub async fn shows(api: &Api, command: ShowCommand) -> Result<()> {
    match command {
        ShowCommand::Order { show, episode } => {
            let order: p::EpisodeOrder = if episode.is_empty() {
                serde_json::from_value(
                    api.json(
                        api.client()
                            .get_episode_order(p::GetEpisodeOrderParams { show_slug: show }),
                        &[200],
                    )
                    .await?,
                )?
            } else {
                listenbox_sync_engine::sync::set_order(api, &show, episode, None).await?
            };
            for id in order.episode_ids {
                println!("{id}");
            }
        }
        ShowCommand::Sync {
            command: crate::SyncCommand::Youtube { show, watch },
        } => {
            let engine = listenbox_sync_engine::sync::Engine::default();
            let progress_stop = tokio_util::sync::CancellationToken::new();
            let progress = tokio::spawn(print_sync_progress(
                engine.downloads.clone(),
                progress_stop.clone(),
            ));
            let report = |report: &listenbox_sync_engine::sync::Report| {
                println!(
                    "{} added, {} removed, {} unchanged{}",
                    report.added,
                    report.removed,
                    report.unchanged,
                    if report.reordered {
                        "; feed order updated"
                    } else {
                        ""
                    }
                )
            };
            let result = if watch {
                engine.watch(api, &show, report).await
            } else {
                engine.once(api, &show).await.map(|value| report(&value))
            };
            progress_stop.cancel();
            progress.await?;
            result?;
        }
        ShowCommand::Source {
            show,
            youtube,
            disconnect: _,
        } => {
            let result = listenbox_sync_engine::sync::set_source(api, &show, youtube).await?;
            println!(
                "{}: {}",
                result.slug,
                result
                    .youtube_source_url
                    .as_deref()
                    .unwrap_or("source disconnected")
            );
        }
        ShowCommand::List => {
            let result = api.json(api.client().list_shows(), &[200]).await?;
            let shows: Vec<p::Show> = serde_json::from_value(result)?;
            for show in shows {
                println!("{}", show.slug);
            }
        }
        ShowCommand::Create {
            title,
            slug,
            source_kind,
            language,
            artwork,
        } => {
            let id = format!("shw_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
            let image_asset_id = match artwork {
                Some(path) => Some(upload_artwork(api, &id, &path).await?),
                None => None,
            };
            let request = api.client().create_show(p::CreateShowParams {
                body: p::CreateShow {
                    id,
                    title,
                    slug: slug.clone(),
                    language,
                    image_asset_id,
                    source_kind: serde_json::from_value(serde_json::to_value(source_kind)?)?,
                },
            });
            let response = api.send(request).await?;
            if response.status() == reqwest::StatusCode::CONFLICT {
                bail!("create show: slug {slug:?} already exists");
            }
            let show: p::Show = serde_json::from_value(api.accept(response, &[201]).await?)?;
            println!(
                "Created show {:?}\nOpen in Listenbox: {}",
                show.slug,
                api.config.show_url(&show.team_id, &show.id)?
            );
        }
        ShowCommand::Delete { show, yes } => {
            ensure!(yes, "shows delete: --yes is required");
            let result = api
                .json(
                    api.client()
                        .create_show_deletion(p::CreateShowDeletionParams {
                            show_slug: show.clone(),
                        }),
                    &[202],
                )
                .await?;
            delete_events(
                api,
                api.client()
                    .show_deletion_events(p::ShowDeletionEventsParams {
                        show_deletion_run_id: string(&result, "show_deletion_run_id")?.into(),
                    }),
                "show",
                "show_slug",
                &show,
            )
            .await?;
        }
    }
    Ok(())
}

async fn print_sync_progress(
    manager: listenbox_sync_engine::downloads::DownloadManager,
    stop: tokio_util::sync::CancellationToken,
) {
    use std::collections::HashMap;
    let mut changes = manager.changes();
    let mut printed = HashMap::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            changed = changes.changed() => if changed.is_err() { return; },
        }
        for item in manager.snapshot().items {
            let bucket = item
                .received()
                .saturating_mul(10)
                .checked_div(item.total)
                .unwrap_or(0);
            if printed.insert(item.id, (item.phase, bucket)) == Some((item.phase, bucket)) {
                continue;
            }
            let title: String = item
                .title
                .chars()
                .filter(|character| !character.is_control())
                .collect();
            eprintln!("{title}: {}", item.phase.label());
        }
    }
}

async fn upload_artwork(api: &Api, show: &str, path: &std::path::Path) -> Result<String> {
    let raw = std::fs::read(path).with_context(|| format!("read artwork {}", path.display()))?;
    let reader = image::ImageReader::new(std::io::Cursor::new(&raw)).with_guessed_format()?;
    let content_type = match reader.format() {
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::Png) => "image/png",
        _ => bail!("artwork must be a JPEG or PNG image"),
    };
    let (width, height) = reader
        .into_dimensions()
        .context("read artwork dimensions")?;
    ensure!(
        width == height && (1400..=3000).contains(&width),
        "upload a square image between 1400 and 3000 pixels. Attempted resolution: {width} × {height} pixels"
    );
    let result = api.json(api.client().create_image_upload_presign(p::CreateImageUploadPresignParams { body: serde_json::from_value(json!({
        "show_id": show, "byte_length": raw.len(), "content_type": content_type,
        "file_name": path.file_name().context("artwork filename")?.to_string_lossy(),
    }))? }), &[201]).await?;
    let response = api
        .send(
            api.http
                .request(
                    string(&result, "method")?.parse()?,
                    string(&result, "upload_url")?,
                )
                .header("Content-Type", content_type)
                .body(raw),
        )
        .await?;
    ensure!(
        response.status().is_success(),
        "upload artwork: HTTP {}",
        response.status()
    );
    let completed = api
        .json(
            api.client()
                .complete_image_upload(p::CompleteImageUploadParams {
                    image_asset_id: string(&result, "image_asset_id")?.into(),
                    body: p::CompleteImageUpload {
                        object_key: string(&result, "object_key")?.into(),
                    },
                }),
            &[201],
        )
        .await?;
    Ok(string(&completed, "id")?.into())
}

pub async fn import_rss(api: &Api, source: &str, slug: Option<&str>) -> Result<()> {
    let response = api
        .send(api.client().import_rss(p::ImportRSSParams {
            body: p::ImportRSSRequest {
                source_url: source.into(),
                slug: slug.map(str::to_owned),
            },
        }))
        .await?;
    if response.status() == reqwest::StatusCode::CONFLICT {
        if let Some(slug) = slug {
            bail!("requested slug {slug:?} conflicts");
        }
        bail!("import choice conflicts");
    }
    if response.status() != reqwest::StatusCode::ACCEPTED {
        return Err(api.response_error(response).await);
    }
    api.trace(&response, true)?;
    let created: p::CreatedPublicRSSImport = api.decode(response).await?;
    let mut stream = Events::open(
        api,
        api.client()
            .import_rss_run_events(p::ImportRSSRunEventsParams {
                import_run_id: created.import_run_id,
            }),
        false,
    )
    .await?;
    let mut progress = Progress::default();
    loop {
        let event = stream.next(api).await?;
        match string(&event, "type")? {
            "progress" => progress.update(number(&event, "percent")?)?,
            "terminal" => match string(&event, "status")? {
                "completed" => {
                    let slug =
                        crate::slug(string(&event, "show_slug")?).map_err(anyhow::Error::msg)?;
                    ensure!(
                        crate::valid_id(string(&event, "feed_id")?, "lb_"),
                        "invalid import feed ID"
                    );
                    let location = api
                        .config
                        .show_url(string(&event, "team_id")?, string(&event, "show_id")?)?;
                    progress.update(100)?;
                    println!("{slug}");
                    eprintln!("Open in Listenbox: {location}");
                    return Ok(());
                }
                "cancelled" => bail!("RSS import was cancelled"),
                "failed" => bail!(
                    "RSS import failed ({}): {}",
                    string(&event, "error_code")?,
                    string(&event, "error_message")?
                ),
                status => bail!("malformed import terminal status {status:?}"),
            },
            kind => bail!("malformed import event {kind:?}"),
        }
    }
}

pub async fn delete_events(
    api: &Api,
    request: reqwest_middleware::RequestBuilder,
    kind: &str,
    identity_key: &str,
    identity: &str,
) -> Result<()> {
    let mut stream = Events::open(api, request, false).await?;
    let mut progress = Progress::default();
    loop {
        let event = stream.next(api).await?;
        match string(&event, "type")? {
            "progress" => {
                string(&event, "current_step")?;
                progress.update(number(&event, "percent")?)?;
            }
            "terminal" => match string(&event, "status")? {
                "completed" => {
                    ensure!(
                        string(&event, identity_key)? == identity,
                        "deletion completed for another {kind}"
                    );
                    progress.update(100)?;
                    println!("Deleted {kind} {identity:?}");
                    return Ok(());
                }
                "failed" => bail!(
                    "delete {kind} failed ({}): {}",
                    string(&event, "error_code")?,
                    string(&event, "error_message")?
                ),
                status => bail!("malformed deletion terminal status {status:?}"),
            },
            kind => bail!("malformed deletion event {kind:?}"),
        }
    }
}

pub async fn members(api: &Api, command: MemberCommand) -> Result<()> {
    let client = api.client();
    match command {
        MemberCommand::List { show } => {
            let request = match &show {
                Some(show) => client.list_show_members(p::ListShowMembersParams {
                    show_slug: show.clone(),
                }),
                None => client.list_team_members(),
            };
            let result = api.json(request, &[200]).await?;
            for member in result["members"].as_array().context("missing members")? {
                if show.is_some() {
                    print!("{}\t", string(member, "access_source")?);
                }
                println!(
                    "{}\t{}\t{}",
                    string(member, "role")?,
                    string(&member["user"], "email")?,
                    string(&member["user"], "id")?
                );
            }
            for invite in result["invitations"]
                .as_array()
                .context("missing invitations")?
            {
                let expiry = chrono::DateTime::from_timestamp_millis(number(invite, "expires_at")?)
                    .context("invalid invitation expiry")?;
                println!(
                    "pending\t{}\t{}\t{}\t{}",
                    string(invite, "role")?,
                    string(invite, "email")?,
                    string(invite, "id")?,
                    expiry.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                );
            }
        }
        MemberCommand::Invite { email, role, show } => {
            let body = json!({"email": email.trim().to_lowercase(), "role": role});
            let request = match &show {
                Some(show) => client.create_show_invitation(p::CreateShowInvitationParams {
                    show_slug: show.clone(),
                    body: serde_json::from_value(body)?,
                }),
                None => client.create_team_invitation(p::CreateTeamInvitationParams {
                    body: serde_json::from_value(body)?,
                }),
            };
            let result = api.json(request, &[201]).await?;
            let target = show.map_or(String::new(), |show| format!(" to {show}"));
            println!(
                "Invited {}{target} as {}.",
                string(&result, "email")?,
                string(&result, "role")?
            );
        }
        MemberCommand::Role { member, role, show } => {
            let label = if show.is_some() {
                "show member"
            } else {
                "member"
            };
            let body = json!({"role": role});
            let request = match show {
                Some(show) => client.update_show_member_role(p::UpdateShowMemberRoleParams {
                    show_slug: show,
                    user_id: member.clone(),
                    body: serde_json::from_value(body.clone())?,
                }),
                None => client.update_team_member_role(p::UpdateTeamMemberRoleParams {
                    user_id: member.clone(),
                    body: serde_json::from_value(body.clone())?,
                }),
            };
            api.json(request, &[204]).await?;
            println!(
                "Changed {label} {member} to {}.",
                body["role"].as_str().unwrap_or("")
            );
        }
        MemberCommand::Remove { member, yes, show } => {
            let label = if show.is_some() {
                "show member"
            } else {
                "member"
            };
            ensure!(yes, "members remove: --yes is required");
            let request = match show {
                Some(show) => client.remove_show_member(p::RemoveShowMemberParams {
                    show_slug: show,
                    user_id: member.clone(),
                }),
                None => client.remove_team_member(p::RemoveTeamMemberParams {
                    user_id: member.clone(),
                }),
            };
            api.json(request, &[204]).await?;
            println!("Removed {label} {member}.");
        }
    }
    Ok(())
}
