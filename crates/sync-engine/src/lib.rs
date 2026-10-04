//! Shared Listenbox client operations.
use anyhow::Result;
pub mod api;
pub mod artwork;
mod audio;
pub mod auth;
#[rustfmt::skip]
mod clientconfig;
pub mod config;
pub mod cookies;
pub mod episodes;
pub mod events;
pub mod image_http;
pub mod innertube;
mod media;
#[cfg(test)]
mod native_media_tests;
#[rustfmt::skip]
pub mod publicapi;
pub mod youtube;
pub fn nonblank(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("must not be blank".into());
    }
    Ok(value.into())
}

pub fn slug(value: &str) -> Result<String, String> {
    let value = nonblank(value)?;
    if value.split('-').any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    }) {
        return Err("must use lowercase letters, numbers, and single hyphens".into());
    }
    Ok(value)
}

pub fn language(value: &str) -> Result<String, String> {
    let value = nonblank(value)?;
    let bytes = value.as_bytes();
    if !((bytes.len() == 2 || bytes.len() == 5)
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && (bytes.len() == 2
            || (bytes[2] == b'-' && bytes[3..].iter().all(u8::is_ascii_uppercase))))
    {
        return Err("must use a language code such as en or en-US".into());
    }
    Ok(value)
}

pub fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 16
            && suffix
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

pub fn episode_id(value: &str) -> Result<String, String> {
    if !valid_id(value, "ep_") {
        return Err(format!("invalid episode ID {value:?}"));
    }
    Ok(value.into())
}

mod database;
mod download;
pub mod downloads;
mod errors;
pub mod sync;

pub fn redact(message: &str) -> String {
    message
        .split_whitespace()
        .map(|word| {
            if word.starts_with("https://") && word.contains("?") {
                "[URL]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub mod client;
