//! Native extraction boundary. Only scoped YouTube cookies cross stdout.
//! Browser discovery extensions follow Sweetcookie's MIT-licensed catalogue.
use anyhow::{Context, Result, ensure};
use rookie_cookies::{AppBoundPolicy, FromPathRequest, ReadRequest, enums::DetailedCookie};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    browser: String,
    profile: String,
}

#[derive(Deserialize)]
struct Browser {
    id: String,
    platforms: Vec<String>,
    engine: String,
    macos: String,
    linux: Vec<String>,
    windows: String,
    #[cfg(unix)]
    account: String,
    #[cfg(unix)]
    service: String,
}

#[derive(Serialize)]
struct Response {
    cookies: Vec<rookie_cookies::enums::Cookie>,
    warnings: Vec<String>,
    error: Option<String>,
}

fn main() {
    let response = match run() {
        Ok((cookies, warnings)) => Response {
            cookies,
            warnings,
            error: None,
        },
        Err(error) => Response {
            cookies: vec![],
            warnings: vec![],
            error: Some(error.to_string()),
        },
    };
    let _ = serde_json::to_writer(io::stdout().lock(), &response);
}

fn run() -> Result<(Vec<rookie_cookies::enums::Cookie>, Vec<String>)> {
    let mut input = Vec::new();
    io::stdin().take(16_385).read_to_end(&mut input)?;
    ensure!(input.len() <= 16_384, "Browser selection is too large");
    let request: Request = serde_json::from_slice(&input).context("Invalid browser selection")?;
    let browsers: Vec<Browser> = serde_json::from_str(include_str!("browsers.json"))?;
    let browser = browsers
        .into_iter()
        .find(|item| {
            item.id == request.browser && item.platforms.iter().any(|os| os == std::env::consts::OS)
        })
        .context("Browser is not supported on this operating system")?;
    let registered = rookie_cookies::supported_browsers()?
        .iter()
        .any(|item| item.id.as_str() == browser.id);
    let supplied = Path::new(&request.profile);
    let mut warnings = Vec::new();
    let cookies = if !request.profile.is_empty() && supplied.exists() || !registered {
        let path = resolve(&browser, &request.profile)?;
        read_path(&browser, &path, registered, &mut warnings)?
    } else {
        let mut query = ReadRequest::browser(&browser.id)
            .timeout(Duration::from_secs(20))
            .app_bound(AppBoundPolicy::Disabled);
        if !request.profile.is_empty() {
            query = query.profile(request.profile);
        }
        {
            let result = rookie_cookies::read(query).context("Cannot read browser cookies. Check browser access permissions and the selected profile")?;
            warnings.extend(result.warnings().iter().map(ToString::to_string));
            result.into_detailed_cookies()
        }
    };
    Ok((
        cookies
            .into_iter()
            .filter(|item| {
                let context = &item.context;
                matches!(
                    item.cookie.domain.as_str(),
                    "youtube.com" | ".youtube.com" | "www.youtube.com" | ".www.youtube.com"
                ) && context.user_context_id.unwrap_or(0) == 0
                    && context.private_browsing_id.unwrap_or(0) == 0
                    && context
                        .top_frame_site_key
                        .as_deref()
                        .is_none_or(str::is_empty)
                    && context.partition_key.as_deref().is_none_or(str::is_empty)
            })
            .map(|item| item.cookie)
            .collect(),
        warnings,
    ))
}

fn roots(browser: &Browser) -> Result<Vec<PathBuf>> {
    let home = std::env::home_dir().context("Cannot find the user home directory")?;
    Ok(match std::env::consts::OS {
        "macos" => vec![
            home.join("Library/Application Support")
                .join(&browser.macos),
        ],
        "windows" => {
            let variable = if browser.engine == "gecko" || browser.id == "opera" {
                "APPDATA"
            } else {
                "LOCALAPPDATA"
            };
            vec![
                PathBuf::from(
                    std::env::var_os(variable).context("Cannot find browser application data")?,
                )
                .join(&browser.windows),
            ]
        }
        "linux" => {
            let base = if browser.engine == "gecko" {
                home
            } else {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"))
            };
            browser.linux.iter().map(|path| base.join(path)).collect()
        }
        _ => vec![],
    })
}

fn database(directory: &Path, engine: &str) -> Option<PathBuf> {
    let names: &[&str] = match engine {
        "gecko" => &["cookies.sqlite"],
        "safari" => &["Cookies.binarycookies"],
        _ => &["Network/Cookies", "Cookies"],
    };
    names
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

fn resolve(browser: &Browser, profile: &str) -> Result<PathBuf> {
    let supplied = Path::new(profile);
    if !profile.is_empty() && supplied.is_file() {
        return Ok(supplied.to_owned());
    }
    if !profile.is_empty() && supplied.is_dir() {
        return database(supplied, &browser.engine)
            .context("Selected directory has no cookie database");
    }
    for root in roots(browser)? {
        if browser.engine == "gecko" {
            if let Ok(profiles) = ini::Ini::load_from_file(root.join("profiles.ini")) {
                let mut matches = Vec::new();
                for (section, values) in &profiles {
                    if !section.is_some_and(|name| name.starts_with("Profile")) {
                        continue;
                    }
                    let Some(path) = values.get("Path") else {
                        continue;
                    };
                    let path = if values.get("IsRelative") == Some("1") {
                        root.join(path)
                    } else {
                        PathBuf::from(path)
                    };
                    let name = values.get("Name").unwrap_or_default();
                    if !profile.is_empty()
                        && name != profile
                        && path.file_name().and_then(|v| v.to_str()) != Some(profile)
                    {
                        continue;
                    }
                    if let Some(db) = database(&path, "gecko") {
                        matches.push((
                            name != "default-release" && values.get("Default") != Some("1"),
                            db,
                        ));
                    }
                }
                matches.sort_by_key(|(secondary, _)| *secondary);
                if let Some((_, path)) = matches.into_iter().next() {
                    return Ok(path);
                }
            }
        } else {
            if profile.is_empty() {
                if let Some(path) = database(&root.join("Default"), &browser.engine)
                    .or_else(|| database(&root, &browser.engine))
                {
                    return Ok(path);
                }
            } else {
                if let Some(path) = database(&root.join(profile), &browser.engine) {
                    return Ok(path);
                }
                if let Ok(bytes) = std::fs::read(root.join("Local State"))
                    && let Ok(state) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && let Some(profiles) = state
                        .pointer("/profile/info_cache")
                        .and_then(|v| v.as_object())
                {
                    for (directory, info) in profiles {
                        if info.get("name").and_then(|v| v.as_str()) == Some(profile)
                            && let Some(path) = database(&root.join(directory), &browser.engine)
                        {
                            return Ok(path);
                        }
                    }
                }
            }
        }
    }
    anyhow::bail!(
        "Browser profile not found. Open YouTube in this browser, or select its profile directory"
    )
}

fn read_path(
    browser: &Browser,
    path: &Path,
    registered: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<DetailedCookie>> {
    if browser.engine == "chromium" && !registered {
        // Upstream's registry lacks some Sweetcookie brands. Its configurable
        // reader still owns database acquisition, decryption and cookie decoding.
        #[cfg(unix)]
        {
            #[allow(deprecated)]
            let config = rookie_cookies::config::Browser {
                paths: vec![],
                channels: None,
                unix_crypt_name: Some(browser.account.to_lowercase()),
                osx_key_service: Some(browser.service.clone()),
                osx_key_user: Some(browser.account.clone()),
            };
            #[allow(deprecated)]
            return rookie_cookies::chromium_based_detailed(
                &config,
                path.to_owned(),
                Some(vec!["youtube.com".into()]),
                false,
            )
            .context("Cannot read this browser's YouTube cookies");
        }
    }
    let mut request = FromPathRequest::new(path)
        .timeout(Duration::from_secs(20))
        .app_bound(AppBoundPolicy::Disabled);
    if browser.engine == "chromium" {
        #[cfg(unix)]
        {
            request = request.chromium_browser_id(&browser.id);
        }
        #[cfg(windows)]
        {
            let state = path
                .ancestors()
                .skip(1)
                .map(|dir| dir.join("Local State"))
                .find(|p| p.is_file())
                .context("Chromium Local State file is missing")?;
            request = request.chromium_local_state(state);
        }
    }
    let result =
        rookie_cookies::from_path(request).context("Cannot read the selected cookie database")?;
    warnings.extend(result.warnings().iter().map(ToString::to_string));
    Ok(result.into_detailed_cookies())
}
