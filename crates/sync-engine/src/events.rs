use crate::api::Api;
use crate::publicapi as p;
use anyhow::{Result, bail, ensure};
use reqwest::Response;
use serde_json::Value;

pub struct Events {
    response: Response,
    buffer: Vec<u8>,
    data: String,
}

impl Events {
    pub async fn open<R: p::Response>(api: &Api, response: R) -> Result<Self> {
        let response = match response.into_stream() {
            Ok(response) => response,
            Err(response) => return Err(api.response_error(response).await),
        };
        ensure!(
            response
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("text/event-stream")),
            "expected an event stream"
        );
        Ok(Self {
            response,
            buffer: Vec::new(),
            data: String::new(),
        })
    }

    pub async fn next(&mut self, api: &Api) -> Result<Value> {
        loop {
            while let Some(end) = self.buffer.iter().position(|c| *c == b'\n') {
                let line = self.buffer.drain(..=end).collect::<Vec<_>>();
                let line = std::str::from_utf8(&line)?.trim_end_matches(['\r', '\n']);
                if line.is_empty() && !self.data.is_empty() {
                    let data = std::mem::take(&mut self.data);
                    return Ok(serde_json::from_str(&data)?);
                }
                if let Some(value) = line.strip_prefix("data:") {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
                }
                ensure!(self.data.len() <= 1 << 20, "event exceeds size limit");
            }
            let chunk = api.wait(async { Ok(self.response.chunk().await?) }).await?;
            match chunk {
                Some(chunk) => {
                    ensure!(
                        self.buffer.len() + chunk.len() <= 1 << 20,
                        "event line exceeds size limit"
                    );
                    self.buffer.extend_from_slice(&chunk);
                }
                None => bail!("event stream ended before a terminal event"),
            }
        }
    }
}

#[derive(Default)]
pub struct Progress(Option<i64>);

impl Progress {
    pub fn update(&mut self, percent: i64) -> Result<()> {
        ensure!(
            (0..=100).contains(&percent),
            "malformed progress percent {percent}"
        );
        if self.0.is_none_or(|previous| percent > previous) {
            eprintln!("Import progress: {percent}%");
            self.0 = Some(percent);
        }
        Ok(())
    }
}
