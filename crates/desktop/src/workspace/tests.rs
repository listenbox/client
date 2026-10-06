use super::*;
// The GPUI glob also re-exports its #[test] macro. Generated Rust test
// attributes must resolve to the built-in macro, not recursively to GPUI.
use core::prelude::v1::test;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use listenbox_sync_engine::config::Config;
use std::{cell::Cell, rc::Rc, time::Duration};

#[test]
#[ignore = "requires the parent API's isolated YouTube player"]
fn live_player_runtime() {
    let config = Config::load(None).unwrap();
    let slug = std::fs::read_to_string(config.directory.join("test-player-slug")).unwrap();
    let imported = config.directory.join("test-player-imported").exists();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let client = Client::desktop(config).unwrap();
        client
            .sync(&slug, false, CancellationToken::new(), move |report| {
                assert_eq!(report.added, if imported { 1 } else { 0 });
                assert_eq!(report.skipped, if imported { 0 } else { 1 });
            })
            .await
            .unwrap();
        let snapshot = client.downloads().snapshot();
        assert_eq!(snapshot.items.len(), 1);
        let item = &snapshot.items[0];
        assert_eq!(
            item.phase,
            if imported {
                Phase::Complete
            } else {
                Phase::Skipped
            }
        );
        if !imported {
            assert!(
                item.reason
                    .as_deref()
                    .unwrap()
                    .contains("RangeError: Maximum call stack size exceeded")
            );
        }
        client.catalog(CancellationToken::new()).await.unwrap();
    });
}

#[gpui_kit::test]
#[ignore = "requires the parent API's acknowledged-body upload gate"]
async fn live_upload_progress(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let control = std::fs::read_to_string(config.directory.join("test-upload-control")).unwrap();
    let api =
        listenbox_sync_engine::api::Api::new(config.clone(), CancellationToken::new()).unwrap();
    let client = Client::desktop(config).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let lifetime = CancellationToken::new();
    let _stop = lifetime.clone().drop_guard();
    let tasks = TaskTracker::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx
            .new(|cx| Workspace::new(client, runtime.clone(), lifetime, tasks.clone(), window, cx));
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| {
        view.progress
            .items
            .iter()
            .any(|item| item.phase == Phase::Uploading)
    })
    .await;
    runtime.block_on(async {
        api.http
            .get(control)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    });
    // Flush the live engine snapshot before reading the rendered progress.
    wait_for_import_state(cx, &view, &runtime, |view| {
        view.progress == view.client.downloads().snapshot()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let state = view.read(cx);
        let slug = &state.show().unwrap().slug;
        let progress = state.import_progress(slug);
        assert_eq!(
            progress.imported, 0,
            "the storage acknowledgement is still gated"
        );
        assert!(
            progress.percent() > 0.,
            "transmitted media still renders an empty progress bar"
        );
        window.render_frame(cx);
        window.click("pause-sync", cx);
    })
    .unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| view.jobs.is_empty()).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("sync-now").label(), Some("Resume"));
    })
    .unwrap();
    assert!(
        view.read_with(cx, |view, _| view
            .client
            .downloads()
            .snapshot()
            .items
            .iter()
            .all(|item| item.phase == Phase::Queued)),
        "Pause left an admitted transfer active"
    );
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_authorization_recovery(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let client = Client::desktop(config.clone()).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let tasks = TaskTracker::new();
    let lifetime = CancellationToken::new();
    let _stop_on_exit = lifetime.clone().drop_guard();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let window = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        let entity = cx.new(|cx| {
            Workspace::new(
                client.clone(),
                runtime.clone(),
                lifetime,
                tasks.clone(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    let http = reqwest::Client::new();
    let gate = |path: &str| {
        runtime.block_on(async {
            http.get(format!(
                "{}/__test__/authorization/{path}",
                config.api_origin
            ))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        });
    };
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("welcome-action", cx);
    })
    .unwrap();
    gate("started/1");
    cx.run_until_parked();
    let first_url = cx.opened_url().expect("sign-in did not open the browser");
    // The external browser has been abandoned without an approval or denial.
    // Reset the headless platform's URL so reopening must activate the control.
    cx.update(|cx| cx.open_url("about:blank"));
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("reopen-sign-in").is_some(),
            "pending sign-in has no browser recovery control"
        );
        window.click("reopen-sign-in", cx);
    })
    .unwrap();
    assert_eq!(cx.opened_url().as_deref(), Some(first_url.as_str()));
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("cancel-sign-in", cx);
    })
    .unwrap();
    gate("disconnected");
    wait_for(cx, &view, |view| view.authorization.is_none()).await;
    assert!(
        tasks.is_empty(),
        "cancelled authorization task did not finish"
    );
    assert!(!config.directory.join("auth.json").exists());
    assert!(
        cx.update(|cx| view.read(cx).error.is_none()),
        "cancellation displayed a sign-in error"
    );
    let first_code = reqwest::Url::parse(&first_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    gate(&format!("approve?code={first_code}"));
    assert!(
        !client.has_credentials(),
        "late approval restored a cancelled sign-in"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("welcome-action").label(),
            Some("Sign in to Listenbox")
        );
        window.click("welcome-action", cx);
    })
    .unwrap();
    gate("started/2");
    cx.run_until_parked();
    let second_url = cx.opened_url().unwrap();
    assert_ne!(
        first_url, second_url,
        "fresh sign-in reused the abandoned attempt"
    );
    let second_code = reqwest::Url::parse(&second_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    gate(&format!("approve?code={second_code}"));
    wait_for(cx, &view, |view| view.loaded).await;
    assert!(client.has_credentials());
    assert_eq!(cx.update(|cx| view.read(cx).catalog.teams.len()), 1);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("welcome-action").is_none());
        assert!(window.try_find("cancel-sign-in").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_authorization_cancel_starting(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let client = Client::desktop(config.clone()).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let tasks = TaskTracker::new();
    let lifetime = CancellationToken::new();
    let _stop_on_exit = lifetime.clone().drop_guard();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let window = cx.open_window(size(px(480.), px(600.)), |window, cx| {
        let entity = cx.new(|cx| {
            Workspace::new(
                client.clone(),
                runtime.clone(),
                lifetime,
                tasks.clone(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    let http = reqwest::Client::new();
    let gate = |path: &str| {
        runtime.block_on(async {
            http.get(format!(
                "{}/__test__/authorization/{path}",
                config.api_origin
            ))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        });
    };
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("welcome-action", cx);
    })
    .unwrap();
    gate("committed");
    assert!(
        cx.opened_url().is_none(),
        "browser opened before authorization was received"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "workspace-content",
            ScrollDelta::Pixels(point(px(0.), px(-400.))),
            cx,
        );
        window.render_frame(cx);
        let cancel = window.find("cancel-sign-in");
        assert!(
            cancel.visible(),
            "cancel control is hidden in a narrow window"
        );
        assert!(
            cancel.bounds().bottom() <= window.find("workspace-content").bounds().bottom(),
            "cancel control did not scroll into the viewport"
        );
        window.click("cancel-sign-in", cx);
    })
    .unwrap();
    assert!(
        cx.update(|cx| view
            .read(cx)
            .authorization
            .as_ref()
            .unwrap()
            .cancel
            .is_cancelled()),
        "cancel control did not cancel the authorization owner"
    );
    gate("disconnected");
    wait_for(cx, &view, |view| view.authorization.is_none()).await;
    assert!(tasks.is_empty());
    assert!(!client.has_credentials());
    assert!(
        cx.opened_url().is_none(),
        "cancelled authorization opened the browser"
    );
    assert!(cx.update(|cx| view.read(cx).error.is_none()));
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("welcome-action").label(),
            Some("Sign in to Listenbox")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_logout_contended_lock(cx: &mut TestAppContext) {
    use std::io::Write;
    #[derive(Clone)]
    struct LockEvents(tokio::sync::mpsc::UnboundedSender<()>);
    impl Write for LockEvents {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if String::from_utf8_lossy(bytes).contains("acquiring profile lock") {
                let _ = self.0.send(());
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (events, mut acquiring) = tokio::sync::mpsc::unbounded_channel();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(move || LockEvents(events.clone()))
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let config = Config::load(None).unwrap();
    let mode = std::fs::read_to_string(config.directory.join("test-logout-lock")).unwrap();
    let lock_path = if mode == "journal" {
        config.directory.join("sync-schema.lock")
    } else {
        let root = config.directory.join("youtube-cookies");
        std::fs::create_dir_all(&root).unwrap();
        root.join("write.lock")
    };
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock.lock().unwrap();
    let client = Client::desktop(config.clone()).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let tasks = TaskTracker::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let window = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        let entity = cx.new(|cx| {
            Workspace::new(
                client.clone(),
                runtime.clone(),
                CancellationToken::new(),
                tasks.clone(),
                window,
                cx,
            )
        });
        crate::platform::install_actions(&entity, cx);
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for(cx, &view, |view| view.loaded).await;
    if mode == "cookies" {
        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| view.open_settings(window, cx));
        })
        .unwrap();
        wait_for(cx, &view, |view| !view.cookie_busy).await;
        cx.update_window(window.into(), |_, window, cx| {
            view.update(cx, |view, cx| {
                view.cookie_input.update(cx, |input, cx| input.set_value(
                    "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tfixture-session\n", window, cx));
            });
            window.render_frame(cx);
            window.click("save-cookies", cx);
        }).unwrap();
    }
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), acquiring.recv())
            .await
            .expect("profile operation never reached lock acquisition")
            .unwrap();
    });
    cx.update(|cx| cx.dispatch_action(&crate::platform::Logout));
    let drained = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), tasks.wait())
            .await
            .is_ok()
    });
    if !drained {
        eprintln!(
            "logout did not cancel the contended {mode} lock; external owner is still holding it"
        );
        // The failing isolated driver must exit without hanging in Runtime::drop
        // on the very worker this regression is proving cannot terminate.
        std::process::exit(1);
    }
    wait_for(cx, &view, |view| view.stopping.is_none()).await;
    assert!(!client.has_credentials());
    assert!(tasks.is_empty());
    assert!(
        !config.directory.join("youtube-cookies/jar.json").exists(),
        "cancelled lock waiter committed cookies"
    );
    // The contender is released only after all assertions, never to help logout.
    drop(lock);
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_logout_initializing(cx: &mut TestAppContext) {
    use sha2::{Digest, Sha256};
    let config = Config::load(None).unwrap();
    let diagnostics = crate::diagnostics::open(&config.directory).unwrap();
    let slug = std::fs::read_to_string(config.directory.join("test-logout-slug")).unwrap();
    let client = Client::desktop(config.clone()).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let tasks = TaskTracker::new();
    let lifetime = CancellationToken::new();
    let _stop_on_exit = lifetime.clone().drop_guard();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    cx.open_window(size(px(840.), px(600.)), |window, cx| {
        let entity = cx.new(|cx| {
            Workspace::new(
                client.clone(),
                runtime.clone(),
                lifetime,
                tasks.clone(),
                window,
                cx,
            )
        });
        crate::platform::install_actions(&entity, cx);
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for(cx, &view, |view| view.jobs.contains_key(&slug)).await;
    runtime.block_on(async {
        reqwest::Client::new()
            .get(format!(
                "{}/__test__/youtube-initializing",
                config.api_origin
            ))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    });
    cx.update(|cx| cx.dispatch_action(&crate::platform::Logout));
    // This real-time guard bounds the drain even if the JS promise loses its
    // wakeup. It does not advance the stalled request or cancel any child.
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), tasks.wait())
            .await
            .expect("logout did not cancel the YouTube session owner promptly");
    });
    wait_for(cx, &view, |view| view.stopping.is_none()).await;
    assert!(!client.has_credentials(), "logout retained authentication");
    assert!(cx.update(|cx| view.read(cx).catalog.shows.is_empty()));
    let lock_key = hex::encode(Sha256::digest(format!("{}\0{slug}", config.api_origin)));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(config.directory.join(format!("sync-{lock_key}.lock")))
        .unwrap();
    lock.try_lock().expect("logout left the sync owner running");
    drop(diagnostics); // Flush the production writer before inspecting its journal.
    let events: Vec<serde_json::Value> = std::fs::read_dir(config.directory.join("diagnostics"))
        .unwrap()
        .flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(events.iter().any(
        |event| event["fields"]["message"] == "desktop shutdown completed"
            && event["fields"]["success"] == true
    ));
    for name in [
        "desktop.operation",
        "youtube.initialize",
        "http.request",
        "auth.logout",
    ] {
        for message in ["new", "close"] {
            assert!(
                events
                    .iter()
                    .any(|event| event["fields"]["message"] == message
                        && event["span"]["name"] == name),
                "production diagnostics omitted {message} for {name}"
            );
        }
    }
    assert!(events.iter().any(
        |event| event["fields"]["message"] == "desktop operations drained"
            && event["fields"]["dropped_log_lines"] == 0
    ));
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_artwork(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let slug = std::fs::read_to_string(config.directory.join("test-artwork-slug")).unwrap();
    cx.update(|cx| crate::initialize(cx, &config, &runtime).unwrap());
    cx.executor().allow_parking();
    let client = Client::desktop(config).unwrap();
    let catalog = runtime
        .block_on(client.catalog(CancellationToken::new()))
        .unwrap();
    let show = catalog
        .shows
        .iter()
        .find(|show| show.slug == slug)
        .expect("artwork show in catalog");
    let url = show
        .image_url
        .clone()
        .expect("catalog includes artwork URL");
    let image = cx
        .update(|cx| {
            let executor = cx.background_executor().clone();
            executor.spawn(ImageAssetLoader::load(Resource::Uri(url.into()), cx))
        })
        .await
        .expect("production desktop image loader must fetch and decode remote artwork");
    assert!(
        image
            .as_bytes(0)
            .is_some_and(|bytes| (1_000..20_000_000).contains(&bytes.len()))
    );
}

// Parent API E2E changes direction through the real destination endpoint at
// the inventory/catalog request gates. No fake API or injected UI message.
#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_server_direction(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let slug = std::fs::read_to_string(config.directory.join("test-direction-slug")).unwrap();
    let client = Client::desktop(config).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut workspace = None;
    let handle = cx.open_window(size(px(1080.), px(840.)), |window, cx| {
        let view = cx.new(|cx| {
            Workspace::new(
                client,
                runtime,
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    wait_for(cx, &workspace, |view| {
        view.loaded && view.catalog.shows.is_empty() && view.jobs.is_empty()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert!(view.selected.is_none());
        assert!(!view.reports.contains_key(&slug));
        assert!(view.error.is_none());
        window.render_frame(cx);
        assert!(
            window
                .try_find(SharedString::from(format!("show-{slug}")))
                .is_none()
        );
        window.dispatch_action(Box::new(crate::platform::Reload), cx);
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        !view.loading
            && view.catalog.shows.len() == 1
            && view.jobs.is_empty()
            && view.reports.contains_key(&slug)
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(workspace.read(cx).selected.as_deref(), Some(slug.as_str()));
        assert!(workspace.read(cx).error.is_none());
        window.render_frame(cx);
        assert!(
            window
                .try_find(SharedString::from(format!("show-{slug}")))
                .is_some()
        );
    })
    .unwrap();
    cancel.cancel();
}

// Runs against the real ephemeral Listenbox API from apps/api/e2e. Standalone
// checks do not silently substitute a fake API; the parent explicitly invokes it.
#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_episode_scrolling(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let slug = std::fs::read_to_string(config.directory.join("test-scroll-slug")).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let client = Client::desktop(config).unwrap();
    let catalog = runtime
        .block_on(client.catalog(CancellationToken::new()))
        .unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut workspace = None;
    let handle = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        tokens::apply(window, cx);
        let view = cx.new(|cx| {
            let mut view = Workspace::new(
                client,
                runtime,
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            );
            for show in &catalog.shows {
                view.reports.insert(
                    show.slug.clone(),
                    SyncReport::Notice("Previous sync saved".into()),
                );
            }
            view.selected = Some(slug);
            view
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    wait_for(cx, &workspace, |view| {
        view.loaded && !view.episode_loading && !view.source_loading
    })
    .await;
    let all_episodes = cx.update(|cx| {
        let view = workspace.read(cx);
        assert_eq!(
            view.episodes.len(),
            520,
            "the library stopped at a page boundary"
        );
        for (index, episode) in view.episodes.iter().enumerate() {
            assert_eq!(
                episode.title,
                format!(
                    "{}{number}",
                    "Wrapped episode title ".repeat(8),
                    number = index + 1
                )
            );
        }
        assert!(view.jobs.is_empty(), "scrolling fixture started a sync");
        view.episodes
            .iter()
            .map(|episode| episode.id.clone())
            .collect::<Vec<_>>()
    });
    let narrow_height = cx
        .update_window(handle.into(), |_, window, cx| {
            let start = std::time::Instant::now();
            for _ in 0..8 {
                window.render_frame(cx);
            }
            assert_podcast_header(window, 520);
            assert_eq!(
                window.find("last-synced").label(),
                Some("Not synced on this device yet")
            );
            eprintln!("episode scroll: eight initial frames {:?}", start.elapsed());
            let rendered = all_episodes
                .iter()
                .filter(|id| {
                    window
                        .try_find(SharedString::from(format!("episode-row-{id}")))
                        .is_some()
                })
                .count();
            eprintln!(
                "episode scroll: {rendered} mounted rows of {}",
                all_episodes.len()
            );
            assert!(rendered > 0, "no episode rows reached layout");
            assert!(
                rendered < 20,
                "all {rendered} episode rows were laid out outside the viewport"
            );
            // Drive actual wheel input through the variable-height content, rather
            // than setting the list's private offset or measuring every row first.
            for _ in 0..13 {
                window.scroll(
                    "workspace-content",
                    ScrollDelta::Pixels(point(px(0.), px(-4000.))),
                    cx,
                );
            }
            window.render_frame(cx);
            assert!(
                window.try_find("more-episodes").is_none(),
                "a complete library still asks for pagination"
            );
            let last = SharedString::from(format!("episode-row-{}", all_episodes.last().unwrap()));
            assert!(
                window.find(last).visible(),
                "scrolling skipped the last loaded episode"
            );
            window.scroll(
                "workspace-content",
                ScrollDelta::Pixels(point(px(0.), px(100_000.))),
                cx,
            );
            window.render_frame(cx);
            assert!(window.find("filter-episodes").visible());
            assert!(
                window
                    .find(SharedString::from(format!(
                        "episode-row-{}",
                        all_episodes[0]
                    )))
                    .visible()
            );
            // Drag the visible scrollbar through the real list, without
            // changing its private scroll offset or mounting off-screen rows.
            let bounds = window.find("workspace-content").bounds();
            let top = point(bounds.right() - px(8.), bounds.top() + px(12.));
            let bottom = point(bounds.right() - px(8.), bounds.bottom() - px(12.));
            window.drag(top, bottom, cx);
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(format!(
                        "episode-row-{}",
                        all_episodes.last().unwrap()
                    )))
                    .is_some_and(|row| row.visible()),
                "dragging the scrollbar did not reach the last episode"
            );
            window.drag(bottom, top, cx);
            window.render_frame(cx);
            assert!(
                window
                    .find(SharedString::from(format!(
                        "episode-row-{}",
                        all_episodes[0]
                    )))
                    .visible(),
                "dragging the scrollbar back did not reach the first episode"
            );
            for _ in 0..16 {
                window.focus_next(cx);
                window.render_frame(cx);
                if window.find("filter-episodes").focused() == Some(true) {
                    break;
                }
            }
            assert_eq!(window.find("filter-episodes").focused(), Some(true));
            for _ in 0..12 {
                window.scroll(
                    "workspace-content",
                    ScrollDelta::Pixels(point(px(0.), px(-4000.))),
                    cx,
                );
            }
            assert_eq!(
                window.find("filter-episodes").focused(),
                Some(true),
                "scrolling dropped filter keyboard focus"
            );
            assert!(
                !window.find("filter-episodes").visible(),
                "focused header was never scrolled out of view"
            );
            window.scroll(
                "workspace-content",
                ScrollDelta::Pixels(point(px(0.), px(100_000.))),
                cx,
            );
            window
                .find(SharedString::from(format!(
                    "episode-row-{}",
                    all_episodes[0]
                )))
                .bounds()
                .size
                .height
        })
        .unwrap();
    cx.simulate_window_resize(handle.into(), size(px(1080.), px(760.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let first = window.find(SharedString::from(format!(
            "episode-row-{}",
            all_episodes[0]
        )));
        assert!(first.visible(), "resize hid the first episode");
        assert!(
            first.bounds().size.height < narrow_height,
            "wrapped row did not remeasure after widening the window"
        );
        assert!(
            first.bounds().right() <= window.find("workspace-content").bounds().right(),
            "wrapped title overflowed after resize"
        );
    })
    .unwrap();
    cancel.cancel();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_not_imported(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.directory.join("test-not-imported.json")).unwrap(),
    )
    .unwrap();
    let slug = fixture["slug"].as_str().unwrap().to_owned();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let client = Client::desktop(config).unwrap();
    let catalog = runtime
        .block_on(client.catalog(CancellationToken::new()))
        .unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut workspace = None;
    let handle = cx.open_window(size(px(1080.), px(1040.)), |window, cx| {
        tokens::apply(window, cx);
        let view = cx.new(|cx| {
            let mut view = Workspace::new(
                client,
                runtime,
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            );
            // Gate the startup scan until the saved results have been inspected.
            for show in &catalog.shows {
                view.reports.insert(
                    show.slug.clone(),
                    SyncReport::Notice("Previous sync saved".into()),
                );
            }
            view.selected = Some(slug.clone());
            view
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert!(
            view.loading && !view.loaded,
            "startup catalog gate was not reached"
        );
        window.render_frame(cx);
        assert!(window.try_find("welcome-action").is_none());
        assert!(window.try_find("sign-in").is_none());
        assert!(
            workspace.read(cx).authorization.is_none(),
            "startup opened a second sign-in flow"
        );
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        view.loaded && !view.episode_loading && !view.source_loading
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let view = workspace.read(cx);
        assert_podcast_header(window, 1);
        if fixture["mode"].as_str() == Some("lost") {
            assert!(
                view.last_synced.is_none(),
                "failed pass recorded a successful sync"
            );
            assert_eq!(
                window.find("last-synced").label(),
                Some("Not synced on this device yet")
            );
        } else {
            assert!(
                view.last_synced.is_some(),
                "successful CLI sync did not survive desktop restart"
            );
            assert!(
                window
                    .find("last-synced")
                    .label()
                    .unwrap()
                    .starts_with("Last synced ")
            );
        }
        if matches!(fixture["mode"].as_str(), Some("checkpoint" | "failed_scan")) {
            let timestamp = view
                .last_synced
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .to_string();
            let path = Config::load(None)
                .unwrap()
                .directory
                .join("test-last-synced");
            if fixture["mode"].as_str() == Some("checkpoint") {
                std::fs::write(path, timestamp).unwrap();
            } else {
                assert_eq!(
                    timestamp,
                    std::fs::read_to_string(path).unwrap(),
                    "failed scan advanced last successful sync"
                );
            }
        }
    })
    .unwrap();
    if matches!(
        fixture["mode"].as_str(),
        Some("lost" | "success" | "checkpoint" | "failed_scan")
    ) {
        cx.update_window(handle.into(), |_, window, cx| {
            let view = workspace.read(cx);
            assert!(
                view.jobs.is_empty(),
                "persistence check resynced the source"
            );
            assert_eq!(view.source_items.len(), 1);
            if fixture["mode"].as_str() == Some("lost") {
                assert_eq!(view.source_items[0].phase, Phase::Queued);
            } else {
                assert_eq!(view.source_items[0].phase, Phase::Complete);
            }
            let rows = view.episode_rows();
            assert_eq!(
                rows.len(),
                1,
                "published source duplicated its failed transfer"
            );
            assert!(rows[0].episode.is_some());
            assert!(!rows[0].issue());
            window.render_frame(cx);
            assert!(
                window.try_find("filter-not-imported").is_none(),
                "published source retained the Not imported tab"
            );
        })
        .unwrap();
        cancel.cancel();
        return;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(workspace.read(cx).episodes.len(), 1);
        assert!(
            workspace.read(cx).jobs.is_empty(),
            "persistence check resynced the source"
        );
        window.render_frame(cx);
        assert!(
            window
                .find("open-playlist")
                .label()
                .unwrap()
                .starts_with("Open YouTube source:")
        );
        assert!(
            window.try_find("filter-not-imported").is_some(),
            "YouTube failures have no Not imported tab"
        );
        assert!(window.try_find("retry-not-imported").is_none());
        window.click("filter-not-imported", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("filter-not-imported").label(),
            Some("Not imported (4)")
        );
        let rows = workspace.read(cx).episode_rows();
        let issues: Vec<_> = rows.iter().filter(|row| row.issue()).collect();
        assert_eq!(issues.len(), 4);
        for (row, key) in issues
            .iter()
            .zip(["skipped", "no_stream", "signin", "claimed"])
        {
            let item = row.item.as_ref().unwrap();
            assert_eq!(item.phase, Phase::Skipped);
            assert_eq!(
                item.source_url,
                format!(
                    "https://www.youtube.com/watch?v={}",
                    fixture[key].as_str().unwrap()
                )
            );
            assert!(!item.title.is_empty());
            assert!(
                item.reason.is_some() || item.error.is_some(),
                "saved {key} failure lost its explanation"
            );
        }
        assert_eq!(
            issues[0].item.as_ref().unwrap().reason.as_deref(),
            Some("Video unavailable. It was blocked due to the claimed content by SME."),
            "saved YouTube interstitial lost the copyright explanation"
        );
        assert_eq!(
            issues[3].item.as_ref().unwrap().reason.as_deref(),
            Some("Video unavailable. It was blocked due to the claimed content by WMG."),
            "saved YouTube ERROR response lost the copyright explanation"
        );
        assert!(
            issues[2]
                .item
                .as_ref()
                .unwrap()
                .reason
                .as_ref()
                .unwrap()
                .contains("Settings")
        );
        assert!(
            issues[1]
                .item
                .as_ref()
                .unwrap()
                .reason
                .as_ref()
                .unwrap()
                .contains("no downloadable stream URLs")
        );
        for key in ["skipped", "no_stream", "signin", "claimed"] {
            let id = fixture[key].as_str().unwrap();
            assert!(
                window
                    .find(SharedString::from(format!("episode-row-{slug}/{id}")))
                    .visible(),
                "saved {key} video is not visible"
            );
        }
        let no_stream = fixture["no_stream"].as_str().unwrap();
        // Activate the title at the top of the actual row, rather than a
        // separate browser handoff control below the error.
        window.click_at(
            SharedString::from(format!("episode-row-{slug}/{no_stream}")),
            point(px(8.), px(20.)),
            cx,
        );
    })
    .unwrap();
    assert_eq!(
        cx.opened_url(),
        Some(format!(
            "https://www.youtube.com/watch?v={}",
            fixture["no_stream"].as_str().unwrap()
        ))
    );
    let other = fixture["other"].as_str().unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(SharedString::from(format!("show-{other}")), cx);
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        !view.episode_loading && !view.source_loading
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert_eq!(view.selected.as_deref(), Some(other));
        assert!(!view.show_issues, "podcast selection kept the old filter");
        assert!(
            view.episode_rows().is_empty(),
            "another podcast inherited failed imports"
        );
        window.render_frame(cx);
        assert!(
            window.try_find("filter-not-imported").is_none(),
            "empty podcast has a Not imported tab"
        );
        assert_eq!(window.find("episode-count").label(), Some("0 episodes"));
        assert_eq!(
            window.find("last-synced").label(),
            Some("Not synced on this device yet")
        );
        window.click(SharedString::from(format!("show-{slug}")), cx);
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        !view.episode_loading && !view.source_loading
    })
    .await;
    cx.update(|cx| {
        assert_eq!(
            workspace
                .read(cx)
                .episode_rows()
                .iter()
                .filter(|row| row.issue())
                .count(),
            4
        )
    });
    if fixture["mode"].as_str() == Some("recover") {
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("filter-not-imported", cx);
            window.render_frame(cx);
            window.click("retry-not-imported", cx);
            assert_eq!(
                workspace.read(cx).jobs.len(),
                1,
                "recovery sync was not admitted"
            );
            window.render_frame(cx);
            window.click("retry-not-imported", cx);
            assert_eq!(
                workspace.read(cx).jobs.len(),
                1,
                "retry admitted a duplicate sync"
            );
        })
        .unwrap();
        wait_for(cx, &workspace, |view| {
            view.jobs.is_empty() && !view.episode_loading && !view.source_loading
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            let progress = workspace.read(cx).client.downloads().snapshot();
            let source: Vec<_> = progress
                .items
                .iter()
                .filter(|item| item.source_id == slug)
                .collect();
            assert_eq!(
                source.len(),
                2,
                "progress omitted the previously imported episode"
            );
            assert!(source.iter().all(|item| item.phase == Phase::Complete));
            for key in ["video", "skipped"] {
                assert!(source.iter().any(|item| item.source_url
                    == format!(
                        "https://www.youtube.com/watch?v={}",
                        fixture[key].as_str().unwrap()
                    )));
            }
            let rows = workspace.read(cx).episode_rows();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|row| row.episode.is_some() && !row.issue()));
            window.render_frame(cx);
            assert!(
                window.try_find("filter-not-imported").is_none(),
                "successful sync retained the Not imported tab"
            );
            assert!(window.try_find("filter-episodes").is_some());
            assert_podcast_header(window, 2);
            assert!(workspace.read(cx).last_synced.is_some());
        })
        .unwrap();
    }
    cancel.cancel();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_backend(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let client = Client::desktop(Config::load(None).unwrap()).unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let menu_events =
        std::cell::RefCell::new(None::<tokio::sync::mpsc::UnboundedSender<muda::MenuId>>);
    let mut workspace = None;
    let handle = cx.open_window(size(px(1080.), px(760.)), |window, cx| {
        tokens::apply(window, cx);
        let view = cx.new(|cx| {
            Workspace::new(
                client,
                runtime.clone(),
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        crate::platform::install_actions(&view, cx);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        menu_events.replace(Some(crate::platform::native_menu::install(window, cx)));
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    wait_for(cx, &workspace, |view| !view.loading).await;
    // Catalog loading starts sync automatically without an opt-in control.
    wait_for(cx, &workspace, |view| {
        view.jobs.is_empty() && !view.reports.is_empty()
    })
    .await;
    wait_for(cx, &workspace, |view| {
        !view.episode_loading && !view.source_loading && !view.episodes.is_empty()
    })
    .await;
    cx.update(|cx| {
        let view = workspace.read(cx);
        assert_eq!(view.episodes[0].title, "Listenbox deterministic fixture");
        assert!(
            view.episodes[0]
                .duration_seconds
                .is_some_and(|duration| (1..=5).contains(&duration))
        );
        assert_eq!(
            view.progress.items[0].title,
            "Listenbox deterministic fixture"
        );
        assert_eq!(view.progress.items[0].duration_seconds, Some(2));
    });
    assert!(cx.update(|cx| {
        workspace
            .read(cx)
            .reports
            .values()
            .any(|report| matches!(report, SyncReport::Summary(summary) if summary.added == 1))
    }));
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert!(view.loaded, "catalog failed: {:?}", view.error);
        assert!(!view.catalog.teams.is_empty());
        assert_eq!(view.catalog.shows.len(), 1);
        assert!(view.last_synced.is_some());
        window.render_frame(cx);
        assert_podcast_header(window, 1);
        assert!(
            window
                .find("last-synced")
                .label()
                .unwrap()
                .starts_with("Last synced ")
        );
        assert_ne!(window.find("sync-now").disabled(), Some(true));
        assert!(window.try_find("keep-syncing").is_none());
        window.click("team-picker", cx);
        let team_id = workspace.read(cx).catalog.teams[0].id.clone();
        window.click(SharedString::from(format!("team-{team_id}")), cx);
        assert_eq!(workspace.read(cx).team.as_ref(), Some(&team_id));
        window.click("open-show", cx);
        window.click("open-playlist", cx);
        window.click("sync-now", cx);
        assert_eq!(workspace.read(cx).jobs.len(), 1);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url(),
        cx.update(|cx| workspace
            .read(cx)
            .show()
            .unwrap()
            .youtube_linkage()
            .map(str::to_owned))
    );
    wait_for(cx, &workspace, |view| {
        view.jobs.is_empty() && !view.episode_loading && !view.source_loading
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert!(
            view.reports
                .values()
                .any(|report| matches!(report, SyncReport::Summary(summary) if summary.added == 0)),
            "reports: {:?}",
            view.reports
        );
        window.render_frame(cx);
        assert_ne!(window.find("sync-now").disabled(), Some(true));
        assert_eq!(window.find("pause-sync").label(), Some("Pause"));
        assert!(window.try_find("pause-transfers").is_none());
        assert_podcast_header(window, 1);
        window.dispatch_action(native_menu_action("Settings…"), cx);
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        view.settings_open && !view.cookie_busy
    })
    .await;
    let request = cx.update(|cx| workspace.read(cx).episode_request);
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("close-settings", cx);
        assert!(!workspace.read(cx).settings_open);
        window.dispatch_action(native_menu_action("Reload"), cx);
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        !view.loading && view.episode_request > request
    })
    .await;
    let request = cx.update(|cx| workspace.read(cx).episode_request);
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-r"
            } else {
                "ctrl-r"
            },
            cx,
        );
    })
    .unwrap();
    wait_for(cx, &workspace, |view| {
        !view.loading && view.episode_request > request
    })
    .await;
    cx.update(|cx| {
        let (id, action) = native_menu_command("Log out");
        if let Some(sender) = menu_events.borrow().as_ref() {
            sender.send(id).unwrap();
        } else {
            cx.dispatch_action(action.as_ref());
        }
    });
    wait_for(cx, &workspace, |view| {
        view.stopping.is_none() && !view.loaded
    })
    .await;
    assert!(!cx.update(|cx| workspace.read(cx).client.has_credentials()));
    cancel.cancel();
}

fn native_menu_action(label: &str) -> Box<dyn Action> {
    native_menu_command(label).1
}

fn native_menu_command(label: &str) -> (muda::MenuId, Box<dyn Action>) {
    // A headless GPUI test has no AppKit main thread. Drive the same owned
    // command schema consumed by native menus; Linux also sends its stable ID
    // through the installed native event handler in live_backend.
    crate::platform::application_menus()
        .into_iter()
        .flat_map(|menu| menu.items)
        .find_map(|item| match item {
            gpui_kit::OwnedMenuItem::Action {
                name,
                action,
                disabled,
                ..
            } if name == label => {
                assert!(!disabled, "native menu command is disabled: {label}");
                Some((
                    muda::MenuId::new(format!("listenbox.app.{}", action.name())),
                    action,
                ))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing native menu command: {label}"))
}

fn assert_podcast_header(window: &mut Window, count: usize) {
    assert!(window.try_find("open-settings").is_none());
    assert!(window.try_find("reload").is_none());
    assert!(window.try_find("sign-in").is_none());
    assert!(
        window.try_find("sync-summary").is_none(),
        "idle sync retained its totals"
    );
    let label = if count == 1 {
        "1 episode".into()
    } else {
        format!("{count} episodes")
    };
    assert_eq!(window.find("episode-count").label(), Some(label.as_str()));
    assert!(window.find("episode-count").visible());
    assert!(window.find("last-synced").visible());
    let listenbox = window.find("open-show").bounds();
    let youtube = window.find("open-playlist").bounds();
    assert_eq!(
        listenbox.origin.y, youtube.origin.y,
        "source link was not placed beside Listenbox"
    );
    assert!(
        youtube.right() <= window.find("workspace-content").bounds().right(),
        "header links overflow the minimum window"
    );
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_concurrent_imports(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let sources = std::fs::read_to_string(config.directory.join("test-playlist-urls")).unwrap();
    let sources: Vec<_> = sources.lines().collect();
    let api =
        listenbox_sync_engine::api::Api::new(config.clone(), CancellationToken::new()).unwrap();
    let client = Client::desktop(config).unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime.clone(),
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for(cx, &view, |view| !view.loading).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).loaded, "{:?}", view.read(cx).error);
        window.render_frame(cx);
        window.click("youtube-url", cx);
        window.input(sources[0], cx);
        window.click("start-import", cx);
    })
    .unwrap();
    wait_for(cx, &view, |view| view.jobs.len() == 1).await;
    import_gate(&runtime, &api, "first-admitted");
    let first = cx
        .update_window(handle.into(), |_, window, cx| {
            let first = view.read(cx).show().unwrap().slug.clone();
            window.render_frame(cx);
            window.click("new-import", cx);
            assert!(
                view.read(cx).import_open,
                "another podcast's import prevented opening the import form"
            );
            window.click("youtube-url", cx);
            window.input(sources[1], cx);
            window.click("start-import", cx);
            first
        })
        .unwrap();
    wait_for(cx, &view, |view| view.jobs.len() == 2).await;
    import_gate(&runtime, &api, "second-admitted");
    let second = cx
        .update_window(handle.into(), |_, window, cx| {
            let second = view.read(cx).show().unwrap().slug.clone();
            assert_ne!(first, second);
            window.render_frame(cx);
            window.click("new-import", cx);
            assert!(
                view.read(cx).import_open,
                "simultaneous imports prevented opening the import form"
            );
            window.click("youtube-url", cx);
            window.input("https://www.youtube.com/playlist?list=PLnextdraft", cx);
            second
        })
        .unwrap();
    import_gate(&runtime, &api, "release-first");
    wait_for(cx, &view, |view| !view.jobs.contains_key(&first)).await;
    cx.update_window(handle.into(), |_, window, cx| {
        let state = view.read(cx);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(
            matches!(state.reports[&first], SyncReport::Summary(ref report) if report.added == 1),
            "{:?}",
            state.reports
        );
        assert!(
            !state.jobs[&second].is_cancelled(),
            "first completion stopped the second import"
        );
        assert!(
            state.import_open,
            "first completion closed the next import form"
        );
        assert_eq!(
            state.source.read(cx).value().as_str(),
            "https://www.youtube.com/playlist?list=PLnextdraft"
        );
        window.render_frame(cx);
        window.click(format!("show-{second}"), cx);
        window.click("pause-sync", cx);
    })
    .unwrap();
    wait_for(cx, &view, |view| view.jobs.is_empty()).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).error.is_none(), "{:?}", view.read(cx).error);
        assert!(matches!(view.read(cx).reports[&first], SyncReport::Summary(ref report) if report.added == 1));
        window.render_frame(cx);
        assert!(window.try_find("pause-sync").is_none());
        window.click("new-import", cx);
        assert!(view.read(cx).import_open);
        assert_eq!(
            view.read(cx).source.read(cx).value().as_str(),
            "https://www.youtube.com/playlist?list=PLnextdraft"
        );
    })
    .unwrap();
    cancel.cancel();
}

fn import_gate(
    runtime: &tokio::runtime::Runtime,
    api: &listenbox_sync_engine::api::Api,
    gate: &str,
) {
    runtime.block_on(async {
        let response = api
            .http
            .get(format!("{}/__test__/imports/{gate}", api.config.api_origin))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 204, "import gate {gate} failed");
    });
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_pause_imports(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let sources = std::fs::read_to_string(config.directory.join("test-playlist-urls")).unwrap();
    let sources: Vec<_> = sources.lines().collect();
    let api =
        listenbox_sync_engine::api::Api::new(config.clone(), CancellationToken::new()).unwrap();
    let client = Client::desktop(config).unwrap();
    let cancel = CancellationToken::new();
    let _cancel_on_exit = cancel.clone().drop_guard();
    let tasks = TaskTracker::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client.clone(),
                runtime.clone(),
                cancel.clone(),
                tasks.clone(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| !view.loading).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).loaded, "{:?}", view.read(cx).error);
        window.render_frame(cx);
        window.click("youtube-url", cx);
        window.input(sources[0], cx);
        window.click("start-import", cx);
    })
    .unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| {
        view.progress
            .items
            .iter()
            .filter(|item| item.attempt == 1)
            .count()
            == listenbox_sync_engine::downloads::MAX_IN_FLIGHT
            && view
                .progress
                .items
                .iter()
                .any(|item| item.phase == Phase::Resolving)
    })
    .await;
    let first = cx
        .update_window(handle.into(), |_, window, cx| {
            let first = view.read(cx).show().unwrap().slug.clone();
            window.render_frame(cx);
            window.click("new-import", cx);
            window.click("youtube-url", cx);
            window.input(sources[1], cx);
            window.click("start-import", cx);
            first
        })
        .unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| {
        view.jobs.len() == 2
            && view
                .progress
                .items
                .iter()
                .any(|item| item.source_id != first && item.phase == Phase::Queued)
    })
    .await;
    let second = cx
        .update_window(handle.into(), |_, window, cx| {
            let second = view.read(cx).show().unwrap().slug.clone();
            window.render_frame(cx);
            window.click(format!("show-{first}"), cx);
            assert_eq!(window.find("pause-sync").label(), Some("Pause"));
            window.click("pause-sync", cx);
            window.render_frame(cx);
            assert_eq!(window.find("pause-sync").label(), Some("Pausing…"));
            second
        })
        .unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| !view.jobs.contains_key(&first)).await;
    wait_for_import_state(cx, &view, &runtime, |view| !view.jobs.contains_key(&second)).await;
    let saved = runtime
        .block_on(client.sync_state(first.clone(), CancellationToken::new()))
        .unwrap()
        .items;
    assert_eq!(saved.len(), 32, "pause lost durable queued work");
    assert!(
        saved.iter().all(|item| item.phase == Phase::Queued),
        "{saved:?}"
    );
    cx.update_window(handle.into(), |_, _, cx| {
        let state = view.read(cx);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(
            matches!(&state.reports[&second], SyncReport::Summary(report) if report.added == 1),
            "{:?}",
            state.reports
        );
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.sync_all(false, cx));
        assert!(
            !view.read(cx).jobs.contains_key(&first),
            "automatic scan restarted the paused podcast"
        );
        assert!(
            view.read(cx).jobs.contains_key(&second),
            "pause disabled a sibling's automatic sync"
        );
        assert_eq!(window.find("sync-now").label(), Some("Resume"));
        assert!(window.try_find("pause-sync").is_none());
        window.dispatch_action(native_menu_action("Reload"), cx);
    })
    .unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| !view.loading).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("sync-now").label(),
            Some("Resume"),
            "Reload cleared the session pause"
        );
        assert!(!view.read(cx).jobs.contains_key(&first));
    })
    .unwrap();
    import_gate(&runtime, &api, "resume-paused");
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("sync-now", cx);
        assert!(
            view.read(cx).jobs.contains_key(&first),
            "Resume did not start the saved podcast"
        );
    })
    .unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| !view.jobs.contains_key(&first)).await;
    let resumed = runtime
        .block_on(client.sync_state(first.clone(), CancellationToken::new()))
        .unwrap()
        .items;
    assert_eq!(resumed.len(), 32);
    assert!(
        resumed.iter().all(|item| item.phase == Phase::Skipped),
        "Resume did not finish the saved queue: {resumed:?}"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("sync-now").label(), Some("Sync now"));
        window.click("pause-sync", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("sync-now").label(),
            Some("Resume"),
            "idle podcast could not be paused"
        );
    })
    .unwrap();
    cancel.cancel();
    tasks.close();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), tasks.wait())
            .await
            .unwrap()
    });
    let next_cancel = CancellationToken::new();
    let _cancel_next_session = next_cancel.clone().drop_guard();
    let next_tasks = TaskTracker::new();
    let mut next = None;
    cx.open_window(size(px(1080.), px(840.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime.clone(),
                next_cancel.clone(),
                next_tasks.clone(),
                window,
                cx,
            )
        });
        next = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let next = next.unwrap();
    wait_for_import_state(cx, &next, &runtime, |view| {
        view.loaded && view.jobs.is_empty() && view.reports.contains_key(&first)
    })
    .await;
    cx.update(|cx| {
        assert!(
            matches!(&next.read(cx).reports[&first], SyncReport::Summary(report) if report.skipped == 32),
            "a new app session did not sync the paused podcast: {:?}",
            next.read(cx).reports
        )
    });
    next_cancel.cancel();
    next_tasks.close();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), next_tasks.wait())
            .await
            .unwrap()
    });
}

async fn wait_for_import_state(
    cx: &mut TestAppContext,
    view: &Entity<Workspace>,
    runtime: &tokio::runtime::Runtime,
    predicate: impl Fn(&Workspace) -> bool,
) {
    let guard = runtime.spawn(async { tokio::time::sleep(Duration::from_secs(3)).await });
    let timed_out =
        match futures_util::future::select(Box::pin(wait_for(cx, view, predicate)), guard).await {
            futures_util::future::Either::Left(((), guard)) => {
                guard.abort();
                false
            }
            futures_util::future::Either::Right((_, pending)) => {
                drop(pending);
                true
            }
        };
    if timed_out {
        cx.update(|cx| {
            panic!(
                "import state did not arrive: jobs={:?} progress={:?}",
                view.read(cx).jobs,
                view.read(cx).progress.items
            )
        });
    }
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_video_creation_progress(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let source = std::fs::read_to_string(config.directory.join("test-playlist-url")).unwrap();
    let fail_scan = config.directory.join("test-video-scan-failure").exists();
    let api =
        listenbox_sync_engine::api::Api::new(config.clone(), CancellationToken::new()).unwrap();
    let client = Client::desktop(config).unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(1080.), px(840.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime.clone(),
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for(cx, &view, |view| !view.loading).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).loaded, "{:?}", view.read(cx).error);
        window.render_frame(cx);
        window.click("youtube-url", cx);
        window.input(&source, cx);
        window.click("import-video", cx);
        window.click("start-import", cx);
        window.render_frame(cx);
        assert!(window.find("creation-status").visible());
        assert!(window.try_find("start-import").is_none());
        assert!(window.try_find("youtube-url").is_none());
    })
    .unwrap();
    wait_for(cx, &view, |view| {
        view.creating.as_ref().is_some_and(|preparation| {
            preparation.preparation.stage == ImportStage::ScanningPlaylist
                && preparation
                    .preparation
                    .scan
                    .as_ref()
                    .is_some_and(|scan| scan.videos == 2)
        })
    })
    .await;
    cx.update(|cx| {
        let preparation = &view.read(cx).creating.as_ref().unwrap().preparation;
        let scan = preparation.scan.as_ref().unwrap();
        assert_eq!(scan.estimated_seconds, 900);
        assert_eq!(scan.unknown_durations, 1);
        assert_eq!(preparation.video_remaining_seconds, Some(72 * 3600));
        assert!(view.read(cx).catalog.shows.is_empty());
    });
    // Check the same real stalled scan at both window sizes and appearances.
    for dimensions in [size(px(840.), px(600.)), size(px(1080.), px(840.))] {
        cx.simulate_window_resize(handle.into(), dimensions);
        for mode in [
            gpui_kit::component::ThemeMode::Light,
            gpui_kit::component::ThemeMode::Dark,
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                gpui_kit::component::Theme::change(mode, Some(window), cx);
                tokens::project(cx);
                window.render_frame(cx);
                for id in [
                    "creation-status",
                    "creation-videos",
                    "creation-duration",
                    "creation-storage",
                ] {
                    let element = window.find(id);
                    assert!(
                        element.visible(),
                        "{id} was not visible at {dimensions:?} in {mode:?}"
                    );
                    assert!(
                        element.bounds().right()
                            <= window.find("workspace-content").bounds().right(),
                        "{id} overflowed the working pane"
                    );
                }
                assert_eq!(
                    window.find("creation-videos").label(),
                    Some("2 videos found so far")
                );
                assert_eq!(
                    window.find("creation-duration").label(),
                    Some("Known video duration: 15 min")
                );
                assert_eq!(
                    window.find("creation-storage").label(),
                    Some("Video storage available: 72 hr")
                );
                let was_open = view.read(cx).import_open;
                window.click("new-import", cx);
                window.input("changed while preparing", cx);
                assert_eq!(
                    view.read(cx).import_open,
                    was_open,
                    "preparation allowed opening another import form"
                );
                assert_eq!(
                    view.read(cx).source.read(cx).value().as_str(),
                    source.as_str()
                );
            })
            .unwrap();
        }
    }
    import_gate(&runtime, &api, "release-scan");
    if fail_scan {
        wait_for(cx, &view, |view| view.creating.is_none() && !view.loading).await;
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx);
            assert!(
                state.catalog.shows.is_empty(),
                "incomplete scan created a podcast"
            );
            assert!(
                state.jobs.is_empty(),
                "incomplete scan started importing episodes"
            );
            assert!(
                state
                    .error
                    .as_ref()
                    .is_some_and(|error| error.message.contains("503")),
                "{:?}",
                state.error
            );
            assert_eq!(state.source.read(cx).value().as_str(), source.as_str());
            window.render_frame(cx);
            assert!(window.try_find("creation-status").is_none());
            assert_ne!(window.find("start-import").disabled(), Some(true));
            assert!(window.find("youtube-url").visible());
        })
        .unwrap();
        cancel.cancel();
        return;
    }
    import_gate(&runtime, &api, "creation-requested");
    wait_for(cx, &view, |view| {
        view.creating.as_ref().is_some_and(|preparation| {
            preparation.preparation.stage == ImportStage::CreatingPodcast
        })
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).creating.is_some());
        assert!(view.read(cx).catalog.shows.is_empty());
        let scan = view
            .read(cx)
            .creating
            .as_ref()
            .unwrap()
            .preparation
            .scan
            .as_ref()
            .unwrap();
        assert_eq!(
            scan.videos, 3,
            "duplicate videos inflated preparation progress"
        );
        assert_eq!(
            scan.estimated_seconds, 2700,
            "known hidden durations were not included"
        );
        assert_eq!(scan.unknown_durations, 1);
        window.render_frame(cx);
        assert!(
            window.try_find("creation-status").is_some(),
            "playlist preparation gave no visible explanation of the wait"
        );
        assert!(
            window.try_find("creation-videos").is_some(),
            "the full scan gave no video count"
        );
        assert!(
            window.try_find("creation-storage").is_some(),
            "the video plan gave no available storage"
        );
        assert!(window.try_find("start-import").is_none());
        assert_eq!(
            window.find("creation-videos").label(),
            Some("3 videos found")
        );
        assert_eq!(
            window.find("creation-duration").label(),
            Some("Known video duration: 45 min")
        );
    })
    .unwrap();
    import_gate(&runtime, &api, "release-creation");
    wait_for(cx, &view, |view| {
        view.creating.is_none() && !view.loading && view.jobs.is_empty()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let state = view.read(cx);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.catalog.shows.len(), 1);
        assert!(
            state
                .reports
                .values()
                .any(|report| matches!(report, SyncReport::Summary(summary) if summary.added == 1)),
            "{:?}",
            state.reports
        );
        window.render_frame(cx);
        assert!(window.try_find("creation-status").is_none());
        assert!(window.try_find("sync-now").is_some());
    })
    .unwrap();
    cancel.cancel();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_team_import(cx: &mut TestAppContext) {
    let config = Config::load(None).unwrap();
    let values: HashMap<String, String> = serde_json::from_slice(
        &std::fs::read(config.directory.join("test-team-import.json")).unwrap(),
    )
    .unwrap();
    let origin = config.api_origin.clone();
    let dashboard = config.dashboard_origin.clone();
    let client = Client::desktop(config).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let cancel = CancellationToken::new();
    let _stop_on_exit = cancel.clone().drop_guard();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(1080.), px(760.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime.clone(),
                cancel,
                TaskTracker::new(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for_import_state(cx, &view, &runtime, |view| !view.loading).await;
    if values["mode"] == "readonly" {
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(view.read(cx).loaded, "{:?}", view.read(cx).error);
            assert!(view.read(cx).catalog.teams.is_empty());
            assert!(view.read(cx).catalog.import_team.is_none());
            window.render_frame(cx);
            window.click("new-import", cx);
            window.click("start-import", cx);
            assert!(view.read(cx).creating.is_none());
            assert_eq!(
                window.find("import-team").label(),
                Some("Choose a team with write access to create a podcast.")
            );
        })
        .unwrap();
        return;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).loaded, "{:?}", view.read(cx).error);
        window.render_frame(cx);
        window.click("team-picker", cx);
        if values["mode"] == "picker" {
            for id in [&values["reader"], &values["direct"]] {
                assert!(
                    window
                        .try_find(SharedString::from(format!("team-{id}")))
                        .is_none(),
                    "team without podcast creation permission appeared in the picker: {id}"
                );
            }
        }
        for id in [&values["original"], &values["destination"]] {
            assert!(
                window
                    .try_find(SharedString::from(format!("team-{id}")))
                    .is_some()
            );
        }
        window.click(
            SharedString::from(format!("team-{}", values["destination"])),
            cx,
        );
        assert_eq!(view.read(cx).team.as_ref(), Some(&values["destination"]));
    })
    .unwrap();
    if values["mode"] == "picker" {
        return;
    }
    let destination_label = cx.update(|cx| {
        let team = view
            .read(cx)
            .catalog
            .teams
            .iter()
            .find(|team| team.id == values["destination"])
            .unwrap();
        format!("Creates a new podcast in {}.", team.name)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("new-import", cx);
        assert_eq!(
            window.find("import-team").label(),
            Some(destination_label.as_str())
        );
        window.click("youtube-url", cx);
        window.input(&values["source"], cx);
        window.click("start-import", cx);
        assert!(view.read(cx).creating.is_some(), "import never started");
    })
    .unwrap();
    if values["mode"] == "payment" {
        wait_for_import_state(cx, &view, &runtime, |view| {
            view.creating.is_none() && !view.loading
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(
                view.read(cx)
                    .error
                    .as_ref()
                    .is_some_and(|error| error.message.starts_with("Could not create podcast."))
            );
            assert!(view.read(cx).catalog.shows.is_empty());
            window.render_frame(cx);
            window.click("upgrade-plan", cx);
        })
        .unwrap();
        assert_eq!(
            cx.opened_url(),
            Some(format!("{dashboard}/{}/upgrade", values["destination"]))
        );
        return;
    }
    let http = reqwest::Client::new();
    let gate = |path: &str| {
        runtime.block_on(async {
            http.get(format!("{origin}/__test__/team-import/{path}"))
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        });
    };
    gate("requested");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("team-picker", cx);
        window.click(
            SharedString::from(format!("team-{}", values["original"])),
            cx,
        );
        assert_eq!(view.read(cx).team.as_ref(), Some(&values["original"]));
        assert!(view.read(cx).creating.is_some());
        window.render_frame(cx);
        assert_eq!(
            window.find("import-team").label(),
            Some(destination_label.as_str())
        );
    })
    .unwrap();
    gate("release");
    wait_for_import_state(cx, &view, &runtime, |view| {
        view.creating.is_none()
            && view.jobs.is_empty()
            && (!view.reports.is_empty() || view.error.is_some())
            && !view.loading
    })
    .await;
    cx.update(|cx| {
        let state = view.read(cx);
        if values["mode"] == "revoked" {
            assert!(
                state
                    .error
                    .as_ref()
                    .is_some_and(|error| error.message.contains("permission denied")),
                "{:?}",
                state.error
            );
            assert!(state.catalog.shows.is_empty());
            assert!(state.jobs.is_empty());
            return;
        }
        assert!(state.error.is_none(), "{:?}", state.error);
        let show = state.show().expect("imported podcast was not selected");
        assert_eq!(
            show.team_id, values["destination"],
            "import ignored the selected team"
        );
        assert_eq!(show.youtube_linkage(), Some(values["source"].as_str()));
        assert!(
            state
                .reports
                .values()
                .any(|report| matches!(report, SyncReport::Summary(report) if report.added == 1))
        );
    });
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_import(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let source = std::fs::read_to_string(config.directory.join("test-playlist-url")).unwrap();
    let recover_scan = config.directory.join("test-import-scan-failure").exists();
    let repeat = config.directory.join("test-repeat-import").exists();
    let client = Client::desktop(config).unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime.clone(),
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for(cx, &view, |view| !view.loading).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).loaded, "{:?}", view.read(cx).error);
        assert!(
            view.read(cx).catalog.shows.is_empty(),
            "ordinary podcast leaked into sync library"
        );
        window.render_frame(cx);
        window.click("youtube-url", cx);
        window.input(&source, cx);
        window.click("start-import", cx);
        assert!(
            view.read(cx).creating.is_some(),
            "import did not start: {:?}",
            view.read(cx).error
        );
    })
    .unwrap();
    wait_for(cx, &view, |view| {
        view.creating.is_none() && !view.loading && view.jobs.is_empty()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let state = view.read(cx);
        if recover_scan {
            let error = state
                .error
                .as_ref()
                .expect("continuation failure was never observed");
            assert!(error.message.contains("was created"), "{error:?}");
            assert!(
                !state.import_open,
                "created podcast stayed on the import form"
            );
            assert!(
                state.source_items.is_empty(),
                "incomplete scan admitted media"
            );
        } else {
            assert!(state.error.is_none(), "{:?}", state.error);
        }
        assert_eq!(state.catalog.shows.len(), 1);
        assert_eq!(
            state.show().unwrap().youtube_linkage(),
            Some(source.as_str())
        );
        if !recover_scan {
            assert!(
                state.reports.values().any(
                    |report| matches!(report, SyncReport::Summary(summary) if summary.added == 1)
                ),
                "{:?}",
                state.reports
            );
        }
        window.render_frame(cx);
        assert!(window.try_find("save-source").is_none());
    })
    .unwrap();
    if repeat {
        let original = cx.update(|cx| view.read(cx).show().unwrap().clone());
        // Reusing an audio podcast also works when the new form requests video.
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("new-import", cx);
            view.update(cx, |view, cx| {
                view.import_kind = ShowSourceKind::Video;
                cx.notify();
            });
            window.render_frame(cx);
            window.click("youtube-url", cx);
            window.input(&source, cx);
            window.click("start-import", cx);
            assert!(
                view.read(cx).creating.is_some(),
                "repeat import never started"
            );
        })
        .unwrap();
        wait_for(cx, &view, |view| {
            view.creating.is_none() && !view.loading && view.jobs.is_empty()
        })
        .await;
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx);
            assert!(state.error.is_none(), "{:?}", state.error);
            assert!(!state.import_open, "repeat import stayed on the form");
            assert_eq!(state.catalog.shows.len(), 1);
            assert_eq!(state.show().unwrap().id, original.id);
            assert_eq!(state.show().unwrap().source_kind, original.source_kind);
            window.render_frame(cx);
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("sync-now", cx);
        assert_eq!(view.read(cx).jobs.len(), 1);
    })
    .unwrap();
    wait_for(cx, &view, |view| view.jobs.is_empty()).await;
    assert!(cx.update(|cx| {
        view.read(cx)
            .reports
            .values()
            .any(|report| matches!(report, SyncReport::Summary(summary) if summary.added == usize::from(recover_scan)))
    }));
    cancel.cancel();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_import_payment_required(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let source = std::fs::read_to_string(config.directory.join("test-playlist-url")).unwrap();
    let dashboard = config.dashboard_origin.clone();
    let client = Client::desktop(config).unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let handle = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        tokens::apply(window, cx);
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime,
                cancel.clone(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    wait_for(cx, &view, |view| !view.loading).await;
    let team = cx.update(|cx| {
        let state = view.read(cx);
        assert!(state.loaded, "catalog failed: {:?}", state.error);
        assert!(state.catalog.shows.is_empty());
        assert!(state.team.is_none(), "test must import from All teams");
        state.catalog.import_team.clone().unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("upgrade-plan").is_none());
        window.click("youtube-url", cx);
        window.input(&source, cx);
        window.click("import-video", cx);
        window.click("start-import", cx);
        assert!(view.read(cx).creating.is_some(), "import did not start");
    })
    .unwrap();
    wait_for(cx, &view, |view| {
        view.creating.is_none() && !view.loading && view.jobs.is_empty()
    })
    .await;
    cx.update_window(handle.into(), |_, window, cx| {
        let state = view.read(cx);
        assert!(
            state.error.is_some(),
            "unpaid import unexpectedly succeeded"
        );
        assert!(
            state
                .error
                .as_ref()
                .unwrap()
                .message
                .starts_with("Could not create podcast.")
        );
        assert!(state.catalog.shows.is_empty());
        assert_eq!(state.source.read(cx).value().as_str(), source);
        window.render_frame(cx);
        assert!(
            window.try_find("upgrade-plan").is_some(),
            "payment error has no upgrade action: {:?}",
            view.read(cx).error
        );
        window.click("upgrade-plan", cx);
    })
    .unwrap();
    assert_eq!(
        cx.opened_url(),
        Some(format!("{dashboard}/{team}/upgrade?family=video_hd"))
    );
    cx.update_window(handle.into(), |_, window, cx| {
        view.read(cx)
            .source
            .clone()
            .update(cx, |state, cx| state.set_value("invalid", window, cx));
        window.scroll(
            "youtube-url",
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            cx,
        );
        window.click("start-import", cx);
        window.scroll(
            "start-import",
            ScrollDelta::Pixels(point(px(0.), px(300.))),
            cx,
        );
        window.render_frame(cx);
        assert!(view.read(cx).error.is_some());
        assert!(
            window.try_find("upgrade-plan").is_none(),
            "validation retained the previous payment action"
        );
    })
    .unwrap();
    cancel.cancel();
}

#[gpui_kit::test]
fn unpaid_podcast_opens_its_teams_upgrade_page(cx: &mut TestAppContext) {
    let (_profile, handle, view) = quit_workspace(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.loaded = true;
            view.catalog.import_team = Some("team_0123456789abcdef".into());
            view.catalog.shows = vec![serde_json::from_value(serde_json::json!({
                "id": "shw_0123456789abcdef", "team_id": "team_fedcba9876543210",
                "title": "Podcast", "slug": "podcast", "language": "en", "source_kind": "audio",
                "has_active_subscription": false,
                "youtube": {"destination_status":"none", "url":"https://www.youtube.com/playlist?list=PLtest"}
            })).unwrap()];
            view.select(Some("podcast".into()), window, cx);
        });
        window.render_frame(cx);
        window.click("choose-plan", cx);
    }).unwrap();
    assert_eq!(
        cx.opened_url(),
        Some("https://web.listenbox.app/team_fedcba9876543210/upgrade".into())
    );
}

#[gpui_kit::test]
fn quit_hint_expires_without_a_key_up_event(cx: &mut TestAppContext) {
    let (_profile, window, view) = quit_workspace(cx);
    cx.dispatch_keystroke(window.into(), Keystroke::parse("cmd-q").unwrap());
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx).quit_notice.is_some(),
            "Cmd-Q did not show the hint"
        );
        assert!(view.read(cx).stopping.is_none());
    })
    .unwrap();
    // No key-up is delivered. The hint must stay readable, fade, then disappear.
    advance_quit_clock(cx, Duration::from_millis(1900));
    assert_eq!(
        cx.update(|cx| view
            .read(cx)
            .quit_notice
            .unwrap()
            .opacity(cx.background_executor().now())),
        1.
    );
    advance_quit_clock(cx, Duration::from_millis(200));
    let opacity = cx.update(|cx| {
        view.read(cx)
            .quit_notice
            .unwrap()
            .opacity(cx.background_executor().now())
    });
    assert!(
        (0.2..0.8).contains(&opacity),
        "the hint did not fade: {opacity}"
    );
    advance_quit_clock(cx, Duration::from_millis(200));
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            view.read(cx).quit_notice.is_none(),
            "the quit hint stayed visible after its two-second lifetime"
        );
        assert!(view.read(cx).stopping.is_none());
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(
            PlatformInput::KeyUp(KeyUpEvent {
                keystroke: Keystroke::parse("cmd-q").unwrap(),
            }),
            cx,
        );
        assert!(
            view.read(cx).stopping.is_none(),
            "a late key-up armed a stale quit"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_new_quit_hint_owns_its_full_lifetime(cx: &mut TestAppContext) {
    let (_profile, window, view) = quit_workspace(cx);
    cx.dispatch_keystroke(window.into(), Keystroke::parse("cmd-q").unwrap());
    advance_quit_clock(cx, Duration::from_millis(1500));
    cx.dispatch_keystroke(window.into(), Keystroke::parse("cmd-q").unwrap());
    // The previous attempt would have expired by now. It cannot dismiss this one.
    advance_quit_clock(cx, Duration::from_millis(800));
    assert_eq!(
        cx.update(|cx| view
            .read(cx)
            .quit_notice
            .unwrap()
            .opacity(cx.background_executor().now())),
        1.
    );
    assert!(cx.update(|cx| view.read(cx).stopping.is_none()));
    advance_quit_clock(cx, Duration::from_millis(1500));
    assert!(cx.update(|cx| view.read(cx).quit_notice.is_none()));
}

#[gpui_kit::test]
fn native_key_release_does_not_turn_a_tap_into_a_hold(cx: &mut TestAppContext) {
    let (_profile, _, view) = quit_workspace(cx);
    // The native key query reports a released key even though no KeyUp arrived.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.quit_pressed(Some(Box::new(|| false)), cx)
        })
    });
    // Even a delayed first poll is not evidence the shortcut was held.
    advance_quit_clock(cx, Duration::from_millis(2100));
    assert!(cx.update(|cx| !view.read(cx).quit_guard.is_held()));
    assert!(cx.update(|cx| view.read(cx).stopping.is_none()));
    advance_quit_clock(cx, Duration::from_millis(200));
    assert!(cx.update(|cx| view.read(cx).quit_notice.is_none()));
}

#[gpui_kit::test]
async fn held_shortcut_waits_for_release_and_drain(cx: &mut TestAppContext) {
    let (_profile, _, view) = quit_workspace(cx);
    let (tasks, runtime, cancel) = cx.update(|cx| {
        let view = view.read(cx);
        (
            view.tasks.clone(),
            view.runtime.clone(),
            view.cancel.clone(),
        )
    });
    let (admitted, started) = tokio::sync::oneshot::channel();
    let (commit, committed) = tokio::sync::oneshot::channel();
    tasks.spawn_on(
        async move {
            admitted.send(()).unwrap();
            cancel.cancelled().await;
            committed.await.unwrap();
        },
        runtime.handle(),
    );
    started.await.unwrap();
    let down = Rc::new(Cell::new(true));
    let key = down.clone();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.quit_pressed(Some(Box::new(move || key.get())), cx)
        })
    });
    advance_quit_clock(cx, crate::quit::HOLD);
    assert!(cx.update(|cx| view.read(cx).stopping.is_none()));
    advance_quit_clock(cx, crate::quit::FADE + crate::quit::KEY_POLL);
    assert!(cx.update(|cx| view.read(cx).quit_notice.is_none()));
    assert!(
        cx.update(|cx| view.read(cx).stopping.is_none()),
        "quitting while Q is held would send the shortcut to the next app"
    );
    down.set(false);
    advance_quit_clock(cx, crate::quit::KEY_POLL);
    assert!(cx.update(|cx| view.read(cx).stopping == Some(Shutdown::Quit)));
    assert_eq!(tasks.len(), 1, "shutdown did not wait for admitted work");
    commit.send(()).unwrap();
    // GPUI's headless platform deliberately stubs OS Quit. The observable
    // shutdown boundary here is the tracker becoming empty after the commit.
    tasks.wait().await;
    assert!(tasks.is_empty());
}

#[gpui_kit::test]
async fn logout_waits_for_admitted_work_to_drain(cx: &mut TestAppContext) {
    let profile = tempfile::tempdir().unwrap();
    let config = Config::load_in(None, profile.path().into()).unwrap();
    let client = Client::desktop(config).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let tasks = TaskTracker::new();
    let lifetime = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let window = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        let entity = cx
            .new(|cx| Workspace::new(client, runtime.clone(), lifetime, tasks.clone(), window, cx));
        crate::platform::install_actions(&entity, cx);
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    let view = view.unwrap();
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.dispatch_keystroke(window.into(), Keystroke::parse("cmd-q").unwrap());
    assert!(cx.update(|cx| view.read(cx).quit_notice.is_some()));
    assert!(cx.update(|cx| view.read(cx).stopping.is_none()));
    let cancel = cx.update(|cx| view.read(cx).cancel.clone());
    let (admitted, started) = tokio::sync::oneshot::channel();
    let (cancelled, cancellation) = tokio::sync::oneshot::channel();
    let (commit, committed) = tokio::sync::oneshot::channel();
    tasks.spawn_on(
        async move {
            admitted.send(()).unwrap();
            cancel.cancelled().await;
            cancelled.send(()).unwrap();
            committed.await.unwrap();
        },
        runtime.handle(),
    );
    started.await.unwrap();
    cx.update(|cx| cx.dispatch_action(&crate::platform::Logout));
    cancellation.await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(cx.update(|cx| view.read(cx).stopping == Some(Shutdown::Logout)));
    // This gate represents the worker finishing its already-admitted commit.
    commit.send(()).unwrap();
    wait_for(cx, &view, |view| view.stopping.is_none()).await;
    assert!(tasks.is_empty());
    assert!(
        !profile.path().join("sync.sqlite").exists(),
        "authentication opened the sync journal"
    );
}

async fn wait_for(
    cx: &mut TestAppContext,
    view: &Entity<Workspace>,
    predicate: impl Fn(&Workspace) -> bool,
) {
    use futures_util::StreamExt;
    let mut notifications = cx.notifications(view);
    loop {
        if cx.update(|cx| predicate(view.read(cx))) {
            return;
        }
        notifications
            .next()
            .await
            .expect("workspace closed before its expected state");
    }
}

fn quit_workspace(
    cx: &mut TestAppContext,
) -> (tempfile::TempDir, WindowHandle<Root>, Entity<Workspace>) {
    let profile = tempfile::tempdir().unwrap();
    let client = Client::desktop(Config::load_in(None, profile.path().into()).unwrap()).unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
    let mut view = None;
    let window = cx.open_window(size(px(840.), px(600.)), |window, cx| {
        let entity = cx.new(|cx| {
            Workspace::new(
                client,
                runtime,
                CancellationToken::new(),
                TaskTracker::new(),
                window,
                cx,
            )
        });
        view = Some(entity.clone());
        Root::new(entity, window, cx)
    });
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    (profile, window, view.unwrap())
}

fn advance_quit_clock(cx: &TestAppContext, elapsed: Duration) {
    // Drain runnable tasks without advancing to future timers automatically.
    while cx.executor().tick() {}
    cx.executor().advance_clock(elapsed);
    while cx.executor().tick() {}
}

#[gpui_kit::test]
fn library_shares_artwork_requests(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    cx.update(|cx| {
        cx.set_http_client(gpui_kit::http_client::FakeHttpClient::create(move |_| {
            requests.fetch_add(1, Ordering::SeqCst);
            async {
                Ok(gpui_kit::http_client::Response::builder()
                    .status(200)
                    .header("Content-Type", "image/png")
                    .body(gpui_kit::http_client::AsyncBody::from(
                        include_bytes!("../../tests/fixtures/podcast.png").to_vec(),
                    ))?)
            }
        }))
    });
    let (_profile, handle, view) = quit_workspace(cx);
    let shows = vec![serde_json::from_value(serde_json::json!({
            "id":"shw_0123456789abcdef", "team_id":"team_0123456789abcdef", "slug":"import",
            "title":"Podcast artwork", "language":"en", "source_kind":"audio",
            "has_active_subscription":true, "image_url":"https://artwork.example.test/podcast.png",
            "youtube":{"destination_status":"none", "url":"https://www.youtube.com/playlist?list=PLabc"}
        })).unwrap()];
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.receive(
                Message::Catalog(Ok(Catalog {
                    shows,
                    ..Default::default()
                })),
                window,
                cx,
            )
        });
        assert_eq!(view.read(cx).catalog.shows.len(), 1);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "sidebar and header must share one image request"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        let cache = view.read(cx).artwork_cache.clone();
        cache.update(cx, |cache, cx| cache.clear(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "reload must refresh stale artwork"
    );
}

#[gpui_kit::test]
async fn youtube_settings_validate_save_forget_and_link_to_guide(cx: &mut TestAppContext) {
    let (_profile, handle, view) = quit_workspace(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |view, cx| {
            view.error = Some(ErrorNotice::youtube_sign_in());
            cx.notify();
        });
        window.render_frame(cx);
        window.click("youtube-sign-in-settings", cx);
    })
    .unwrap();
    wait_for(cx, &view, |view| !view.cookie_busy).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(view.read(cx).settings_open);
        window.click("cookie-guide", cx);
        view.read(cx).cookie_input.clone().update(cx, |input, cx| {
            input.set_value("invalid export", window, cx)
        });
        window.scroll(
            "cookie-guide",
            ScrollDelta::Pixels(point(px(0.), px(-400.))),
            cx,
        );
        window.click("save-cookies", cx);
    })
    .unwrap();
    wait_for(cx, &view, |view| !view.cookie_busy).await;
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(listenbox_sync_engine::cookies::GUIDE_URL)
    );
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).cookie_error.is_some());
        assert!(!view.read(cx).client.cookie_jar().is_enabled().unwrap());
        view.read(cx).cookie_input.clone().update(cx, |input, cx| input.set_value("# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsynthetic-ui-cookie\n", window, cx));
        window.render_frame(cx);
        window.scroll("cookie-guide", ScrollDelta::Pixels(point(px(0.), px(-400.))), cx);
        window.click("save-cookies", cx);
    }).unwrap();
    wait_for(cx, &view, |view| !view.cookie_busy).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).client.cookie_jar().is_enabled().unwrap());
        assert!(view.read(cx).cookie_input.read(cx).value().is_empty());
        assert!(view.read(cx).cookie_error.is_none());
        window.render_frame(cx);
        window.scroll(
            "save-cookies",
            ScrollDelta::Pixels(point(px(0.), px(600.))),
            cx,
        );
        window.click("close-settings", cx);
        window.render_frame(cx);
        window.dispatch_action(Box::new(crate::platform::OpenSettings), cx);
    })
    .unwrap();
    wait_for(cx, &view, |view| !view.cookie_busy).await;
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(view.read(cx).cookie_saved);
        assert!(view.read(cx).cookie_input.read(cx).value().is_empty());
        window.render_frame(cx);
        window.scroll(
            "cookie-guide",
            ScrollDelta::Pixels(point(px(0.), px(-400.))),
            cx,
        );
        window.click("remove-cookies", cx);
    })
    .unwrap();
    wait_for(cx, &view, |view| !view.cookie_busy).await;
    assert!(!cx.update(|cx| view.read(cx).client.cookie_jar().is_enabled().unwrap()));
}
