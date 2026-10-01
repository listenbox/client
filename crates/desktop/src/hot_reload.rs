//! Receive Subsecond patches while GPUI retains the workspace and its live jobs.
use crate::{tokens, workspace::Workspace};
use dioxus_devtools::subsecond;
use gpui_kit::{Context, Entity, Window};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

pub fn connect(view: &Entity<Workspace>, window: &mut Window, cx: &mut gpui_kit::App) {
    let (sender, mut receiver) = unbounded_channel();
    subsecond::register_handler(Arc::new(move || {
        // The patch arrives on the devtools thread; GPUI stays on its UI thread.
        let _ = sender.send(());
    }));
    view.update(cx, |_, cx: &mut Context<Workspace>| {
        cx.spawn_in(window, async move |view, cx| {
            while receiver.recv().await.is_some() {
                if view
                    .update_in(cx, |_, window, cx| {
                        subsecond::call(|| tokens::apply(window, cx));
                        cx.notify();
                        // Invalidate cached subtrees and rebuild their event callbacks.
                        window.refresh();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    });
    dioxus_devtools::connect_subsecond();
}
