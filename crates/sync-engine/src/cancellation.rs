//! Cancellable admission for blocking profile operations. Once admitted, the
//! caller owns its commit and must finish it before releasing the guard.
use anyhow::{Result, ensure};
use std::{
    fs::{File, TryLockError},
    future::Future,
    pin::pin,
    sync::Arc,
    task::{Context, Wake, Waker},
    thread::{self, Thread},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct LockWaiter(Thread);

impl Wake for LockWaiter {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

#[tracing::instrument(name = "profile.lock", skip_all, fields(lock))]
pub(crate) fn admission<T>(
    cancel: &CancellationToken,
    lock: &'static str,
    mut acquire: impl FnMut() -> Result<Option<T>>,
) -> Result<T> {
    let mut cancelled = pin!(cancel.cancelled());
    let waker = Waker::from(Arc::new(LockWaiter(thread::current())));
    let mut context = Context::from_waker(&waker);
    tracing::info!(lock, "acquiring profile lock");
    loop {
        ensure!(
            cancelled.as_mut().poll(&mut context).is_pending(),
            "operation cancelled while acquiring {lock}"
        );
        if let Some(guard) = acquire()? {
            tracing::info!("profile lock acquired");
            return Ok(guard);
        }
        // OS file locks cannot notify Rust when another process releases them.
        // Poll availability, but cancellation unparks this thread immediately;
        // the unpark permit also closes the signal-before-park race.
        thread::park_timeout(Duration::from_millis(10));
    }
}

pub(crate) fn file(file: &File, cancel: &CancellationToken, lock: &'static str) -> Result<()> {
    admission(cancel, lock, || match file.try_lock() {
        Ok(()) => Ok(Some(())),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    })
}
