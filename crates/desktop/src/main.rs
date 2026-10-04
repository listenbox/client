#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod artwork;
#[cfg(all(debug_assertions, feature = "hot-reload"))]
mod hot_reload;
mod http;
mod platform;
#[cfg(feature = "profiling")]
mod profiling;
mod quit;
mod tokens;
mod updater;
mod workspace;

use gpui_kit::component::Root;
use gpui_kit::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use listenbox_sync_engine::{client::Client, config::Config};
use std::sync::Arc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const APP_NAME: &str = env!("LISTENBOX_APP_NAME");

fn main() -> anyhow::Result<()> {
    let runtime = Arc::new(tokio::runtime::Runtime::new()?);
    let mut args = std::env::args_os().skip(1);
    let explicit = match args.next() {
        Some(flag) if flag == "--config" => {
            Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                anyhow::anyhow!("--config requires a path")
            })?))
        }
        Some(flag) if flag == "--help" => {
            println!("{APP_NAME} desktop\nUsage: listenbox-desktop [--config PATH]");
            return Ok(());
        }
        Some(flag) if flag == "--version" => {
            println!(
                "{APP_NAME} {} ({})",
                env!("CARGO_PKG_VERSION"),
                env!("LISTENBOX_SOURCE_COMMIT")
            );
            return Ok(());
        }
        Some(_) => anyhow::bail!("Usage: listenbox-desktop [--config PATH]"),
        None => None,
    };
    anyhow::ensure!(args.next().is_none(), "Unexpected desktop argument");
    #[cfg(target_os = "windows")]
    let _installation_lock = platform::InstallationLock::new()?;
    let config = Config::load(explicit.as_deref())?;
    if cfg!(debug_assertions) {
        eprintln!("Listenbox desktop profile: {}", config.directory.display());
        eprintln!(
            "Sync database: {}",
            config.directory.join("sync.sqlite").display()
        );
    }
    let client = Client::desktop(config.clone())?;
    let shutdown_signal = shutdown_signal(&runtime)?;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let tasks = TaskTracker::new();
    let drain = tasks.clone();
    let runtime_ui = runtime.clone();
    #[cfg(feature = "profiling")]
    let recorder = profiling::Recorder::open()?;
    #[cfg(feature = "profiling")]
    let recorder_tasks = TaskTracker::new();
    #[cfg(feature = "profiling")]
    let recorder_drain = recorder_tasks.clone();
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    application.on_reopen(platform::show_window);
    application.run(move |cx| {
        initialize(cx, &config, &runtime_ui).expect("initialize desktop");
        #[cfg(feature = "profiling")]
        if let Some(recorder) = recorder {
            // Recording lives through workspace logout and stops with the app.
            // It must not hold the workspace's pre-quit sync drain open.
            recorder.start(cx, &runtime_ui, cancel.clone(), &recorder_tasks);
        }
        let (quit_cancel, quit_tasks) = (cancel.clone(), tasks.clone());
        cx.on_app_quit(move |_| {
            quit_cancel.cancel();
            quit_tasks.close();
            let tasks = quit_tasks.clone();
            async move {
                tasks.wait().await;
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1080.), px(760.)), cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(840.), px(600.))),
                    titlebar: Some(gpui_kit::TitlebarOptions {
                        title: Some(APP_NAME.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    tokens::apply(window, cx);
                    #[cfg(feature = "profiling")]
                    window.set_debug_frame_overlay_mode(gpui_kit::DebugFrameOverlayMode::Full);
                    window
                        .observe_window_appearance(|window, cx| {
                            tokens::apply(window, cx);
                            window.refresh();
                        })
                        .detach();
                    let view = cx.new(|cx| {
                        workspace::Workspace::new(client, runtime_ui, cancel, tasks, window, cx)
                    });
                    platform::install(&view, window, cx);
                    #[cfg(all(debug_assertions, feature = "hot-reload"))]
                    hot_reload::connect(&view, window, cx);
                    cx.spawn(async move |cx| {
                        if shutdown_signal.await.is_ok() {
                            cx.update(|cx| cx.dispatch_action(&platform::Quit));
                        }
                    })
                    .detach();
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("open Listenbox window");
        })
        .detach();
        cx.activate(true);
    });
    stop.cancel();
    drain.close();
    // Also cover OS termination paths: GPUI bounds its own quit observers.
    runtime.block_on(drain.wait());
    #[cfg(feature = "profiling")]
    {
        recorder_drain.close();
        runtime.block_on(recorder_drain.wait());
    }
    updater::cleanup();
    Ok(())
}

fn initialize(
    cx: &mut gpui_kit::App,
    config: &Config,
    runtime: &Arc<tokio::runtime::Runtime>,
) -> anyhow::Result<()> {
    gpui_kit::init(cx);
    cx.set_http_client(Arc::new(http::DesktopHttpClient::new(
        &config.directory,
        runtime.handle().clone(),
    )?));
    Ok(())
}

fn shutdown_signal(
    runtime: &tokio::runtime::Runtime,
) -> anyhow::Result<tokio::sync::oneshot::Receiver<()>> {
    let _guard = runtime.enter();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        runtime.spawn(async move {
            tokio::select! {
                _ = terminate.recv() => {},
                _ = interrupt.recv() => {},
            }
            let _ = sender.send(());
        });
    }
    #[cfg(not(unix))]
    runtime.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = sender.send(());
        }
    });
    Ok(receiver)
}
