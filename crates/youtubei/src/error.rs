use rquickjs::{Coerced, Ctx, FromJs};

pub type Result<T> = std::result::Result<T, Error>;

/// An owned error; no JavaScript value escapes a locked context on failure.
#[derive(Debug)]
pub struct Error {
    pub message: String,
    pub stack: Option<String>,
    /// Structured upstream exception details, such as a player status.
    pub info: Option<serde_json::Value>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self::message(message)
    }
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stack: None,
            info: None,
        }
    }

    pub(crate) fn caught(ctx: &Ctx<'_>, error: rquickjs::Error) -> Self {
        if !error.is_exception() {
            return Self::message(error.to_string());
        }
        let value = ctx.catch();
        let stack = value.as_exception().and_then(|e| e.stack());
        let info = (|| -> rquickjs::Result<Option<serde_json::Value>> {
            let Some(object) = value.as_object() else {
                return Ok(None);
            };
            let info: rquickjs::Value = object.get("info")?;
            let Some(json) = ctx.json_stringify(info)? else {
                return Ok(None);
            };
            Ok(serde_json::from_str(&json.to_string()?).ok())
        })();
        let info = info.unwrap_or_else(|_| {
            // A non-serializable detail must not replace the original error or
            // leave a second exception pending in the reusable engine.
            ctx.catch();
            None
        });
        let message = Coerced::<String>::from_js(ctx, value)
            .map(|s| s.0)
            .unwrap_or_else(|_| "JavaScript exception".into());
        Self {
            message,
            stack,
            info,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<rquickjs::Error> for Error {
    fn from(value: rquickjs::Error) -> Self {
        Self::message(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::message(value.to_string())
    }
}
