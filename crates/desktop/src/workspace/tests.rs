use super::*;
// The GPUI glob also re-exports its #[test] macro. Generated Rust test
// attributes must resolve to the built-in macro, not recursively to GPUI.
use core::prelude::v1::test;
use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt;
use listenbox_sync_engine::config::Config;
use std::{cell::Cell, rc::Rc, time::Duration};

// Runs against the real ephemeral Listenbox API from apps/api/e2e. Standalone
// checks do not silently substitute a fake API; the parent explicitly invokes it.
#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_backend(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let client = Client::desktop(Config::load(None).unwrap()).unwrap();
    let cancel = CancellationToken::new();
    cx.update(gpui_kit::init);
    cx.executor().allow_parking();
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
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    wait_for(cx, &workspace, |view| !view.loading).await;
    // Catalog loading starts sync automatically without an opt-in control.
    wait_for(cx, &workspace, |view| {
        view.jobs.is_empty() && !view.reports.is_empty()
    })
    .await;
    assert!(cx.update(|cx| {
        workspace
            .read(cx)
            .reports
            .values()
            .any(|report| report.starts_with("1 added"))
    }));
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert!(view.loaded, "catalog failed: {:?}", view.error);
        assert!(!view.catalog.teams.is_empty());
        assert_eq!(view.catalog.shows.len(), 1);
        window.render_frame(cx);
        assert_ne!(window.find("sync-now").disabled(), Some(true));
        assert!(window.try_find("keep-syncing").is_none());
        window.click("team-picker", cx);
        let team_id = workspace.read(cx).catalog.teams[0].id.clone();
        window.click(SharedString::from(format!("team-{team_id}")), cx);
        assert_eq!(workspace.read(cx).team.as_ref(), Some(&team_id));
        window.click("open-show", cx);
        window.click("sync-now", cx);
        assert_eq!(workspace.read(cx).jobs.len(), 1);
    })
    .unwrap();
    assert!(cx.opened_url().is_some());
    wait_for(cx, &workspace, |view| view.jobs.is_empty()).await;
    cx.update_window(handle.into(), |_, window, cx| {
        let view = workspace.read(cx);
        assert!(
            view.reports
                .values()
                .any(|report| report.starts_with("0 added")),
            "reports: {:?}",
            view.reports
        );
        window.render_frame(cx);
        assert_ne!(window.find("sync-now").disabled(), Some(true));
        assert!(window.try_find("stop-sync").is_none());
        window.click("pause-transfers", cx);
    })
    .unwrap();
    wait_for(cx, &workspace, |view| view.progress.paused).await;
    cx.update(|cx| cx.dispatch_action(&crate::platform::Logout));
    wait_for(cx, &workspace, |view| {
        view.stopping.is_none() && !view.loaded
    })
    .await;
    assert!(!cx.update(|cx| workspace.read(cx).client.has_credentials()));
    cancel.cancel();
}

#[gpui_kit::test]
#[ignore = "requires the parent workspace's ephemeral Listenbox services"]
async fn live_import(cx: &mut TestAppContext) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let config = Config::load(None).unwrap();
    let source = std::fs::read_to_string(config.directory.join("test-playlist-url")).unwrap();
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
        window.click("playlist-url", cx);
        window.input(&source, cx);
        window.click("start-import", cx);
        assert!(
            view.read(cx).importing,
            "import did not start: {:?}",
            view.read(cx).error
        );
    })
    .unwrap();
    wait_for(cx, &view, |view| !view.importing).await;
    cx.update_window(handle.into(), |_, window, cx| {
        let state = view.read(cx);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.catalog.shows.len(), 1);
        assert_eq!(
            state.show().unwrap().youtube_source(),
            Some(source.as_str())
        );
        assert!(
            state
                .reports
                .values()
                .any(|report| report.starts_with("1 added")),
            "{:?}",
            state.reports
        );
        window.render_frame(cx);
        assert!(window.try_find("save-source").is_none());
        window.click("sync-now", cx);
        assert_eq!(view.read(cx).jobs.len(), 1);
    })
    .unwrap();
    wait_for(cx, &view, |view| view.jobs.is_empty()).await;
    assert!(cx.update(|cx| {
        view.read(cx)
            .reports
            .values()
            .any(|report| report.starts_with("0 added"))
    }));
    cancel.cancel();
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
fn library_filters_connections_and_shares_artwork_requests(cx: &mut TestAppContext) {
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
    let shows = ["import", "none", "destination"].into_iter().map(|kind| {
        let mut connection = serde_json::json!({"kind":kind});
        if kind == "import" { connection["source_url"] = "https://www.youtube.com/playlist?list=PLabc".into(); }
        serde_json::from_value(serde_json::json!({
            "id":"shw_0123456789abcdef", "team_id":"team_0123456789abcdef", "slug":kind,
            "title":"Podcast artwork", "language":"en", "source_kind":"audio",
            "has_active_subscription":true, "image_url":"https://artwork.example.test/podcast.png", "youtube":connection
        })).unwrap()
    }).collect();
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
