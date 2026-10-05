//! Production diagnostics remain usable while the UI or a worker is stuck.
use anyhow::{Context, Result};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::OnceLock,
};
use tokio_util::task::TaskTracker;
use tracing_appender::{
    non_blocking::{ErrorCounter, WorkerGuard},
    rolling::{Builder, Rotation},
};
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan};

static DROPPED: OnceLock<ErrorCounter> = OnceLock::new();

pub fn open(profile: &Path) -> Result<WorkerGuard> {
    let directory = profile.join("diagnostics");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let writer = Builder::new()
        .rotation(Rotation::HOURLY)
        .filename_prefix("desktop")
        .filename_suffix("jsonl")
        .max_log_files(48)
        .build(directory)?;
    // File I/O never runs on the UI/worker thread. The queue is bounded and
    // drops diagnostics under overload rather than blocking application work.
    let (writer, guard) = tracing_appender::non_blocking(writer);
    let _ = DROPPED.set(writer.error_counter());
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_thread_names(true)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("warn,listenbox_desktop=info,listenbox_sync_engine=info")
        }))
        .with_writer(writer)
        .try_init()
        .map_err(|error| anyhow::anyhow!(error))?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        tracing::error!(error = %listenbox_sync_engine::redact(&panic.to_string()), "desktop panic");
        previous(panic);
    }));
    tracing::info!(pid = std::process::id(), version = env!("CARGO_PKG_VERSION"),
        commit = env!("LISTENBOX_SOURCE_COMMIT"), profile = %profile.display(), "desktop started");
    Ok(guard)
}

pub async fn drain(tasks: &TaskTracker, reason: &str) {
    let started = std::time::Instant::now();
    tracing::info!(reason, pending = tasks.len(), "draining desktop operations");
    let waiting = tasks.wait();
    tokio::pin!(waiting);
    loop {
        tokio::select! {
            biased;
            _ = &mut waiting => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                tracing::warn!(reason, pending = tasks.len(), elapsed_ms = started.elapsed().as_millis() as u64,
                    dropped_log_lines = DROPPED.get().map_or(0, ErrorCounter::dropped_lines),
                    "desktop is still draining; unmatched operation spans identify pending work");
            }
        }
    }
    tracing::info!(
        reason,
        elapsed_ms = started.elapsed().as_millis() as u64,
        dropped_log_lines = DROPPED.get().map_or(0, ErrorCounter::dropped_lines),
        "desktop operations drained"
    );
}

pub fn report(profile: &Path) -> Result<()> {
    let directory = profile.join("diagnostics");
    println!("Diagnostics: {}", directory.display());
    if !directory.exists() {
        return Ok(());
    }
    let mut logs = std::fs::read_dir(directory)?
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    logs.retain(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("desktop."))
            && path.extension().is_some_and(|ext| ext == "jsonl")
    });
    logs.sort();
    for path in logs.iter().rev().take(2).rev() {
        println!("Log: {}", path.display());
        let mut file = std::fs::File::open(path).context("open desktop diagnostics")?;
        let offset = file.metadata()?.len().saturating_sub(256 << 10);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(256 << 10).read_to_end(&mut bytes)?;
        let tail = String::from_utf8_lossy(&bytes);
        let lines = tail
            .lines()
            .skip(usize::from(offset > 0))
            .collect::<Vec<_>>();
        for line in &lines[lines.len().saturating_sub(100)..] {
            println!("{line}");
        }
    }
    Ok(())
}
