//! Live sync driver invoked by apps/api/e2e with a test-owned API and YouTube proxy.
use listenbox_sync_engine::{
    api::Api, client::Client, config::Config, downloads::Phase, sync::Engine,
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

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
        "completion" | "resume" | "player" | "published" => false,
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
    origin.set_host(Some("sync.test.invalid")).unwrap();
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
    let saved = client.sync_items(slug.to_owned()).await.unwrap();
    if mode == "published" {
        let report = result.unwrap();
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.added, 0);
        assert!(items.is_empty());
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
        "dns" | "player" | "completion" => {
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
