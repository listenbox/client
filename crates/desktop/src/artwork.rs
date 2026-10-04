use crate::tokens::Tokens;
use gpui_kit::{component::Icon, *};
use std::{collections::VecDeque, sync::Arc};

// GPUI owns loading, request coalescing, decode and GPU release. This workspace
// adds a bounded working set so browsing a large library cannot retain every cover.
pub struct ArtworkCache {
    images: Entity<RetainAllImageCache>,
    recent: VecDeque<Resource>,
}

impl ArtworkCache {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let images = RetainAllImageCache::new(cx);
        cx.new(|_| Self {
            images,
            recent: VecDeque::new(),
        })
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        self.recent.clear();
        self.images
            .update(cx, |images, cx| images.clear(window, cx));
    }

    pub fn retain(&mut self, urls: &[String], window: &mut Window, cx: &mut App) {
        let resources: Vec<Resource> = urls
            .iter()
            .map(|url| Resource::Uri(url.clone().into()))
            .collect();
        self.recent.retain(|resource| {
            if resources.contains(resource) {
                return true;
            }
            self.images
                .update(cx, |images, cx| images.remove(resource, window, cx));
            false
        });
    }
}

impl ImageCache for ArtworkCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.recent.retain(|item| item != resource);
        self.recent.push_back(resource.clone());
        if self.recent.len() > 128
            && let Some(oldest) = self.recent.pop_front()
        {
            self.images
                .update(cx, |images, cx| images.remove(&oldest, window, cx));
        }
        self.images
            .update(cx, |images, cx| images.load(resource, window, cx))
    }
}

pub fn artwork(url: Option<&str>, side: f32, t: Tokens) -> AnyElement {
    let placeholder = move || {
        div()
            .size_full()
            .rounded(px(if side > 64. { 14. } else { 8. }))
            .flex()
            .items_center()
            .justify_center()
            .bg(t.selected)
            .text_color(t.muted)
            .child(Icon::new(assets::IconName::Headphones).size(px(side * 0.38)))
            .into_any_element()
    };
    div()
        .size(px(side))
        .flex_shrink_0()
        .rounded(px(if side > 64. { 14. } else { 8. }))
        .overflow_hidden()
        .child(match url {
            Some(url) => img(SharedString::from(url.to_owned()))
                .id(SharedString::from(url.to_owned()))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .rounded(px(if side > 64. { 14. } else { 8. }))
                .with_loading(placeholder)
                .with_fallback(placeholder)
                .into_any_element(),
            None => placeholder(),
        })
        .into_any_element()
}
