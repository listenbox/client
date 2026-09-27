//! Explicit, YouTube-only browser session import shared by CLI and desktop.
use crate::config::{Config, write_private_json};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::Write,
    path::PathBuf,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const READER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/browser-cookies"));
const MAX_BYTES: u64 = 1 << 20;

#[derive(Clone, Deserialize)]
pub struct Browser {
    pub id: String,
    pub name: String,
    platforms: Vec<String>,
}

pub fn browsers() -> Vec<Browser> {
    let browsers: Vec<Browser> =
        serde_json::from_str(include_str!("../../browser-cookies/src/browsers.json"))
            .expect("valid bundled browser catalogue");
    browsers
        .into_iter()
        .filter(|browser| {
            browser
                .platforms
                .iter()
                .any(|platform| platform == std::env::consts::OS)
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub browser: String,
    pub imported_at: u64,
    pub count: usize,
    pub warnings: Vec<String>,
}

// Deliberately no Debug: credentials must never appear in diagnostic snapshots.
#[derive(Serialize, Deserialize)]
struct Cookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    expires: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    status: Status,
    cookies: Vec<Cookie>,
}

#[derive(Deserialize)]
struct ImportResult {
    #[serde(default)]
    cookies: Option<Vec<Cookie>>,
    #[serde(default)]
    warnings: Option<Vec<String>>,
    error: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn path(config: &Config) -> PathBuf {
    config.directory.join("youtube-cookies.json")
}

fn usable(cookie: &Cookie) -> bool {
    matches!(
        cookie.domain.as_str(),
        ".youtube.com" | "www.youtube.com" | ".www.youtube.com"
    ) && cookie.path == "/"
        && cookie.expires.is_none_or(|expiry| expiry > now() as i64)
        && !cookie.name.is_empty()
        && cookie
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        && cookie
            .value
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && !b"\";,\\".contains(&byte))
}

fn load(config: &Config) -> Result<Option<Saved>> {
    let file = match std::fs::File::open(path(config)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Read saved YouTube session"),
    };
    ensure!(
        file.metadata()?.len() <= MAX_BYTES,
        "Saved YouTube session is too large; import cookies again"
    );
    let mut saved: Saved = serde_json::from_reader(file)
        .map_err(|_| anyhow::anyhow!("Saved YouTube session is invalid; import cookies again"))?;
    saved.cookies.retain(usable);
    saved.status.count = saved.cookies.len();
    Ok(Some(saved))
}

pub fn status(config: &Config) -> Result<Option<Status>> {
    Ok(load(config)?.map(|saved| saved.status))
}

pub fn header(config: &Config) -> Result<Option<String>> {
    let Some(saved) = load(config)? else {
        return Ok(None);
    };
    ensure!(
        !saved.cookies.is_empty(),
        "YouTube cookies have expired. Import them again in Settings or with listenbox youtube-cookies import"
    );
    Ok(Some(
        saved
            .cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; "),
    ))
}

pub fn clear(config: &Config) -> Result<()> {
    match std::fs::remove_file(path(config)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("Remove saved YouTube cookies"),
    }
}

/// Manual input never opens browser files or invokes the native reader.
pub fn import_json(config: &Config, json: &str) -> Result<Status> {
    ensure!(
        json.len() as u64 <= MAX_BYTES,
        "Cookie JSON is too large (maximum 1 MiB)"
    );
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Input {
        name: String,
        value: String,
        domain: String,
        #[serde(default = "root_path")]
        path: String,
        #[serde(default)]
        secure: bool,
        #[serde(default)]
        expiration_date: Option<f64>,
        #[serde(default)]
        session: bool,
        #[serde(default)]
        host_only: Option<bool>,
        #[serde(default)]
        http_only: Option<bool>,
        #[serde(default)]
        same_site: Option<String>,
        #[serde(default)]
        store_id: Option<String>,
        #[serde(default)]
        id: Option<serde_json::Value>,
    }
    fn root_path() -> String {
        "/".into()
    }
    let input: Vec<Input> = serde_json::from_str(json).map_err(|_| anyhow::anyhow!("Use a JSON array of cookies with name, value and domain. Optional fields: path, secure, expirationDate, session, hostOnly, httpOnly, sameSite, storeId and id. Partitioned cookies are not supported"))?;
    let mut cookies = Vec::with_capacity(input.len());
    for item in input {
        let _ = (item.http_only, item.same_site, item.store_id, item.id);
        ensure!(
            item.expiration_date
                .is_none_or(|value| value.is_finite() && value >= 0. && value < i64::MAX as f64),
            "Cookie expirationDate must be a valid Unix timestamp in seconds"
        );
        cookies.push(Cookie {
            name: item.name,
            value: item.value,
            // Retain host-only scope even when an exporter includes a leading dot.
            domain: match item.host_only {
                Some(true) => item.domain.trim_start_matches('.').to_owned(),
                Some(false) if !item.domain.starts_with('.') => format!(".{}", item.domain),
                _ => item.domain,
            },
            path: item.path,
            secure: item.secure,
            expires: if item.session {
                None
            } else {
                item.expiration_date.map(|value| value as i64)
            },
        });
    }
    save(config, "json".into(), cookies, vec![])
}

fn save(
    config: &Config,
    browser: String,
    mut cookies: Vec<Cookie>,
    warnings: Vec<String>,
) -> Result<Status> {
    cookies.retain(usable);
    cookies.sort_by_key(|cookie| std::cmp::Reverse(cookie.domain.trim_start_matches('.').len()));
    let mut names = HashSet::new();
    cookies.retain(|cookie| names.insert(cookie.name.clone()));
    ensure!(
        !cookies.is_empty(),
        "No usable YouTube cookies found. Supply unexpired cookies for youtube.com with path /, or sign in to YouTube in the selected browser/profile"
    );
    let status = Status {
        browser,
        imported_at: now(),
        count: cookies.len(),
        warnings,
    };
    write_private_json(
        &path(config),
        &Saved {
            status: status.clone(),
            cookies,
        },
    )?;
    Ok(status)
}

pub async fn import(
    config: &Config,
    browser: &str,
    profile: Option<&str>,
    cancel: CancellationToken,
) -> Result<Status> {
    let browser = browsers().into_iter().find(|item| item.id == browser)
        .context("This browser is not supported on this operating system; run listenbox youtube-cookies browsers")?;
    ensure!(
        profile.is_none_or(|profile| profile.len() <= 4096),
        "Browser profile is too long"
    );
    ensure!(!cancel.is_cancelled(), "Cookie import cancelled");
    let directory = tempfile::tempdir().context("Create private browser reader directory")?;
    let executable = directory.path().join(if cfg!(windows) {
        "browser-cookies.exe"
    } else {
        "browser-cookies"
    });
    let mut file = std::fs::File::create(&executable)?;
    file.write_all(READER)?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    }
    let request = serde_json::to_vec(
        &serde_json::json!({"browser":browser.id, "profile":profile.unwrap_or_default()}),
    )?;
    let bytes = run_reader(
        tokio::process::Command::new(&executable),
        &request,
        cancel.clone(),
    )
    .await?;
    let imported: ImportResult = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Browser cookie reader returned an invalid result"))?;
    if let Some(error) = imported.error {
        bail!("{error}");
    }
    ensure!(!cancel.is_cancelled(), "Cookie import cancelled");
    save(
        config,
        browser.id,
        imported.cookies.unwrap_or_default(),
        imported.warnings.unwrap_or_default(),
    )
}

// The importer owns the reader and its descendants until the pipes close and
// the child is reaped. A cancelled/failed reader never reaches session commit.
async fn run_reader(
    mut command: tokio::process::Command,
    request: &[u8],
    cancel: CancellationToken,
) -> Result<Vec<u8>> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command
        .spawn()
        .context("Start bundled browser cookie reader")?;
    let pid = child.id();
    let mut stdin = child.stdin.take().context("Browser reader input pipe")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Browser reader output pipe")?
        .take(MAX_BYTES + 1);
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow::anyhow!("Cookie import cancelled")),
        _ = tokio::time::sleep(Duration::from_secs(25)) => Err(anyhow::anyhow!("Browser access timed out. Close the browser or approve its keychain prompt, then import again")),
        result = async {
            stdin.write_all(request).await.context("Send browser selection to the cookie reader")?;
            drop(stdin);
            let read = async {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).await?;
                ensure!(bytes.len() as u64 <= MAX_BYTES, "Browser cookie result is too large");
                Ok::<_, anyhow::Error>(bytes)
            };
            let (bytes, status) = tokio::try_join!(read, async {Ok::<_, anyhow::Error>(child.wait().await?)})?;
            ensure!(status.success(), "Browser cookie reader did not finish");
            Ok(bytes)
        } => result,
    };
    if result.is_err() {
        #[cfg(unix)]
        if let Some(pid) = pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = pid;
        let _ = child.kill().await;
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::{io::OwnedFd, net::UnixStream};
    use tokio::{io::AsyncBufReadExt, net::UnixListener};

    // Re-executed in an isolated process; ordinary test runs do no work here.
    #[test]
    fn reader_fixture() {
        let Some(path) = std::env::var_os("LISTENBOX_COOKIE_READER_FIXTURE") else {
            return;
        };
        if path == "oversized" {
            std::io::stdout()
                .write_all(&vec![b'x'; MAX_BYTES as usize + 1])
                .unwrap();
            std::thread::park();
            return;
        }
        let mut gate = UnixStream::connect(path).unwrap();
        let input: OwnedFd = gate.try_clone().unwrap().into();
        let mut descendant = std::process::Command::new("/bin/cat")
            .stdin(Stdio::from(input))
            .stdout(Stdio::inherit())
            .spawn()
            .unwrap();
        writeln!(gate, "{}", std::process::id()).unwrap();
        let _ = descendant.wait();
    }

    fn fixture(path: &std::ffi::OsStr) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "cookies::tests::reader_fixture", "--nocapture"])
            .env("LISTENBOX_COOKIE_READER_FIXTURE", path);
        command
    }

    async fn stops_admitted_reader(timeout: bool) {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("gate");
        let listener = UnixListener::bind(&socket).unwrap();
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let command = fixture(socket.as_os_str());
            async move { run_reader(command, b"{}", cancel).await }
        });
        let (gate, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut gate = tokio::io::BufReader::new(gate);
        let mut pid = String::new();
        tokio::time::timeout(Duration::from_secs(2), gate.read_line(&mut pid))
            .await
            .unwrap()
            .unwrap();
        let pid: i32 = pid.trim().parse().unwrap();
        if timeout {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(26)).await;
            tokio::time::resume();
        } else {
            cancel.cancel();
        }
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(if timeout { "timed out" } else { "cancelled" })
        );
        // EOF proves both the admitted reader and its pipe-holding descendant
        // have closed their inherited handles; the parent has also been reaped.
        let mut rest = Vec::new();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), gate.read_to_end(&mut rest))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[tokio::test]
    async fn cancellation_closes_reader_and_descendant() {
        stops_admitted_reader(false).await;
    }

    #[tokio::test]
    async fn timeout_closes_reader_and_descendant() {
        stops_admitted_reader(true).await;
    }

    #[tokio::test]
    async fn oversized_output_stops_reader_without_waiting_for_exit() {
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            run_reader(
                fixture(std::ffi::OsStr::new("oversized")),
                b"{}",
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("too large"));
    }
}
