//! Live sync driver invoked by apps/api/e2e with a test-owned API and YouTube proxy.
use listenbox_sync_engine::{
    api::Api,
    client::Client,
    config::Config,
    downloads::{DownloadManager, Phase, RangePhase},
    sync::Engine,
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

// Cross Tokio's millisecond-rounded deadline so each advance wakes the sampler.
const SAMPLE_WINDOW: Duration = Duration::from_millis(1001);

async fn hold_manual_clock() -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (hold_clock, receive) = std::sync::mpsc::channel::<()>();
    let (started, ready) = tokio::sync::oneshot::channel();
    let clock_guard = tokio::task::spawn_blocking(move || {
        started.send(()).unwrap();
        let _ = receive.recv();
    });
    ready.await.unwrap();
    tokio::time::pause();
    (hold_clock, clock_guard)
}

async fn downloading_gate(
    manager: &DownloadManager,
    changes: &mut tokio::sync::watch::Receiver<()>,
    count: usize,
) {
    loop {
        let snapshot = manager.snapshot();
        let downloading: Vec<_> = snapshot
            .items
            .iter()
            .filter(|item| item.phase == Phase::Downloading)
            .collect();
        if downloading.len() == count
            && downloading.iter().all(|item| {
                item.ranges
                    .iter()
                    .filter(|range| range.start > 0)
                    .all(|range| range.phase == RangePhase::Complete)
            })
        {
            return;
        }
        changes.changed().await.unwrap();
    }
}

async fn receive_window(
    api: &Api,
    mock: &str,
    manager: &DownloadManager,
    changes: &mut tokio::sync::watch::Receiver<()>,
    credit: u64,
) {
    let before: Vec<_> = manager
        .snapshot()
        .items
        .into_iter()
        .filter(|item| item.phase == Phase::Downloading)
        .map(|item| {
            let received = item.received();
            (item.source_url, received)
        })
        .collect();
    assert!(
        !before.is_empty(),
        "A throughput window needs admitted downloads"
    );
    for (source, _) in &before {
        api.http.post(format!("{mock}/__mock__/media-window"))
            .json(&serde_json::json!({ "context": source.strip_prefix("https://www.youtube.com/watch?v=").unwrap(), "bytes": credit }))
            .send().await.unwrap().error_for_status().unwrap();
    }
    loop {
        let snapshot = manager.snapshot();
        if before.iter().all(|(source, received)| {
            snapshot
                .items
                .iter()
                .any(|item| item.source_url == *source && item.received() >= received + credit)
        }) {
            break;
        }
        changes.changed().await.unwrap();
    }
    tokio::time::advance(SAMPLE_WINDOW).await;
    tokio::task::yield_now().await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the parent API's finite media windows and real sync fixture"]
async fn live_download_adaptation() {
    let config = Config::load(None).unwrap();
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.directory.join("test-pipeline.json")).unwrap(),
    )
    .unwrap();
    let slug = fixture["slug"].as_str().unwrap();
    let mock = fixture["mock"].as_str().unwrap();
    let prelude = fixture["prelude"].as_str().unwrap();
    let cancel = CancellationToken::new();
    let api = Api::new(config, cancel.clone()).unwrap();
    let engine = Engine::default();
    let mut changes = engine.downloads.changes();
    let (hold_clock, clock_guard) = hold_manual_clock().await;
    let upload_work = engine.once(&api, prelude);
    let work = async {
        let mut admission = engine.downloads.changes();
        while engine
            .downloads
            .snapshot()
            .items
            .iter()
            .filter(|item| item.phase == Phase::Uploading)
            .count()
            != 2
        {
            let snapshot = engine.downloads.snapshot();
            assert!(
                !snapshot.items.iter().any(|item| matches!(
                    item.phase,
                    Phase::Skipped | Phase::Failed | Phase::Complete
                )),
                "Upload precondition ended before the storage gate: {:?}",
                snapshot.items
            );
            admission.changed().await.unwrap();
        }
        engine.once(&api, slug).await
    };
    let monitor = async {
        while engine
            .downloads
            .snapshot()
            .items
            .iter()
            .filter(|item| item.phase == Phase::Uploading)
            .count()
            != 2
        {
            changes.changed().await.unwrap();
        }
        downloading_gate(&engine.downloads, &mut changes, 2).await;
        tokio::time::advance(SAMPLE_WINDOW).await;
        tokio::task::yield_now().await;
        receive_window(&api, mock, &engine.downloads, &mut changes, 16 << 10).await;
        downloading_gate(&engine.downloads, &mut changes, 3).await;
        // Exclude the window in which the third receiver joined.
        tokio::time::advance(SAMPLE_WINDOW).await;
        tokio::task::yield_now().await;
        receive_window(&api, mock, &engine.downloads, &mut changes, 16 << 10).await;
        assert_eq!(
            engine.downloads.snapshot().download_slots,
            3,
            "Useful throughput growth was rolled back"
        );
        receive_window(&api, mock, &engine.downloads, &mut changes, 16 << 10).await;
        downloading_gate(&engine.downloads, &mut changes, 4).await;
        tokio::time::advance(SAMPLE_WINDOW).await;
        tokio::task::yield_now().await;
        // Four receivers get the same aggregate bytes as the previous three.
        receive_window(&api, mock, &engine.downloads, &mut changes, 12 << 10).await;
        assert_eq!(
            engine.downloads.snapshot().download_slots,
            3,
            "A throughput plateau kept an extra connection"
        );
        for _ in 0..10 {
            tokio::time::advance(SAMPLE_WINDOW).await;
            tokio::task::yield_now().await;
        }
        while engine.downloads.snapshot().download_slots > 1 {
            changes.changed().await.unwrap();
        }
        let drained = engine.downloads.snapshot();
        assert_eq!(
            drained
                .items
                .iter()
                .filter(|item| item.phase == Phase::Downloading)
                .count(),
            4,
            "Backoff cancelled existing downloads"
        );
        assert_eq!(
            drained
                .items
                .iter()
                .filter(|item| item.phase == Phase::Queued)
                .count(),
            4
        );
        assert_eq!(
            drained.upload_slots, 2,
            "Download congestion reduced the upload budget"
        );
        assert_eq!(
            drained
                .items
                .iter()
                .filter(|item| item.phase == Phase::Uploading)
                .count(),
            2,
            "Download probing displaced existing uploads"
        );
        cancel.cancel();
    };
    let (upload_result, result, ()) = tokio::join!(upload_work, work, monitor);
    tokio::time::resume();
    drop(hold_clock);
    clock_guard.await.unwrap();
    assert!(result.is_err());
    assert!(upload_result.is_err());
    assert!(
        engine
            .downloads
            .snapshot()
            .items
            .iter()
            .all(|item| item.phase == Phase::Queued && item.attempt <= 1)
    );
}

/// Real network I/O continues while only the scheduler's production clock is
/// advanced. A live blocking task inhibits Tokio's automatic clock advancement.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the parent API's gated storage and real sync fixture"]
async fn live_pipeline_stall() {
    let config = Config::load(None).unwrap();
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.directory.join("test-pipeline.json")).unwrap(),
    )
    .unwrap();
    let slug = fixture["slug"].as_str().unwrap();
    let control = fixture["control"].as_str().unwrap();
    let api = Api::new(config, CancellationToken::new()).unwrap();
    let engine = Engine::default();
    let mut changes = engine.downloads.changes();
    let (hold_clock, clock_guard) = hold_manual_clock().await;
    let work = engine.once(&api, slug);
    let monitor = async {
        loop {
            let snapshot = engine.downloads.snapshot();
            if snapshot
                .items
                .iter()
                .filter(|item| item.phase == Phase::Uploading)
                .count()
                == 2
                && snapshot
                    .items
                    .iter()
                    .filter(|item| item.phase == Phase::WaitingToUpload)
                    .count()
                    == 1
            {
                break;
            }
            changes.changed().await.unwrap();
        }
        // The storage gate establishes real admitted requests, not just a UI phase.
        api.http
            .get(format!("{control}/ready"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let initial = engine.downloads.snapshot().upload_slots;
        for _ in 0..12 {
            tokio::time::advance(SAMPLE_WINDOW).await;
            tokio::task::yield_now().await;
        }
        while engine.downloads.snapshot().upload_slots >= initial {
            changes.changed().await.unwrap();
        }
        let drained = engine.downloads.snapshot();
        assert_eq!(
            drained
                .items
                .iter()
                .filter(|item| item.phase == Phase::Uploading)
                .count(),
            2,
            "Backoff cancelled an admitted upload"
        );
        assert_eq!(
            drained
                .items
                .iter()
                .filter(|item| item.phase == Phase::WaitingToUpload)
                .count(),
            1
        );
        assert_eq!(
            drained.download_slots, 2,
            "An upload stall reduced download capacity"
        );
        api.http
            .get(format!("{control}/release/0"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        loop {
            let snapshot = engine.downloads.snapshot();
            if snapshot
                .items
                .iter()
                .any(|item| item.phase == Phase::Complete)
            {
                assert_eq!(
                    snapshot
                        .items
                        .iter()
                        .filter(|item| item.phase == Phase::Uploading)
                        .count(),
                    1
                );
                assert_eq!(
                    snapshot
                        .items
                        .iter()
                        .filter(|item| item.phase == Phase::WaitingToUpload)
                        .count(),
                    1,
                    "A new upload was admitted before the reduced budget drained"
                );
                break;
            }
            changes.changed().await.unwrap();
        }
        api.http
            .get(format!("{control}/release/1"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    };
    let (result, ()) = tokio::join!(work, monitor);
    tokio::time::resume();
    drop(hold_clock);
    clock_guard.await.unwrap();
    let report = result.unwrap();
    assert_eq!(report.added, 3);
    assert_eq!(report.skipped, 0);
    assert!(
        engine
            .downloads
            .snapshot()
            .items
            .iter()
            .all(|item| item.phase == Phase::Complete && item.attempt == 1)
    );
}

struct DnsFailure {
    address: SocketAddr,
    fail_import: bool,
    calls: AtomicUsize,
}

impl Resolve for DnsFailure {
    fn resolve(&self, name: Name) -> Resolving {
        assert_eq!(name.as_str(), "sync.test.invalid");
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let failure = self.fail_import && call == 1;
        let address = self.address;
        Box::pin(async move {
            if failure {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "failed to lookup address information: nodename nor servname provided, or not known",
                )
                .into());
            }
            Ok(Box::new(std::iter::once(address)) as Addrs)
        })
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the parent API's integrated sync fixture"]
async fn live_network_recovery() {
    let config = Config::load(None).unwrap();
    let fixture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.directory.join("test-network.json")).unwrap())
            .unwrap();
    let slug = fixture["slug"].as_str().unwrap();
    let mode = fixture["mode"].as_str().unwrap();
    let fail_import = match mode {
        "dns" => true,
        "completion" | "resume" | "player" | "published" | "server" | "api" => false,
        _ => panic!("Unknown network scenario: {mode}"),
    };
    let cancel = CancellationToken::new();
    let mut api = Api::new(config, cancel.clone()).unwrap();
    let mut origin = url::Url::parse(&api.config.api_origin).unwrap();
    let resolver = Arc::new(DnsFailure {
        address: format!("{}:{}", origin.host_str().unwrap(), origin.port().unwrap())
            .parse()
            .unwrap(),
        fail_import,
        calls: AtomicUsize::new(0),
    });
    if mode != "api" {
        origin.set_host(Some("sync.test.invalid")).unwrap();
    }
    let test_config = api.config.directory.join("test-network-config.yaml");
    std::fs::write(
        &test_config,
        format!(
            "api_origin: {}\ndashboard_origin: {}\n",
            origin, api.config.dashboard_origin
        ),
    )
    .unwrap();
    api.config = Config::load_in(Some(&test_config), api.config.directory.clone()).unwrap();
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::https(std::env::var("HTTPS_PROXY").unwrap()).unwrap())
        .pool_max_idle_per_host(0)
        .dns_resolver(resolver.clone());
    for certificate in reqwest::Certificate::from_pem_bundle(
        &std::fs::read(std::env::var_os("SSL_CERT_FILE").unwrap()).unwrap(),
    )
    .unwrap()
    {
        builder = builder.add_root_certificate(certificate);
    }
    api.http = reqwest_middleware::ClientBuilder::new(builder.build().unwrap()).build();
    let engine = Engine::default();
    let mut changes = engine.downloads.changes();
    let work = engine.once(&api, slug);
    tokio::pin!(work);
    let mut stopped_during_retry = false;
    let result = loop {
        tokio::select! {
            result = &mut work => break result,
            changed = changes.changed() => {
                changed.unwrap();
                let snapshot = engine.downloads.snapshot();
                for item in &snapshot.items {
                    if item.phase != Phase::Retrying || stopped_during_retry {
                        continue;
                    }
                    assert_eq!(item.attempt, 1, "A second attempt started before Stop");
                    assert!(item.error.is_some(), "Retry lost its transport error");
                    if mode == "dns" {
                        assert!(item.error.as_ref().unwrap().contains("dns error"));
                    }
                    if mode == "server" {
                        assert!(item.error.as_ref().unwrap().contains("HTTP 5"));
                    }
                    // Prove admission to retry policy, then Stop before another
                    // attempt. CI never exercises automatic retries or backoffs.
                    stopped_during_retry = true;
                    cancel.cancel();
                }
            }
        }
    };
    let items = engine.downloads.snapshot().items;
    let client = Client::desktop(api.config.clone()).unwrap();
    let state = client.sync_state(slug.to_owned()).await.unwrap();
    assert_eq!(
        state.last_synced.is_some(),
        result.is_ok(),
        "interrupted work must not record successful sync completion"
    );
    let saved = state.items;
    if mode == "published" {
        let report = result.unwrap();
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.added, 0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].phase, Phase::Complete);
        assert_eq!(
            items[0].attempt, 0,
            "Published media started another transfer"
        );
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].phase, Phase::Complete);
        return;
    }
    assert_eq!(items.len(), 1);
    let item = &items[0];
    let saved = saved
        .iter()
        .find(|saved| saved.source_url == item.source_url)
        .unwrap();
    match mode {
        "resume" => {
            let report = result.unwrap();
            assert_eq!(report.added, 1);
            assert_eq!(report.skipped, 0);
            assert!(!stopped_during_retry);
            assert_eq!(item.phase, Phase::Complete);
            assert_eq!(item.attempt, 1);
            assert!(item.error.is_none());
            assert_eq!(saved.phase, Phase::Complete);
        }
        "dns" | "player" | "completion" | "server" | "api" => {
            assert!(result.is_err());
            assert!(stopped_during_retry, "Stop never reached the retry state");
            assert_eq!(item.attempt, 1);
            if mode == "dns" {
                assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
            }
            assert_eq!(item.phase, Phase::Queued);
            assert_eq!(saved.phase, Phase::Queued);
        }
        _ => unreachable!(),
    }
}
