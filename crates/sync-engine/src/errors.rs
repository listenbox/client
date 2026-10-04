//! Episode error policy. The sync token owns cancellation independently.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    /// HTTP server failures, DNS, connections, timeouts and interrupted bodies.
    Retryable,
    /// Skip this episode for this run, retaining the reason for the next sync.
    Skip,
}

const CATEGORY_INFO: &str = "listenbox_error_category";

pub(crate) fn classify(error: &anyhow::Error) -> Category {
    if error
        .downcast_ref::<crate::api::HttpFailure>()
        .is_some_and(|error| (500..600).contains(&error.status))
    {
        return Category::Retryable;
    }
    for cause in error.chain() {
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
                || error.is_decode()
                || error
                    .status()
                    .is_some_and(|status| status.is_server_error()))
        {
            return Category::Retryable;
        }
    }
    Category::Skip
}

/// Preserve the category through the embedded JS exception, without matching
/// error messages or letting concurrent videos share mutable failure state.
pub(crate) fn youtube_error(error: anyhow::Error) -> youtubei::Error {
    let category = classify(&error);
    let mut error = youtubei::Error::new(crate::redact(&format!("{error:#}")));
    error.info = Some(Box::new(serde_json::json!({ CATEGORY_INFO: category })));
    error
}
