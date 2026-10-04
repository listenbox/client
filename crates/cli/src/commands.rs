use crate::{MemberCommand, ShowCommand};
use anyhow::{Context, Result, bail, ensure};
use listenbox_sync_engine::{
    api::{Api, number, string},
    events::{Events, Progress},
    publicapi as p,
};

pub async fn shows(api: &Api, command: ShowCommand) -> Result<()> {
    match command {
        ShowCommand::Order { show, episode } => {
            let order: p::EpisodeOrder = if episode.is_empty() {
                match api
                    .client()
                    .get_episode_order(p::GetEpisodeOrderParams { show_slug: show })
                    .await?
                {
                    p::GetEpisodeOrderResponse::Status200(value) => value,
                    response => return Err(api.response_error(response).await),
                }
            } else {
                listenbox_sync_engine::sync::set_order(api, &show, episode).await?
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
                if report.stopped {
                    println!(
                        "Sync stopped by Listenbox. Progress is saved; this show is no longer importing from YouTube."
                    );
                    return;
                }
                println!(
                    "{} added, {} removed, {} unchanged, {} skipped{}",
                    report.added,
                    report.removed,
                    report.unchanged,
                    report.skipped,
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
        ShowCommand::List => {
            let result = match api
                .client()
                .list_shows(p::ListShowsParams {
                    youtube_imports_only: None,
                })
                .await?
            {
                p::ListShowsResponse::Status200(value) => value,
                response => return Err(api.response_error(response).await),
            };
            let shows = result;
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
            let response = api
                .client()
                .create_show(p::CreateShowParams {
                    body: p::CreateShow {
                        id,
                        title,
                        slug: slug.clone(),
                        language,
                        image_asset_id,
                        youtube_url: None,
                        source_kind: serde_json::from_value(serde_json::to_value(source_kind)?)?,
                    },
                })
                .await?;
            let show = match response {
                p::CreateShowResponse::Status201(show) => show,
                p::CreateShowResponse::Status409(()) => {
                    bail!("create show: slug {slug:?} already exists")
                }
                response => return Err(api.response_error(response).await),
            };
            println!(
                "Created show {:?}\nOpen in Listenbox: {}",
                show.slug,
                api.config.show_url(&show.team_id, &show.id)?
            );
        }
        ShowCommand::Delete { show, yes } => {
            ensure!(yes, "shows delete: --yes is required");
            let result = match api
                .client()
                .create_show_deletion(p::CreateShowDeletionParams {
                    show_slug: show.clone(),
                })
                .await?
            {
                p::CreateShowDeletionResponse::Status202(value) => value,
                response => return Err(api.response_error(response).await),
            };
            delete_events(
                api,
                api.client()
                    .show_deletion_events(p::ShowDeletionEventsParams {
                        show_deletion_run_id: result.show_deletion_run_id,
                    })
                    .await?,
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
            if item.phase == listenbox_sync_engine::downloads::Phase::Complete && item.attempt == 0
            {
                continue;
            }
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
    match reader.format() {
        Some(image::ImageFormat::Jpeg | image::ImageFormat::Png) => {}
        _ => bail!("artwork must be a JPEG or PNG image"),
    };
    let (width, height) = reader
        .into_dimensions()
        .context("read artwork dimensions")?;
    ensure!(
        width == height && (1400..=3000).contains(&width),
        "upload a square image between 1400 and 3000 pixels. Attempted resolution: {width} × {height} pixels"
    );
    listenbox_sync_engine::artwork::upload(api, show, raw).await
}

pub async fn import_rss(api: &Api, source: &str, slug: Option<&str>) -> Result<()> {
    let response = api
        .client()
        .import_rss(p::ImportRSSParams {
            body: p::ImportRSSRequest {
                source_url: source.into(),
                slug: slug.map(str::to_owned),
            },
        })
        .await?;
    let created = match response {
        p::ImportRSSResponse::Status202(created) => created,
        p::ImportRSSResponse::Status409(()) => {
            if let Some(slug) = slug {
                bail!("requested slug {slug:?} conflicts");
            }
            bail!("import choice conflicts");
        }
        response => return Err(api.response_error(response).await),
    };
    let mut stream = Events::open(
        api,
        api.client()
            .import_rss_run_events(p::ImportRSSRunEventsParams {
                import_run_id: created.import_run_id,
            })
            .await?,
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

pub async fn delete_events<R: p::Response>(
    api: &Api,
    response: R,
    kind: &str,
    identity_key: &str,
    identity: &str,
) -> Result<()> {
    let mut stream = Events::open(api, response).await?;
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
        MemberCommand::List { show: Some(show) } => {
            let result = match client
                .list_show_members(p::ListShowMembersParams { show_slug: show })
                .await?
            {
                p::ListShowMembersResponse::Status200(result) => result,
                response => return Err(api.response_error(response).await),
            };
            for member in result.members {
                print!(
                    "{}\t",
                    match member.access_source {
                        p::ShowAccessSource::Team => "team",
                        p::ShowAccessSource::Show => "show",
                    }
                );
                print_member(&member.role, &member.user);
            }
            for invite in result.invitations {
                print_invitation(&invite.id, &invite.email, &invite.role, invite.expires_at)?;
            }
        }
        MemberCommand::List { show: None } => {
            let result = match client.list_team_members().await? {
                p::ListTeamMembersResponse::Status200(result) => result,
                response => return Err(api.response_error(response).await),
            };
            for member in result.members {
                print_member(&member.role, &member.user);
            }
            for invite in result.invitations {
                print_invitation(&invite.id, &invite.email, &invite.role, invite.expires_at)?;
            }
        }
        MemberCommand::Invite { email, role, show } => {
            let role = match role {
                crate::Role::Read => p::AssignableTeamRole::Read,
                crate::Role::Write => p::AssignableTeamRole::Write,
            };
            let email = email.trim().to_lowercase();
            let (email, role) = match &show {
                Some(show) => match client
                    .create_show_invitation(p::CreateShowInvitationParams {
                        show_slug: show.clone(),
                        body: p::CreateShowInvitation { email, role },
                    })
                    .await?
                {
                    p::CreateShowInvitationResponse::Status201(invite) => {
                        (invite.email, invite.role)
                    }
                    response => return Err(api.response_error(response).await),
                },
                None => match client
                    .create_team_invitation(p::CreateTeamInvitationParams {
                        body: p::CreateTeamInvitation { email, role },
                    })
                    .await?
                {
                    p::CreateTeamInvitationResponse::Status201(invite) => {
                        (invite.email, invite.role)
                    }
                    response => return Err(api.response_error(response).await),
                },
            };
            let target = show.map_or(String::new(), |show| format!(" to {show}"));
            println!("Invited {email}{target} as {}.", invitation_role(&role));
        }
        MemberCommand::Role { member, role, show } => {
            let label = if show.is_some() {
                "show member"
            } else {
                "member"
            };
            let role = match role {
                crate::Role::Read => p::AssignableTeamRole::Read,
                crate::Role::Write => p::AssignableTeamRole::Write,
            };
            match show {
                Some(show) => match client
                    .update_show_member_role(p::UpdateShowMemberRoleParams {
                        show_slug: show,
                        user_id: member.clone(),
                        body: p::UpdateShowMemberRole { role: role.clone() },
                    })
                    .await?
                {
                    p::UpdateShowMemberRoleResponse::Status204(()) => (),
                    response => return Err(api.response_error(response).await),
                },
                None => match client
                    .update_team_member_role(p::UpdateTeamMemberRoleParams {
                        user_id: member.clone(),
                        body: p::UpdateTeamMemberRole { role: role.clone() },
                    })
                    .await?
                {
                    p::UpdateTeamMemberRoleResponse::Status204(()) => (),
                    response => return Err(api.response_error(response).await),
                },
            }
            println!("Changed {label} {member} to {}.", invitation_role(&role));
        }
        MemberCommand::Remove { member, yes, show } => {
            let label = if show.is_some() {
                "show member"
            } else {
                "member"
            };
            ensure!(yes, "members remove: --yes is required");
            match show {
                Some(show) => match client
                    .remove_show_member(p::RemoveShowMemberParams {
                        show_slug: show,
                        user_id: member.clone(),
                    })
                    .await?
                {
                    p::RemoveShowMemberResponse::Status204(()) => (),
                    response => return Err(api.response_error(response).await),
                },
                None => match client
                    .remove_team_member(p::RemoveTeamMemberParams {
                        user_id: member.clone(),
                    })
                    .await?
                {
                    p::RemoveTeamMemberResponse::Status204(()) => (),
                    response => return Err(api.response_error(response).await),
                },
            }
            println!("Removed {label} {member}.");
        }
    }
    Ok(())
}

fn print_member(role: &p::TeamRole, user: &p::User) {
    let role = match role {
        p::TeamRole::Owner => "owner",
        p::TeamRole::Write => "write",
        p::TeamRole::Read => "read",
    };
    println!("{role}\t{}\t{}", user.email, user.id);
}
fn invitation_role(role: &p::AssignableTeamRole) -> &'static str {
    match role {
        p::AssignableTeamRole::Write => "write",
        p::AssignableTeamRole::Read => "read",
    }
}
fn print_invitation(
    id: &str,
    email: &str,
    role: &p::AssignableTeamRole,
    expires_at: i64,
) -> Result<()> {
    let expiry =
        chrono::DateTime::from_timestamp_millis(expires_at).context("invalid invitation expiry")?;
    println!(
        "pending\t{}\t{email}\t{id}\t{}",
        invitation_role(role),
        expiry.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    Ok(())
}
