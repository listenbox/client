use std::{rc::Rc, thread::ThreadId, time::Duration};
use tokio::sync::{mpsc, oneshot};
use youtubei::Worker;

struct Request {
    text: String,
    release: oneshot::Receiver<()>,
    reply: oneshot::Sender<String>,
}

static_assertions::assert_impl_all!(Worker<Request>: Send, Sync, Clone);

struct State {
    owner: Rc<ThreadId>,
    barrier: tokio::sync::Barrier,
    admitted: mpsc::UnboundedSender<String>,
    dropped: Option<oneshot::Sender<ThreadId>>,
}

static_assertions::assert_not_impl_any!(State: Send, Sync);

impl Drop for State {
    fn drop(&mut self) {
        let _ = self
            .dropped
            .take()
            .unwrap()
            .send(std::thread::current().id());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shared_worker_overlaps_calls_from_other_threads_and_drains_on_drop() {
    let (initialized, owner) = oneshot::channel();
    let (admitted, mut admissions) = mpsc::unbounded_channel();
    let (dropped, mut drop_thread) = oneshot::channel();
    let worker = Worker::new(
        move || async move {
            let owner = std::thread::current().id();
            initialized.send(owner).unwrap();
            State {
                owner: Rc::new(owner),
                barrier: tokio::sync::Barrier::new(2),
                admitted,
                dropped: Some(dropped),
            }
        },
        |state, request: Request| {
            Box::pin(async move {
                assert_eq!(std::thread::current().id(), *state.owner);
                state.barrier.wait().await;
                state.admitted.send(request.text.clone()).unwrap();
                request.release.await.unwrap();
                let _ = request.reply.send(request.text);
            })
        },
    )
    .unwrap();
    let mut callers = Vec::new();
    let mut releases = Vec::new();
    let texts = ["First caller", "Second caller"];
    for text in texts {
        let worker = worker.clone();
        let (release, released) = oneshot::channel();
        releases.push(release);
        callers.push(std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let (reply, response) = oneshot::channel();
            runtime.block_on(async move {
                worker
                    .send(Request {
                        text: text.into(),
                        release: released,
                        reply,
                    })
                    .await
                    .unwrap();
                drop(worker);
            });
            response
        }));
    }
    drop(worker);
    // Joining proves every sender has been dropped before handlers may finish.
    let responses = callers
        .into_iter()
        .map(|caller| caller.join().unwrap())
        .collect::<Vec<_>>();
    let mut accepted = Vec::new();
    for _ in texts {
        accepted.push(
            tokio::time::timeout(Duration::from_secs(2), admissions.recv())
                .await
                .expect("both handlers must overlap after all senders are dropped")
                .expect("worker must retain accepted handlers until they finish"),
        );
    }
    accepted.sort();
    assert_eq!(accepted, texts);
    assert_eq!(
        drop_thread.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    );
    for release in releases {
        release.send(()).unwrap();
    }
    for (response, text) in responses.into_iter().zip(texts) {
        let result = tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("accepted requests must finish after all senders are dropped")
            .unwrap();
        assert_eq!(result, text);
    }
    let actual = tokio::time::timeout(Duration::from_secs(2), drop_thread)
        .await
        .expect("worker state must be destroyed after accepted requests finish")
        .unwrap();
    assert_eq!(actual, owner.await.unwrap());
    assert_ne!(actual, std::thread::current().id());
}
