//! One bounded error policy for source resolution, transfer and sync outcomes.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    /// DNS, connections, timeouts and interrupted HTTP bodies.
    Retryable,
    /// The source explicitly disallows playback; check again on the next sync.
    Skip,
    /// Explicit cancellation preserves resumable work in the queue.
    Cancelled,
    /// Authentication, API rejection, invalid contracts or local processing failures.
    Failed,
}

const CATEGORY_INFO: &str = "listenbox_error_category";

#[derive(Debug)]
pub(crate) struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operation interrupted; resumable source upload state preserved")
    }
}
impl std::error::Error for Interrupted {}

pub(crate) fn classify(error: &anyhow::Error) -> Category {
    for cause in error.chain() {
        if cause.is::<Interrupted>() {
            return Category::Cancelled;
        }
        if cause.is::<crate::download::MediaUnavailable>() {
            return Category::Skip;
        }
        if let Some(error) = cause.downcast_ref::<youtubei::Error>()
            && let Some(category) = error.info.as_ref().and_then(|info| info.get(CATEGORY_INFO))
            && let Ok(category) = serde_json::from_value(category.clone())
        {
            return category;
        }
        // reqwest-middleware's transparent wrapper forwards source() past the
        // reqwest error itself. Inspect its variant as well as bare body errors.
        let transport = cause.downcast_ref::<reqwest::Error>().or_else(|| {
            match cause.downcast_ref::<reqwest_middleware::Error>() {
                Some(reqwest_middleware::Error::Reqwest(error)) => Some(error),
                _ => None,
            }
        });
        if let Some(error) = transport
            && (error.is_connect()
                || error.is_timeout()
                || error.is_request()
                || error.is_body()
                || error.is_decode())
        {
            return Category::Retryable;
        }
    }
    Category::Failed
}

/// Preserve the category through the embedded JS exception, without matching
/// error messages or letting concurrent videos share mutable failure state.
pub(crate) fn youtube_error(error: anyhow::Error) -> youtubei::Error {
    let category = classify(&error);
    let mut error = youtubei::Error::new(crate::redact(&format!("{error:#}")));
    error.info = Some(serde_json::json!({ CATEGORY_INFO: category }));
    error
}
