//! Manual visual inspection of the production components with explicitly synthetic fixtures.
#![allow(dead_code)]
#[path = "../src/artwork.rs"]
mod artwork;
#[path = "../src/platform.rs"]
mod platform;
#[path = "../src/quit.rs"]
mod quit;
#[path = "../src/tokens.rs"]
mod tokens;
mod workspace {
    include!("../src/workspace.rs");

    pub fn show_quit_notice(view: &mut Workspace, cx: &App) {
        view.quit_notice = Some(crate::quit::QuitNotice::new(cx.background_executor().now()));
    }

    pub fn show_error(view: &mut Workspace) {
        view.import_open = true;
        view.error = Some("Enter a public YouTube playlist URL.".into());
        view.catalog.shows[0].image_url = Some("https://artwork.example.test/missing.png".into());
    }

    pub fn show_import(view: &mut Workspace) {
        view.import_open = true;
    }

    pub fn show_selected_team(view: &mut Workspace) {
        view.team = view.catalog.import_team.clone();
    }

    pub fn show_team_menu(view: &mut Workspace) {
        view.team_picker = true;
        view.catalog
            .teams
            .push(listenbox_sync_engine::publicapi::ClientTeam {
                id: "team_fedcba9876543210".into(),
                name: "Independent audio and video productions".into(),
            });
    }

    pub fn populate(view: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        view.catalog.teams = vec![listenbox_sync_engine::publicapi::ClientTeam {
            id: "team_0123456789abcdef".into(),
            name: "Field Notes Studio".into(),
        }];
        view.catalog.shows = vec![serde_json::from_value(serde_json::json!({
            "id": "shw_0123456789abcdef", "team_id": "team_0123456789abcdef",
            "title": "Field Notes", "slug": "field-notes", "language": "en", "source_kind": "audio",
            "has_active_subscription": true, "image_url":"https://artwork.example.test/podcast.png",
            "youtube": {"kind":"import", "source_url":"https://www.youtube.com/playlist?list=PLpreview"}
        })).unwrap()];
        view.catalog.import_team = Some("team_0123456789abcdef".into());
        for (id, slug, title) in [
            (
                "shw_0123456789abcdea",
                "workshop",
                "Conversations from the workshop",
            ),
            ("shw_0123456789abcdeb", "outside", "Outside the studio"),
        ] {
            let mut show = view.catalog.shows[0].clone();
            show.id = id.into();
            show.slug = slug.into();
            show.title = title.into();
            show.image_url = None;
            view.catalog.shows.push(show);
        }
        view.loaded = true;
        view.select(Some("field-notes".into()), window, cx);
        if let Some(cancel) = view.episode_cancel.take() {
            cancel.cancel();
        }
        view.episode_request += 1;
        view.episode_loading = false;
        view.episodes = vec![
            serde_json::from_value(serde_json::json!({
                "id":"ep_0123456789abcdef", "show_id":"shw_0123456789abcdef",
                "title":"A conversation about making things", "duration_seconds":1934,
                "status":"published"
            }))
            .unwrap(),
        ];
        let manager = view.client.downloads();
        let work = manager.enqueue(
            "field-notes",
            "Field Notes",
            &[
                (
                    "preview-a".into(),
                    "A conversation about making things".into(),
                ),
                ("preview-b".into(), "The longer way home".into()),
                ("preview-c".into(), "Recording outside the studio".into()),
            ],
        );
        work[0].phase(Phase::Downloading);
        work[0].start_download(24_000_000, 1_000_000);
        for start in (0..12_000_000).step_by(1_000_000) {
            work[0].range(
                start,
                1_000_000,
                listenbox_sync_engine::downloads::RangePhase::Complete,
            );
        }
        work[1].phase(Phase::Preparing);
        work[2].phase(Phase::Queued);
        view.progress = manager.snapshot();
    }
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use gpui_kit::component::{Root, Theme, ThemeMode};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, HeadlessAppContext, px, size};
    use listenbox_sync_engine::{client::Client, config::Config};
    use std::sync::Arc;
    use tokio_util::{sync::CancellationToken, task::TaskTracker};
    let profile = tempfile::tempdir()?;
    let runtime = Arc::new(tokio::runtime::Runtime::new()?);
    let text = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text,
        Arc::new(gpui_kit::assets::AllAssets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        cx.set_http_client(gpui_kit::http_client::FakeHttpClient::create(
            |request| async move {
                if request.uri().path().ends_with("missing.png") {
                    return Ok(gpui_kit::http_client::Response::builder()
                        .status(404)
                        .body(Default::default())?);
                }
                Ok(gpui_kit::http_client::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/png")
                    .body(gpui_kit::http_client::AsyncBody::from(
                        include_bytes!("../tests/fixtures/podcast.png").to_vec(),
                    ))?)
            },
        ))
    });
    std::fs::create_dir_all("dist/preview")?;
    for (name, mode, width, height, populated, quitting) in [
        ("welcome", ThemeMode::Light, 1080., 760., false, false),
        (
            "workspace-light",
            ThemeMode::Light,
            1080.,
            760.,
            true,
            false,
        ),
        ("workspace-dark", ThemeMode::Dark, 840., 760., true, false),
        ("team-menu", ThemeMode::Light, 840., 600., true, false),
        ("import-light", ThemeMode::Light, 1080., 760., true, false),
        ("import-dark", ThemeMode::Dark, 840., 600., true, false),
        ("import-error", ThemeMode::Light, 840., 600., true, false),
        ("quit-light", ThemeMode::Light, 1080., 760., false, true),
        ("quit-dark", ThemeMode::Dark, 840., 600., true, true),
    ] {
        let client = Client::desktop(Config::load_in(None, profile.path().into())?)?;
        let handle = cx.open_window(size(px(width), px(height)), |window, cx| {
            Theme::change(mode, Some(window), cx);
            tokens::project(cx);
            let view = cx.new(|cx| {
                let mut view = workspace::Workspace::new(
                    client,
                    runtime.clone(),
                    CancellationToken::new(),
                    TaskTracker::new(),
                    window,
                    cx,
                );
                if populated {
                    workspace::populate(&mut view, window, cx);
                }
                if name.starts_with("import-") {
                    workspace::show_import(&mut view);
                }
                if name == "import-error" {
                    workspace::show_error(&mut view);
                }
                if name == "team-menu" {
                    workspace::show_team_menu(&mut view);
                }
                if name == "workspace-dark" {
                    workspace::show_selected_team(&mut view);
                }
                if quitting {
                    workspace::show_quit_notice(&mut view, cx);
                }
                view
            });
            cx.new(|cx| Root::new(view, window, cx))
        })?;
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))?;
        cx.capture_screenshot(handle.into())?
            .save(format!("dist/preview/{name}.png"))?;
        cx.update_window(handle.into(), |_, window, _| window.remove_window())?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The visual capture target uses macOS Metal.");
}
