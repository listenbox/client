use crate::artwork::{ArtworkCache, artwork};
use crate::tokens::{self, Tokens};
use gpui_kit::component::{
    Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputState, TextareaState},
    progress::Progress,
    scroll::{ScrollableElement, Scrollbar, ScrollbarMode},
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use listenbox_sync_engine::{
    api::PaymentRequired,
    client::{Catalog, Client, SyncState},
    downloads::{Phase, Snapshot},
    publicapi::{Show, ShowSourceKind},
    sync::Report,
    youtube::{ImportEvent, ImportPreparation, ImportStage},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[path = "workspace/episodes.rs"]
mod episodes;
#[path = "workspace/import_progress.rs"]
mod import_progress;
#[path = "workspace/settings.rs"]
mod settings;

pub struct Workspace {
    episodes: Vec<listenbox_sync_engine::publicapi::EpisodeListItem>,
    episode_loading: bool,
    episode_error: Option<String>,
    episode_request: u64,
    episode_cancel: Option<CancellationToken>,
    source_items: Vec<listenbox_sync_engine::downloads::Download>,
    last_synced: Option<std::time::SystemTime>,
    source_loading: bool,
    source_error: Option<String>,
    show_issues: bool,
    episode_view: episodes::EpisodeList,
    client: Client,
    runtime: Arc<tokio::runtime::Runtime>,
    cancel: CancellationToken,
    lifetime: CancellationToken,
    tasks: TaskTracker,
    stopping: Option<Shutdown>,
    quit_guard: crate::quit::QuitGuard,
    quit_notice: Option<crate::quit::QuitNotice>,
    quit_task: Option<Task<()>>,
    focus: FocusHandle,
    sender: UnboundedSender<Message>,
    catalog: Catalog,
    loaded: bool,
    loading: bool,
    authenticating: bool,
    creating: Option<ImportPreparation>,
    auto_sync: bool,
    import_open: bool,
    import_kind: ShowSourceKind,
    artwork_cache: Entity<ArtworkCache>,
    team: Option<String>,
    team_picker: bool,
    selected: Option<String>,
    source: Entity<InputState>,
    settings_open: bool,
    cookie_input: Entity<TextareaState>,
    cookie_busy: bool,
    cookie_saved: bool,
    cookie_message: Option<String>,
    cookie_error: Option<String>,
    jobs: HashMap<String, CancellationToken>,
    paused: HashSet<String>,
    reports: HashMap<String, SyncReport>,
    error: Option<ErrorNotice>,
    progress: Snapshot,
}

#[derive(Debug)]
enum SyncReport {
    Summary(Report),
    Notice(String),
}

#[derive(Clone, Debug)]
struct ErrorNotice {
    message: String,
    upgrade_url: Option<String>,
    youtube_sign_in: bool,
}

impl From<String> for ErrorNotice {
    fn from(message: String) -> Self {
        Self {
            message,
            upgrade_url: None,
            youtube_sign_in: false,
        }
    }
}

impl ErrorNotice {
    fn youtube_sign_in() -> Self {
        Self { message: "YouTube is asking you to sign in. Add fresh cookies in Settings, then use Sync now to continue this podcast.".into(), upgrade_url: None, youtube_sign_in: true }
    }

    fn import(
        error: anyhow::Error,
        client: &Client,
        team: Option<&str>,
        created: bool,
        kind: &ShowSourceKind,
    ) -> Self {
        if error.is::<listenbox_sync_engine::cookies::SignInRequired>() {
            Self::youtube_sign_in()
        } else if let Some(payment) = error.downcast_ref::<PaymentRequired>() {
            let reason = if payment.0.is_empty() {
                "Payment is required to continue."
            } else {
                &payment.0
            };
            Self {
                message: format!(
                    "{} {reason}",
                    if created {
                        "Podcast created, but the import did not finish."
                    } else {
                        "Could not create podcast."
                    }
                ),
                youtube_sign_in: false,
                upgrade_url: team
                    .and_then(|team| client.upgrade_url(team).ok())
                    .map(|url| {
                        if *kind == ShowSourceKind::Video {
                            format!("{url}?family=video_hd")
                        } else {
                            url
                        }
                    }),
            }
        } else {
            format!("Import did not finish. {error:#}").into()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    Quit,
    Logout,
    Update,
}

enum Message {
    Drained(Shutdown, anyhow::Result<()>),
    Open(String),
    Catalog(anyhow::Result<Catalog>),
    Login(anyhow::Result<()>),
    ImportPreparation(ImportPreparation),
    ImportCreated(Show, CancellationToken),
    Imported {
        slug: Option<String>,
        kind: ShowSourceKind,
        result: anyhow::Result<(Show, Report)>,
    },
    Report(String, Report),
    Finished(String, anyhow::Result<()>),
    SyncDue,
    CookieStatus(anyhow::Result<bool>),
    CookiesSaved(bool, anyhow::Result<()>),
    Episodes(
        u64,
        anyhow::Result<Vec<listenbox_sync_engine::publicapi::EpisodeListItem>>,
    ),
    SyncState(u64, anyhow::Result<SyncState>),
}

impl Workspace {
    pub fn new(
        client: Client,
        runtime: Arc<tokio::runtime::Runtime>,
        cancel: CancellationToken,
        tasks: TaskTracker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (sender, mut receiver) = unbounded_channel();
        let source = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://www.youtube.com/@your-channel")
        });
        cx.spawn_in(window, async move |view, cx| {
            while let Some(message) = receiver.recv().await {
                if view
                    .update_in(cx, |view, window, cx| view.receive(message, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let downloads = client.downloads();
        let mut changes = downloads.changes();
        cx.spawn(async move |view, cx| {
            while changes.changed().await.is_ok() {
                let snapshot = downloads.snapshot();
                if view
                    .update(cx, |view, cx| {
                        view.progress = snapshot;
                        view.episode_view.dirty = true;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let mut view = Self {
            episodes: vec![],
            episode_loading: false,
            episode_error: None,
            episode_request: 0,
            episode_cancel: None,
            source_items: vec![],
            last_synced: None,
            source_loading: false,
            source_error: None,
            show_issues: false,
            episode_view: episodes::EpisodeList::new(cx),
            client,
            runtime,
            lifetime: cancel.clone(),
            cancel: cancel.child_token(),
            tasks,
            stopping: None,
            quit_guard: Default::default(),
            quit_notice: None,
            quit_task: None,
            focus: cx.focus_handle(),
            sender,
            catalog: Catalog::default(),
            loaded: false,
            loading: false,
            authenticating: false,
            creating: None,
            auto_sync: false,
            import_open: false,
            import_kind: ShowSourceKind::Audio,
            artwork_cache: ArtworkCache::new(cx),
            team: None,
            team_picker: false,
            selected: None,
            source,
            settings_open: false,
            cookie_input: settings::cookie_input(window, cx),
            cookie_busy: false,
            cookie_saved: false,
            cookie_message: None,
            cookie_error: None,
            jobs: HashMap::new(),
            paused: HashSet::new(),
            reports: HashMap::new(),
            error: None,
            progress: Snapshot::default(),
        };
        view.focus.focus(window, cx);
        if view.client.has_credentials() {
            view.reload(cx);
        }
        view
    }

    pub fn shutdown(&mut self, mode: Shutdown, cx: &mut Context<Self>) {
        if let Some(stopping) = &mut self.stopping {
            if mode == Shutdown::Update {
                *stopping = mode;
            }
            return;
        }
        self.stopping = Some(mode);
        self.quit_notice = None;
        self.quit_task = None;
        self.quit_guard = Default::default();
        self.cancel.cancel();
        self.tasks.close();
        let (tasks, client, sender) =
            (self.tasks.clone(), self.client.clone(), self.sender.clone());
        // This join is outside the tracker it waits for. No task is aborted: each
        // worker finishes admitted file writes, FFmpeg cleanup and SQLite commits.
        self.runtime.spawn(async move {
            tasks.wait().await;
            let result = if mode == Shutdown::Logout {
                client.logout()
            } else {
                Ok(())
            };
            let _ = sender.send(Message::Drained(mode, result));
        });
        cx.notify();
    }

    fn quit_pressed(&mut self, key_down: Option<Box<dyn Fn() -> bool>>, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.quit_guard.press(now, false);
        self.quit_notice = Some(crate::quit::QuitNotice::new(now));
        // One owned task per attempt: a fresh press cancels the previous timer.
        // The notice's lifetime never depends on delivery of a key-up event.
        self.quit_task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(crate::quit::KEY_POLL).await;
                let active = view
                    .update(cx, |view, cx| {
                        let now = cx.background_executor().now();
                        if view.quit_guard.is_held()
                            && let Some(key_down) = &key_down
                        {
                            if !key_down() {
                                view.quit_released(cx);
                            } else if view.quit_guard.holding(now)
                                && let Some(notice) = &mut view.quit_notice
                            {
                                notice.instruction = "Release ⌘Q to quit";
                            }
                        }
                        if view
                            .quit_notice
                            .is_some_and(|notice| notice.opacity(now) == 0.)
                        {
                            view.quit_notice = None;
                            // With no native key state, a missing release cannot leave
                            // a stale hold armed for an unrelated future key-up.
                            if key_down.is_none() {
                                view.quit_guard = Default::default();
                            }
                        }
                        cx.notify();
                        view.stopping.is_none()
                            && (view.quit_notice.is_some() || view.quit_guard.is_held())
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn quit_released(&mut self, cx: &mut Context<Self>) {
        if self.quit_guard.release() {
            self.shutdown(Shutdown::Quit, cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.stopping.is_some() {
            return;
        }
        self.loading = true;
        self.error = None;
        let (client, sender, cancel) = (
            self.client.clone(),
            self.sender.clone(),
            self.cancel.child_token(),
        );
        self.tasks.spawn_on(
            async move {
                let _ = sender.send(Message::Catalog(client.catalog(cancel).await));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    fn login(&mut self, cx: &mut Context<Self>) {
        if self.authenticating || self.loading || self.stopping.is_some() {
            return;
        }
        self.authenticating = true;
        self.error = None;
        let (client, sender, cancel) = (
            self.client.clone(),
            self.sender.clone(),
            self.cancel.child_token(),
        );
        self.tasks.spawn_on(
            async move {
                let result = client
                    .login(cancel, |url| {
                        let _ = sender.send(Message::Open(url.into()));
                    })
                    .await;
                let _ = sender.send(Message::Login(result));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    fn receive(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        if self.stopping.is_some() && !matches!(message, Message::Drained(..)) {
            return;
        }
        self.episode_view.dirty = true;
        match message {
            Message::CookieStatus(result) => {
                self.cookie_busy = false;
                match result {
                    Ok(saved) => self.cookie_saved = saved,
                    Err(error) => self.cookie_error = Some(error.to_string()),
                }
            }
            Message::CookiesSaved(saved, result) => {
                self.cookies_received(saved, result, window, cx)
            }
            Message::Episodes(request, result) => self.episodes_received(request, result),
            Message::SyncState(request, result) => {
                if request == self.episode_request {
                    self.source_loading = false;
                    match result {
                        Ok(state) => {
                            self.source_items = state.items;
                            self.last_synced = state.last_synced;
                        }
                        Err(error) => {
                            self.source_error =
                                Some(format!("Could not load saved sync status. {error:#}"))
                        }
                    }
                }
            }
            Message::Drained(mode, result) => {
                let mode = self.stopping.unwrap_or(mode);
                if mode == Shutdown::Update {
                    crate::updater::drained(cx);
                    return;
                }
                if mode == Shutdown::Quit {
                    crate::updater::quit(cx);
                    return;
                }
                self.cancel = self.lifetime.child_token();
                self.tasks.reopen();
                self.stopping = None;
                self.authenticating = false;
                self.loading = false;
                self.creating = None;
                self.auto_sync = false;
                self.import_open = false;
                self.artwork_cache
                    .update(cx, |cache, cx| cache.clear(window, cx));
                self.jobs.clear();
                self.paused.clear();
                self.reports.clear();
                match result {
                    Ok(()) => {
                        self.catalog = Catalog::default();
                        self.loaded = false;
                        self.team = None;
                        self.team_picker = false;
                        self.select(None, window, cx);
                        self.progress = Snapshot::default();
                    }
                    Err(error) => {
                        self.error = Some(format!("Could not sign out. {error:#}").into())
                    }
                }
            }
            Message::SyncDue => {
                self.sync_all(false, cx);
                self.reload(cx);
            }
            Message::Open(url) => cx.open_url(&url),
            Message::Catalog(result) => {
                self.loading = false;
                match result {
                    Ok(catalog) => {
                        self.reports
                            .retain(|slug, _| catalog.shows.iter().any(|show| &show.slug == slug));
                        self.paused
                            .retain(|slug| catalog.shows.iter().any(|show| &show.slug == slug));
                        let urls: Vec<_> = catalog
                            .shows
                            .iter()
                            .filter_map(|show| show.image_url.clone())
                            .collect();
                        self.artwork_cache
                            .update(cx, |cache, cx| cache.retain(&urls, window, cx));
                        self.loaded = true;
                        self.catalog = catalog;
                        if !self
                            .catalog
                            .shows
                            .iter()
                            .any(|show| self.selected.as_ref() == Some(&show.slug))
                        {
                            let error = self.error.take();
                            self.select(
                                self.catalog
                                    .shows
                                    .iter()
                                    .find(|show| {
                                        self.team.as_ref().is_none_or(|team| team == &show.team_id)
                                    })
                                    .map(|show| show.slug.clone()),
                                window,
                                cx,
                            );
                            self.error = error;
                        } else {
                            self.load_episodes(cx);
                        }
                        self.start_auto_sync(cx);
                    }
                    Err(error) => {
                        if error.is::<listenbox_sync_engine::api::AuthenticationRequired>() {
                            self.loaded = false;
                            self.catalog = Catalog::default();
                            self.select(None, window, cx);
                        } else {
                            self.error = Some(format!("Could not load podcasts. {error:#}").into());
                        }
                    }
                }
            }
            Message::Login(result) => {
                self.authenticating = false;
                match result {
                    Ok(()) => self.reload(cx),
                    Err(error) => {
                        self.error = Some(format!("Sign-in did not finish. {error:#}").into())
                    }
                }
            }
            Message::ImportPreparation(preparation) => {
                if self.creating.is_some() {
                    self.creating = Some(preparation);
                }
            }
            Message::ImportCreated(show, cancel) => {
                // Creation owns the form. Episode work belongs to this podcast
                // from here onward, so another playlist can be created now.
                self.creating = None;
                self.source
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.jobs.insert(show.slug.clone(), cancel);
                self.team = Some(show.team_id.clone());
                let slug = show.slug.clone();
                self.catalog.shows.push(show);
                self.import_open = false;
                self.select(Some(slug.clone()), window, cx);
                self.reports.insert(
                    slug,
                    SyncReport::Notice("Importing the first episodes…".into()),
                );
            }
            Message::Imported { slug, kind, result } => {
                if slug.is_none() {
                    self.creating = None;
                }
                let stopped = slug.as_ref().is_some_and(|slug| {
                    self.jobs
                        .remove(slug)
                        .is_some_and(|cancel| cancel.is_cancelled())
                });
                match result {
                    Ok((show, report)) => {
                        self.receive(Message::Report(show.slug, report), window, cx);
                    }
                    Err(_) if stopped => {
                        if let Some(slug) = slug {
                            if self.selected.as_ref() == Some(&slug) {
                                self.load_episodes(cx);
                            }
                            self.reports.insert(
                                slug,
                                SyncReport::Notice(
                                    "Paused for this session. Resume to continue syncing.".into(),
                                ),
                            );
                        }
                    }
                    Err(error) => {
                        if let Some(slug) = &slug {
                            self.reports.insert(
                                slug.clone(),
                                SyncReport::Notice(
                                    "Import stopped. Use Sync now to continue.".into(),
                                ),
                            );
                        }
                        self.error = Some(ErrorNotice::import(
                            error,
                            &self.client,
                            self.catalog.import_team.as_deref(),
                            slug.is_some(),
                            &kind,
                        ));
                        // Creation may have committed even when its reply was lost.
                        // A catalog reload exposes that podcast for explicit resumption.
                        self.reload_after_import(cx);
                    }
                }
            }
            Message::Report(slug, report) => {
                if report.stopped {
                    self.catalog.shows.retain(|show| show.slug != slug);
                    self.paused.remove(&slug);
                    self.reports.remove(&slug);
                    if self.selected.as_ref() == Some(&slug) {
                        self.select(
                            self.catalog.shows.first().map(|show| show.slug.clone()),
                            window,
                            cx,
                        );
                    }
                    cx.notify();
                    return;
                }
                if self.selected.as_ref() == Some(&slug) {
                    self.load_episodes(cx);
                }
                self.reports.insert(slug, SyncReport::Summary(report));
            }
            Message::Finished(slug, result) => {
                if result.is_err() && self.selected.as_ref() == Some(&slug) {
                    self.load_episodes(cx);
                }
                let stopped = self
                    .jobs
                    .remove(&slug)
                    .is_some_and(|cancel| cancel.is_cancelled());
                if stopped {
                    self.reports.insert(
                        slug,
                        SyncReport::Notice(
                            "Paused for this session. Resume to continue syncing.".into(),
                        ),
                    );
                } else if let Err(error) = result {
                    if error.is::<listenbox_sync_engine::cookies::SignInRequired>() {
                        self.error = Some(ErrorNotice::youtube_sign_in());
                        self.reports.insert(
                            slug,
                            SyncReport::Notice(
                                "YouTube sign-in required. Open Settings to add fresh cookies."
                                    .into(),
                            ),
                        );
                    } else {
                        self.reports
                            .insert(slug, SyncReport::Notice(format!("Sync failed. {error:#}")));
                    }
                }
            }
        }
        cx.notify();
    }

    fn select(&mut self, slug: Option<String>, _window: &mut Window, cx: &mut Context<Self>) {
        self.selected = slug;
        self.episode_view.reset();
        self.show_issues = false;
        self.settings_open = false;
        self.cookie_input = settings::cookie_input(_window, cx);
        self.import_open = false;
        self.load_episodes(cx);
        self.error = None;
        cx.notify();
    }

    fn show(&self) -> Option<&Show> {
        self.catalog
            .shows
            .iter()
            .find(|show| Some(&show.slug) == self.selected.as_ref())
    }

    fn reload_after_import(&mut self, cx: &mut Context<Self>) {
        let error = self.error.take();
        self.reload(cx);
        self.error = error;
    }

    fn import_collection(&mut self, cx: &mut Context<Self>) {
        if !self.loaded || self.creating.is_some() || self.stopping.is_some() {
            return;
        }
        let source = self.source.read(cx).value().to_string();
        let source = match listenbox_sync_engine::youtube::collection_source(&source) {
            Ok(source) => source,
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
                return;
            }
        };
        self.creating = Some(ImportPreparation::default());
        self.error = None;
        let cancel = self.cancel.child_token();
        let (client, sender, kind) = (
            self.client.clone(),
            self.sender.clone(),
            self.import_kind.clone(),
        );
        self.tasks.spawn_on(
            async move {
                let created_sender = sender.clone();
                let created_cancel = cancel.clone();
                let (created, mut imported) = tokio::sync::oneshot::channel();
                let mut created = Some(created);
                let result = client
                    .import_collection(&source, kind.clone(), cancel, move |event| match event {
                        ImportEvent::Preparing(preparation) => {
                            let _ = created_sender.send(Message::ImportPreparation(preparation));
                        }
                        ImportEvent::Created(show) => {
                            if let Some(created) = created.take() {
                                let _ = created.send(show.slug.clone());
                            }
                            let _ = created_sender
                                .send(Message::ImportCreated(show, created_cancel.clone()));
                        }
                    })
                    .await;
                // The creation callback finishes before this operation returns,
                // so completion carries its own podcast even on a later error.
                let _ = sender.send(Message::Imported {
                    slug: imported.try_recv().ok(),
                    kind,
                    result,
                });
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    fn start_auto_sync(&mut self, cx: &mut Context<Self>) {
        if self.stopping.is_some() || !self.client.has_credentials() {
            return;
        }
        if !self.auto_sync {
            self.auto_sync = true;
            let (client, cancel, sender) = (
                self.client.clone(),
                self.cancel.clone(),
                self.sender.clone(),
            );
            self.tasks.spawn_on(
                async move {
                    while client.next_scan(&cancel).await {
                        if sender.send(Message::SyncDue).is_err() {
                            break;
                        }
                    }
                },
                self.runtime.handle(),
            );
        }
        self.sync_all(true, cx);
    }

    fn sync_all(&mut self, only_new: bool, cx: &mut Context<Self>) {
        if !self.loaded || self.stopping.is_some() {
            return;
        }
        let slugs: Vec<_> = self
            .catalog
            .shows
            .iter()
            .filter(|show| {
                show.has_active_subscription
                    && !self.paused.contains(&show.slug)
                    && (!only_new || !self.reports.contains_key(&show.slug))
            })
            .map(|show| show.slug.clone())
            .collect();
        for slug in slugs {
            self.sync_show(slug, cx);
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if let Some(show) = self.show() {
            let slug = show.slug.clone();
            if self.jobs.contains_key(&slug) || self.stopping.is_some() {
                return;
            }
            self.paused.remove(&slug);
            self.sync_show(slug, cx);
        }
    }

    fn pause(&mut self, cx: &mut Context<Self>) {
        if self.stopping.is_some() {
            return;
        }
        if let Some(slug) = &self.selected {
            self.paused.insert(slug.clone());
            if let Some(cancel) = self.jobs.get(slug) {
                cancel.cancel();
            }
            cx.notify();
        }
    }

    fn sync_show(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.stopping.is_some() {
            return;
        }
        if self.jobs.contains_key(&slug) || self.paused.contains(&slug) {
            return;
        }
        let cancel = self.cancel.child_token();
        self.jobs.insert(slug.clone(), cancel.clone());
        // This queue is display history, not the durable resume journal. Clear
        // the previous pass so checking the source cannot show stale totals.
        let downloads = self.client.downloads();
        downloads.remove_source(&slug);
        self.progress = downloads.snapshot();
        self.reports.insert(
            slug.clone(),
            SyncReport::Notice("Reading YouTube and Listenbox…".into()),
        );
        let (client, sender) = (self.client.clone(), self.sender.clone());
        self.tasks.spawn_on(
            async move {
                let report_slug = slug.clone();
                let report_sender = sender.clone();
                let result = client
                    .sync(&slug, false, cancel, move |report| {
                        let _ = report_sender
                            .send(Message::Report(report_slug.clone(), report.clone()));
                    })
                    .await;
                let _ = sender.send(Message::Finished(slug, result));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let team_name = self
            .team
            .as_ref()
            .and_then(|id| self.catalog.teams.iter().find(|team| &team.id == id))
            .map(|team| team.name.clone())
            .unwrap_or_else(|| "All teams".into());
        let rail = div()
            .flex()
            .flex_col()
            .w(px(tokens::SIDEBAR))
            .flex_shrink_0()
            .bg(t.rail)
            .border_r_1()
            .border_color(t.divider);
        let mut navigation = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .px(px(tokens::NAV_ROW_INSET))
            .py(px(tokens::SIDEBAR_INSET))
            .gap(px(tokens::GAP))
            .child(
                Button::new("team-picker")
                    .ghost()
                    .accessibility_label(format!("Choose team: {team_name}"))
                    .w_full()
                    .h(px(tokens::CONTROL_HEIGHT))
                    .px(px(tokens::NAV_ROW_INSET))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(tokens::TITLE))
                                    .font_weight(FontWeight::BOLD)
                                    .child(team_name),
                            )
                            .child(
                                Icon::new(if self.team_picker {
                                    assets::IconName::ChevronUp
                                } else {
                                    assets::IconName::ChevronDown
                                })
                                .small()
                                .text_color(t.muted),
                            ),
                    )
                    .disabled(!self.loaded)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.team_picker = !view.team_picker;
                        cx.notify();
                    })),
            );
        if self.team_picker {
            let mut teams = div()
                .id("team-options")
                .flex()
                .flex_col()
                .gap_1()
                .max_h(px(180.))
                .overflow_y_scrollbar()
                .child(
                    Button::new("all-teams")
                        .ghost()
                        .w_full()
                        .h(px(tokens::CONTROL_HEIGHT))
                        .px(px(tokens::NAV_ROW_INSET))
                        .accessibility_label("All teams")
                        .child(div().w_full().child("All teams"))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.team = None;
                            view.team_picker = false;
                            cx.notify();
                        })),
                );
            for team in &self.catalog.teams {
                let id = team.id.clone();
                teams = teams.child(
                    Button::new(SharedString::from(format!("team-{}", team.id)))
                        .ghost()
                        .w_full()
                        .h(px(tokens::CONTROL_HEIGHT))
                        .px(px(tokens::NAV_ROW_INSET))
                        .accessibility_label(team.name.clone())
                        .child(div().w_full().truncate().child(team.name.clone()))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.team = Some(id.clone());
                            view.team_picker = false;
                            let slug = view
                                .catalog
                                .shows
                                .iter()
                                .find(|show| show.team_id == id)
                                .map(|show| show.slug.clone());
                            view.select(slug, window, cx);
                        })),
                );
            }
            navigation = navigation.child(teams);
        }
        navigation = navigation.child(
            div().px(px(tokens::NAV_ROW_INSET)).child(
                Button::new("new-import")
                    .primary()
                    .accessibility_label("Import from YouTube")
                    .w_full()
                    .h(px(tokens::CONTROL_HEIGHT))
                    .px(px(tokens::GAP))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Import from YouTube"),
                            )
                            .child(Icon::new(assets::IconName::Plus).small()),
                    )
                    .disabled(!self.loaded || self.creating.is_some() || self.stopping.is_some())
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.settings_open = false;
                        view.cookie_input = settings::cookie_input(window, cx);
                        view.import_open = true;
                        view.error = None;
                        cx.notify();
                    })),
            ),
        );
        let mut shows = div()
            .id("podcasts")
            .flex()
            .flex_col()
            .gap_1()
            .px(px(tokens::NAV_ROW_INSET))
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();
        let mut count = 0;
        for show in self
            .catalog
            .shows
            .iter()
            .filter(|show| self.team.as_ref().is_none_or(|team| team == &show.team_id))
        {
            count += 1;
            let slug = show.slug.clone();
            let selected = !self.import_open && self.selected.as_ref() == Some(&slug);
            let running = self.jobs.get(&slug);
            let progress = self.import_progress(&slug);
            let status = if running.is_some_and(|cancel| cancel.is_cancelled()) {
                "Pausing…".into()
            } else if self.paused.contains(&slug) {
                "Paused".into()
            } else if running.is_some() && progress.total > 0 {
                format!("{} / {} imported", progress.imported, progress.total)
            } else if running.is_some() {
                "Checking YouTube…".into()
            } else if !show.has_active_subscription {
                "Plan required".into()
            } else {
                "Automatic sync".into()
            };
            shows = shows.child(
                Button::new(SharedString::from(format!("show-{slug}")))
                    .ghost()
                    .w_full()
                    .h_auto()
                    .p(px(tokens::NAV_ROW_INSET))
                    .accessibility_label(format!("{}, {status}", show.title))
                    .justify_start()
                    .when(selected, |button| button.bg(t.selected))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .w_full()
                            .child(artwork(show.image_url.as_deref(), 44., t))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap_1()
                                    .child(
                                        div()
                                            .w_full()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(show.title.clone()),
                                    )
                                    .child(
                                        div().text_size(px(12.)).text_color(t.muted).child(status),
                                    )
                                    .when(running.is_some() && progress.total > 0, |column| {
                                        column.child(
                                            Progress::new(SharedString::from(format!(
                                                "sidebar-progress-{slug}"
                                            )))
                                            .xsmall()
                                            .color(t.action)
                                            .value(progress.percent())
                                            .accessibility_label(format!(
                                                "{}: {} of {} episodes imported",
                                                show.title, progress.imported, progress.total
                                            )),
                                        )
                                    }),
                            ),
                    )
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.select(Some(slug.clone()), window, cx)
                    })),
            );
        }
        if count == 0 && self.loaded {
            shows = shows.child(
                div()
                    .px(px(tokens::NAV_ROW_INSET))
                    .py_4()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child("Your imported podcasts will appear here."),
            );
        }
        rail.child(navigation)
            .child(shows)
            .child(
                div()
                    .p(px(tokens::SIDEBAR_INSET))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child("YouTube → Listenbox")
                    .map(|footer| {
                        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                        let footer = footer.child(crate::platform::application_menu_button());
                        footer
                    }),
            )
            .into_any_element()
    }

    fn import_form(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let busy = self.creating.is_some() || self.stopping.is_some();
        let team = self
            .catalog
            .import_team
            .as_ref()
            .and_then(|id| self.catalog.teams.iter().find(|team| &team.id == id))
            .map(|team| team.name.as_str())
            .unwrap_or("your authorized team");
        if let Some(preparation) = &self.creating {
            return div()
                .w_full()
                .min_w_0()
                .max_w(px(620.))
                .py(px(tokens::SPACE))
                .flex()
                .flex_col()
                .gap(px(tokens::SPACE))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(tokens::PAGE_TITLE))
                                .font_weight(FontWeight::BOLD)
                                .child(if self.import_kind == ShowSourceKind::Video {
                                    "Preparing your video podcast"
                                } else {
                                    "Preparing your audio podcast"
                                }),
                        )
                        .child(
                            div()
                                .text_color(t.muted)
                                .child(format!("Creates a new podcast in {team}.")),
                        ),
                )
                .child(self.creation_status(preparation, cx))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .border_t_1()
                        .border_color(t.divider)
                        .pt_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .child("Playlist or channel URL"),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .child(self.source.read(cx).value().to_string()),
                        ),
                )
                .into_any_element();
        }
        let form = div().flex().flex_col().items_start().gap(px(tokens::SPACE)).max_w(px(620.)).py(px(tokens::SPACE))
            .child(div().flex().flex_col().gap_3()
                .child(div().text_size(px(tokens::PAGE_TITLE)).font_weight(FontWeight::BOLD).child("Import from YouTube"))
                .child(div().text_color(t.muted).child("Import a public playlist or channel, then keep new episodes in sync.")))
            .child(div().w_full().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Playlist or channel URL"))
                .child(Input::new(&self.source).id("youtube-url").disabled(busy))
                .child(div().text_size(px(12.)).text_color(t.muted).child("Use a public playlist or channel URL, with or without /videos.")))
            .child(div().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Podcast format"))
                .child(div().flex().gap_2()
                    .child(Button::new("import-audio").label("Audio").icon(assets::IconName::Headphones)
                        .map(|button| if self.import_kind == ShowSourceKind::Audio { button.primary() } else { button.outline() }).disabled(busy)
                        .on_click(cx.listener(|view, _, _, cx| { view.import_kind = ShowSourceKind::Audio; cx.notify(); })))
                    .child(Button::new("import-video").label("Video").icon(assets::IconName::Video)
                        .map(|button| if self.import_kind == ShowSourceKind::Video { button.primary() } else { button.outline() }).disabled(busy)
                        .on_click(cx.listener(|view, _, _, cx| { view.import_kind = ShowSourceKind::Video; cx.notify(); })))))
            .child(div().w_full().border_t_1().border_color(t.divider).pt_4().flex().flex_col().gap_2()
                .child(format!("Creates a new podcast in {team}."))
                .child(div().text_size(px(12.)).text_color(t.muted)
                    .child(if self.import_kind == ShowSourceKind::Video { "A video plan with enough storage is required. Playlists keep their order; channels show newest uploads first. Removed videos leave the podcast on the next complete sync." } else { "A paid podcast plan is required. Playlists keep their order; channels show newest uploads first. Removed videos leave the podcast on the next complete sync." })))
            .child(div().flex().items_center().gap_3()
                .child(Button::new("start-import").primary().label("Create podcast & import")
                    .disabled(busy).on_click(cx.listener(|view, _, _, cx| view.import_collection(cx))))
                .when(self.show().is_some(), |row| row.child(Button::new("cancel-import").ghost().label("Cancel")
                    .on_click(cx.listener(|view, _, _, cx| { view.import_open = false; view.error = None; cx.notify(); })))));
        form.into_any_element()
    }

    fn detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        if !self.loaded && self.loading {
            return div()
                .flex()
                .items_center()
                .gap_3()
                .py(px(tokens::SPACE))
                .child(Spinner::new().small())
                .child(div().text_color(t.muted).child("Loading your podcasts…"))
                .into_any_element();
        }
        if !self.loaded {
            return div().flex().flex_col().items_start().gap(px(tokens::SPACE)).max_w(px(600.)).py(px(tokens::SPACE * 2.))
                .child(div().text_size(px(tokens::PAGE_TITLE)).font_weight(FontWeight::BOLD).child("Your YouTube recordings as a podcast"))
                .child(div().text_color(t.muted).child("Bring a public YouTube playlist or channel to Listenbox, then keep your podcast in sync from this desktop."))
                .child(Button::new("welcome-action").primary().label(if self.authenticating { "Finish in your browser…" } else { "Sign in to Listenbox" })
                    .disabled(self.authenticating || self.stopping.is_some()).on_click(cx.listener(|view, _, _, cx| view.login(cx))))
                .child(div().text_size(px(12.)).text_color(t.muted).child("Uses your Listenbox account and podcast plan.")).into_any_element();
        }
        if self.import_open || self.show().is_none() {
            return self.import_form(window, cx);
        }
        let show = self.show().unwrap();
        let source = show.youtube_linkage().unwrap_or_default().to_owned();
        let source_link = source.clone();
        let format = if show.source_kind == ShowSourceKind::Audio {
            "Audio podcast"
        } else {
            "Video podcast"
        };
        let mut pane = div()
            .flex()
            .flex_col()
            .gap(px(tokens::SPACE))
            .max_w(px(900.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::SPACE))
                    .py_2()
                    .child(artwork(show.image_url.as_deref(), 120., t))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(tokens::PAGE_TITLE))
                                    .font_weight(FontWeight::BOLD)
                                    .child(show.title.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .text_color(t.muted)
                                    .child(format)
                                    .when(
                                        !self.episode_loading && self.episode_error.is_none(),
                                        |row| {
                                            let count = self.episodes.len();
                                            let label = format!(
                                                "{count} {}",
                                                if count == 1 { "episode" } else { "episodes" }
                                            );
                                            row.child("·").child(
                                                div()
                                                    .id("episode-count")
                                                    .role(Role::Label)
                                                    .aria_label(label.clone())
                                                    .child(label)
                                                    .test_support(),
                                            )
                                        },
                                    ),
                            )
                            .when(
                                !self.source_loading && self.source_error.is_none(),
                                |header| {
                                    let label = self
                                        .last_synced
                                        .map(|time| {
                                            format!(
                                                "Last synced {}",
                                                chrono::DateTime::<chrono::Local>::from(time)
                                                    .format("%b %-d, %Y at %H:%M")
                                            )
                                        })
                                        .unwrap_or_else(|| "Not synced on this device yet".into());
                                    header.child(
                                        div()
                                            .id("last-synced")
                                            .role(Role::Label)
                                            .aria_label(label.clone())
                                            .text_size(px(12.))
                                            .text_color(t.muted)
                                            .child(label)
                                            .test_support(),
                                    )
                                },
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        Button::new("open-show")
                                            .ghost()
                                            .small()
                                            .icon(assets::IconName::ExternalLink)
                                            .label("Open in Listenbox")
                                            .on_click(cx.listener(|view, _, _, cx| {
                                                if let Some(url) = view.show().and_then(|show| {
                                                    view.client.show_url(show).ok()
                                                }) {
                                                    cx.open_url(&url);
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new("open-playlist")
                                            .ghost()
                                            .small()
                                            .icon(assets::IconName::ExternalLink)
                                            .label("Open in YouTube")
                                            .accessibility_label(format!(
                                                "Open YouTube source: {source}"
                                            ))
                                            .on_click(move |_, _, cx| cx.open_url(&source_link)),
                                    ),
                            ),
                    ),
            )
            .child(self.import_status(show, cx));
        if !show.has_active_subscription {
            pane = pane.child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .child("Choose a paid audio or video plan to resume syncing.")
                    .when_some(
                        self.client.upgrade_url(&show.team_id).ok(),
                        |notice, url| {
                            notice.child(
                                Button::new("choose-plan")
                                    .ghost()
                                    .icon(assets::IconName::ExternalLink)
                                    .label("Upgrade plan")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        },
                    ),
            );
        }
        pane.into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "hot-reload")]
        {
            dioxus_devtools::subsecond::call(|| self.render_workspace(window, cx))
        }
        #[cfg(not(feature = "hot-reload"))]
        self.render_workspace(window, cx)
    }
}

impl Workspace {
    fn content_header(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        div()
            .flex()
            .flex_col()
            .when_some(
                self.error.clone().filter(|_| !self.settings_open),
                |pane, error| {
                    pane.child(
                        div()
                            .mb_4()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_2()
                            .child(div().text_color(t.danger).child(error.message))
                            .when(error.youtube_sign_in, |notice| {
                                notice.child(
                                    Button::new("youtube-sign-in-settings")
                                        .primary()
                                        .label("Open YouTube settings")
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.open_settings(window, cx)
                                        })),
                                )
                            })
                            .when_some(error.upgrade_url, |notice, url| {
                                notice.child(
                                    Button::new("upgrade-plan")
                                        .ghost()
                                        .icon(assets::IconName::ExternalLink)
                                        .label("Upgrade plan")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                            }),
                    )
                },
            )
            .child(if self.settings_open {
                self.settings(cx)
            } else {
                self.detail(window, cx)
            })
            .into_any_element()
    }

    fn content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let pane = div().id("workspace-content").flex_1().min_h_0();
        if self.loaded && !self.settings_open && !self.import_open && self.show().is_some() {
            pane.overflow_hidden()
                .child(self.episode_content(cx))
                .test_support()
                .into_any_element()
        } else {
            let scroll = window
                .use_keyed_state("workspace-scroll", cx, |_, _| ScrollHandle::default())
                .read(cx)
                .clone();
            pane.relative()
                .overflow_hidden()
                .child(
                    div()
                        .id("workspace-scroll-area")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .p(px(tokens::SPACE))
                        .child(self.content_header(window, cx)),
                )
                .child(Scrollbar::vertical(&scroll).mode(ScrollbarMode::Always))
                .test_support()
                .into_any_element()
        }
    }

    // Keep the hotpatch boundary's return type independent of the element tree.
    fn render_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let quit_notice = if self.stopping.is_some() {
            Some(("Finishing current work…", 1.))
        } else {
            self.quit_notice.map(|notice| {
                let mut opacity = notice.opacity(cx.background_executor().now());
                if opacity > 0. && opacity < 1. {
                    if cx.reduce_motion() {
                        opacity = 0.;
                    } else {
                        window.request_animation_frame();
                    }
                }
                (notice.instruction, opacity)
            })
        };
        div()
            .image_cache(self.artwork_cache.clone())
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|view, _: &crate::platform::OpenSettings, window, cx| {
                    view.open_settings(window, cx)
                }),
            )
            .on_action(
                cx.listener(|view, _: &crate::platform::Reload, window, cx| {
                    if view.loading || view.stopping.is_some() {
                        return;
                    }
                    view.artwork_cache
                        .update(cx, |cache, cx| cache.clear(window, cx));
                    view.reload(cx);
                }),
            )
            .capture_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.platform && event.keystroke.key == "q" {
                    cx.stop_propagation();
                    if event.is_held || view.stopping.is_some() {
                        return;
                    }
                    view.quit_pressed(crate::platform::quit_key_state(), cx);
                }
            }))
            .capture_key_up(cx.listener(|view, event: &KeyUpEvent, _, cx| {
                if event.keystroke.key == "q" {
                    view.quit_released(cx);
                }
            }))
            .on_modifiers_changed(cx.listener(|view, event: &ModifiersChangedEvent, _, cx| {
                if !event.modifiers.platform {
                    view.quit_released(cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .bg(t.background)
            .text_color(t.ink)
            .text_size(px(tokens::BODY))
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(self.content(window, cx)),
            )
            .when_some(quit_notice, |workspace, (instruction, opacity)| {
                workspace.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id("quit-notice")
                                .opacity(opacity)
                                .occlude()
                                .w(px(tokens::QUIT_HUD_WIDTH))
                                .p(px(tokens::SPACE))
                                .rounded(px(tokens::QUIT_HUD_RADIUS))
                                .bg(t.action.opacity(0.96))
                                .text_color(t.action_ink)
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(tokens::GAP))
                                .text_center()
                                .child(
                                    div()
                                        .text_size(px(if self.stopping.is_some() {
                                            tokens::TITLE
                                        } else {
                                            tokens::QUIT_SHORTCUT
                                        }))
                                        .line_height(relative(1.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(if self.stopping.is_some() {
                                            "Saving progress"
                                        } else {
                                            "⌘ Q"
                                        }),
                                )
                                .child(div().child(instruction)),
                        ),
                )
            })
            .into_any_element()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests;
