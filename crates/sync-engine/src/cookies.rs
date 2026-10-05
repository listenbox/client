//! Private YouTube sessions shared across CLI/desktop processes.
//!
//! Readers open immutable snapshots. Writers hold a separate OS lock through
//! read/compare/merge and fsync + atomic replacement. Each import/removal starts
//! a new generation; each cookie has its own revision (including tombstones).
//! Thus late responses cannot roll back rotations, resurrect deletions, or undo
//! an explicit replacement. A crash exposes either complete file, never a prefix.
use crate::config::write_private_json;
use anyhow::{Context, Result, bail, ensure};
use cookie_store::{Cookie, CookieDomain};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const GUIDE_URL: &str =
    "https://listenbox.app/guides/import-youtube-as-a-podcast/#when-youtube-asks-you-to-sign-in";
const MAX_BYTES: usize = 1 << 20;
const MAX_COOKIES: usize = 1024;

#[derive(Debug)]
pub struct SignInRequired;
impl std::fmt::Display for SignInRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YouTube needs a signed-in session. Open Settings → YouTube to paste fresh cookies, or run listenbox youtube-cookies import <cookies.txt>, then sync again.")
    }
}
impl std::error::Error for SignInRequired {}

// No Debug implementations: snapshots contain credentials.
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    cookie: Cookie<'static>,
    revision: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    generation: String,
    pub(crate) enabled: bool,
    entries: Vec<Entry>,
}
impl Snapshot {
    pub(crate) fn header(&self, url: &Url) -> String {
        if !self.enabled || url.scheme() != "https" || !url.host_str().is_some_and(youtube_domain) {
            return String::new();
        }
        let mut cookies: Vec<_> = self
            .entries
            .iter()
            .map(|entry| &entry.cookie)
            .filter(|cookie| !cookie.is_expired() && cookie.matches(url))
            .collect();
        cookies.sort_by(|a, b| {
            b.path
                .as_ref()
                .len()
                .cmp(&a.path.as_ref().len())
                .then(a.name().cmp(b.name()))
        });
        cookies
            .into_iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; ")
    }
    fn revision(&self, cookie: &Cookie<'_>) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| same_key(&entry.cookie, cookie))
            .map(|entry| entry.revision.as_str())
    }
}

#[derive(Clone)]
pub struct CookieJar {
    directory: PathBuf,
}
impl CookieJar {
    pub fn new(profile: &Path) -> Self {
        Self {
            directory: profile.join("youtube-cookies"),
        }
    }
    fn path(&self) -> PathBuf {
        self.directory.join("jar.json")
    }
    fn write_lock(&self, cancel: &CancellationToken) -> Result<File> {
        std::fs::create_dir_all(&self.directory)
            .context("Create private YouTube cookie directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.directory.join("write.lock"))
            .context("Open YouTube cookie lock")?;
        crate::cancellation::file(&file, cancel, "YouTube cookies")
            .context("Lock YouTube cookie updates")?;
        Ok(file)
    }
    fn save(&self, saved: &Snapshot) -> Result<()> {
        ensure!(
            serde_json::to_vec_pretty(saved)?.len() < MAX_BYTES,
            "YouTube cookie jar is too large; export a smaller YouTube session"
        );
        write_private_json(&self.path(), saved).context("Save YouTube cookies")
    }
    pub(crate) fn snapshot(&self) -> Result<Snapshot> {
        let file = match File::open(self.path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Snapshot::default());
            }
            Err(error) => return Err(error).context("Read saved YouTube cookies"),
        };
        let bytes = read_bounded(file)?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Saved YouTube cookies are invalid. Replace them in Settings or with youtube-cookies import."))?;
        ensure!(
            snapshot.entries.len() <= MAX_COOKIES
                && snapshot
                    .entries
                    .iter()
                    .all(|entry| valid_cookie(&entry.cookie)),
            "Saved YouTube cookies are invalid; import a fresh export"
        );
        Ok(snapshot)
    }
    pub fn is_enabled(&self) -> Result<bool> {
        Ok(self.snapshot()?.enabled)
    }
    pub fn import_file(&self, path: &Path, cancel: &CancellationToken) -> Result<()> {
        let bytes = read_bounded(File::open(path).context("Open cookie export")?)?;
        let text = std::str::from_utf8(&bytes).context("Cookie export must be UTF-8 text")?;
        self.import(text, cancel)
    }
    pub fn import(&self, text: &str, cancel: &CancellationToken) -> Result<()> {
        let cookies = parse_netscape(text)?;
        let saved = Snapshot {
            generation: revision(),
            enabled: true,
            entries: cookies
                .into_iter()
                .map(|cookie| Entry {
                    cookie,
                    revision: revision(),
                })
                .collect(),
        };
        let _lock = self.write_lock(cancel)?;
        self.save(&saved)
    }
    pub fn remove(&self, cancel: &CancellationToken) -> Result<()> {
        let _lock = self.write_lock(cancel)?;
        // Persist the new generation, so already-running requests cannot restore it.
        self.save(&Snapshot {
            generation: revision(),
            ..Snapshot::default()
        })
        .context("Remove YouTube cookies")
    }
    pub(crate) fn update(
        &self,
        base: &Snapshot,
        url: &Url,
        headers: &[String],
        cancel: &CancellationToken,
    ) -> Result<()> {
        if !base.enabled
            || headers.is_empty()
            || url.scheme() != "https"
            || !url.host_str().is_some_and(youtube_domain)
        {
            return Ok(());
        }
        let mut updates = Vec::new();
        for header in headers.iter().take(MAX_COOKIES) {
            if header.len() > 16 << 10 {
                continue;
            }
            let Ok(cookie) = Cookie::parse(header.clone(), url).map(Cookie::into_owned) else {
                continue;
            };
            if valid_cookie(&cookie) {
                // Last Set-Cookie for a key in the same response wins.
                updates.retain(|previous| !same_key(previous, &cookie));
                updates.push(cookie);
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        let _lock = self.write_lock(cancel)?;
        let mut current = self.snapshot()?;
        if current.generation != base.generation || !current.enabled {
            return Ok(());
        }
        let mut changed = false;
        for cookie in updates {
            if current.revision(&cookie) != base.revision(&cookie) {
                continue;
            }
            if let Some(entry) = current
                .entries
                .iter_mut()
                .find(|entry| same_key(&entry.cookie, &cookie))
            {
                *entry = Entry {
                    cookie,
                    revision: revision(),
                };
            } else {
                ensure!(
                    current.entries.len() < MAX_COOKIES,
                    "YouTube cookie jar is full; import a fresh export"
                );
                current.entries.push(Entry {
                    cookie,
                    revision: revision(),
                });
            }
            changed = true;
        }
        if changed {
            self.save(&current)?;
        }
        Ok(())
    }
}
fn revision() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn read_bounded(file: File) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("Read YouTube cookies")?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Cookie file is too large (maximum 1 MiB)"
    );
    Ok(bytes)
}
fn youtube_domain(domain: &str) -> bool {
    domain == "youtube.com" || domain.ends_with(".youtube.com")
}
fn domain<'a>(cookie: &'a Cookie<'_>) -> &'a str {
    match &cookie.domain {
        CookieDomain::HostOnly(host) | CookieDomain::Suffix(host) => host,
        _ => "",
    }
}
fn same_key(a: &Cookie<'_>, b: &Cookie<'_>) -> bool {
    domain(a) == domain(b) && a.path.as_ref() == b.path.as_ref() && a.name() == b.name()
}
fn valid_cookie(cookie: &Cookie<'_>) -> bool {
    youtube_domain(domain(cookie))
        && valid_pair(cookie.name(), cookie.value())
        && (!cookie.name().starts_with("__Secure-") || cookie.secure() == Some(true))
        && (!cookie.name().starts_with("__Host-")
            || (cookie.secure() == Some(true)
                && cookie.path.as_ref() == "/"
                && matches!(cookie.domain, CookieDomain::HostOnly(_))))
}
fn valid_pair(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        && value
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && !b"\";,\\".contains(&b))
}
fn parse_netscape(text: &str) -> Result<Vec<Cookie<'static>>> {
    ensure!(
        text.len() <= MAX_BYTES,
        "Cookie export is too large (maximum 1 MiB)"
    );
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    ensure!(
        matches!(
            lines.next().map(str::trim),
            Some("# Netscape HTTP Cookie File" | "# HTTP Cookie File")
        ),
        "Paste the entire Netscape cookies.txt export, including its header. JSON is not supported."
    );
    let mut cookies = Vec::new();
    for (index, line) in lines.enumerate() {
        let (line, http_only) = match line.strip_prefix("#HttpOnly_") {
            Some(line) => (line, true),
            None if line.starts_with('#') || line.trim().is_empty() => continue,
            None => (line, false),
        };
        let fields: Vec<_> = line.split('\t').collect();
        let invalid = || {
            anyhow::anyhow!(
                "Invalid cookies.txt row {}. Export cookies in Netscape format again.",
                index + 2
            )
        };
        if fields.len() != 7 {
            return Err(invalid());
        }
        let [host, subdomains, path, secure, expires, name, value] = fields.as_slice() else {
            unreachable!()
        };
        let host = host.trim_start_matches('.').to_ascii_lowercase();
        if !youtube_domain(&host) {
            continue;
        }
        if !matches!(*subdomains, "TRUE" | "FALSE")
            || !matches!(*secure, "TRUE" | "FALSE")
            || !valid_pair(name, value)
            || !path.starts_with('/')
            || path.bytes().any(|b| b <= 0x20 || b >= 0x7f || b == b';')
        {
            return Err(invalid());
        }
        let expires: i64 = expires.parse().map_err(|_| invalid())?;
        if expires < 0 {
            return Err(invalid());
        }
        if !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return Err(invalid());
        }
        let url = Url::parse(&format!("https://{host}/")).map_err(|_| invalid())?;
        let mut raw = cookie::Cookie::new((*name).to_owned(), (*value).to_owned());
        raw.set_path((*path).to_owned());
        raw.set_secure(*secure == "TRUE");
        raw.set_http_only(http_only);
        if *subdomains == "TRUE" {
            raw.set_domain(host);
        }
        if expires != 0 {
            raw.set_expires(
                cookie::time::OffsetDateTime::from_unix_timestamp(expires)
                    .map_err(|_| invalid())?,
            );
        }
        let cookie = Cookie::try_from_raw_cookie(&raw, &url)
            .map_err(|_| invalid())?
            .into_owned();
        if !valid_cookie(&cookie) {
            return Err(invalid());
        }
        if cookie.is_expired() {
            continue;
        }
        if cookies.iter().any(|previous| same_key(previous, &cookie)) {
            bail!(
                "Duplicate cookie in row {}; export cookies again",
                index + 2
            );
        }
        cookies.push(cookie);
        ensure!(
            cookies.len() <= MAX_COOKIES,
            "Too many YouTube cookies in this export"
        );
    }
    ensure!(
        !cookies.is_empty(),
        "No unexpired YouTube cookies found. Export a fresh YouTube session."
    );
    Ok(cookies)
}

#[cfg(test)]
mod tests;
