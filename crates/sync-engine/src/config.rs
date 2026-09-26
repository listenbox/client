use crate::clientconfig::ClientConfig;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone)]
pub struct Config {
    values: ClientConfig,
    pub directory: PathBuf,
}

impl std::ops::Deref for Config {
    type Target = ClientConfig;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let directory = match std::env::var_os("LISTENBOX_PROFILE_DIR") {
            Some(directory) => {
                ensure!(!directory.is_empty(), "LISTENBOX_PROFILE_DIR is empty");
                PathBuf::from(directory)
            }
            None => {
                let home = std::env::home_dir().context("resolve user home directory")?;
                home.join(".config/listenbox")
            }
        };
        Self::load_in(explicit, directory)
    }

    /// Resolve configuration within an isolated client profile (also used by E2E).
    pub fn load_in(explicit: Option<&Path>, directory: PathBuf) -> Result<Self> {
        let default_path = directory.join("config.yaml");
        let path = explicit.unwrap_or(&default_path);
        ensure!(
            !path.as_os_str().is_empty(),
            "explicit config path is empty"
        );
        let mut values = match fs::read(path) {
            Ok(raw) => serde_yaml::from_slice::<ClientConfig>(&raw)
                .with_context(|| format!("decode client config {}", path.display()))?,
            Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
                ClientConfig {
                    api_origin: "https://v1.listenbox.app".into(),
                    dashboard_origin: "https://web.listenbox.app".into(),
                    print_trace_ids: false,
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("load client config {}", path.display()));
            }
        };
        values.api_origin = origin(&values.api_origin).context("validate api_origin")?;
        values.dashboard_origin =
            origin(&values.dashboard_origin).context("validate dashboard_origin")?;
        Ok(Self { values, directory })
    }

    pub fn show_url(&self, team: &str, show: &str) -> Result<String> {
        ensure!(
            crate::valid_id(team, "team_") && crate::valid_id(show, "shw_"),
            "invalid show management identifiers"
        );
        Ok(format!("{}/{team}/shows/{show}", self.dashboard_origin))
    }
}

fn origin(value: &str) -> Result<String> {
    let value = value.trim();
    let (_, authority) = value
        .split_once("://")
        .context("origin must have an explicit HTTP(S) authority")?;
    ensure!(
        !authority.is_empty()
            && !authority.starts_with('/')
            && !authority.contains(['@', '\\', '?', '#'])
            && authority
                .split_once('/')
                .is_none_or(|(_, path)| path.is_empty()),
        "origin must contain only a host and optional port"
    );
    let parsed = Url::parse(value).context("invalid origin URL")?;
    ensure!(
        matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
        "origin must be an absolute HTTP(S) URL"
    );
    ensure!(
        parsed.username().is_empty() && parsed.password().is_none(),
        "origin must not include user information"
    );
    ensure!(
        matches!(parsed.path(), "" | "/")
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "origin must not include a path, query or fragment"
    );
    Ok(parsed.origin().ascii_serialization())
}

pub fn write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let directory = path
        .parent()
        .context("private file requires parent directory")?;
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("replace {}", path.display()))?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

pub fn credential_valid(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|c| (0x21..=0x7e).contains(&c))
}

pub fn check_trace(value: &str) -> Result<()> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        || value.bytes().all(|c| c == b'0')
    {
        bail!("invalid X-Trace-Id {value:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_user_config_uses_production_defaults() {
        let home = tempfile::tempdir().unwrap();
        let config = Config::load_in(None, home.path().into()).unwrap();
        assert_eq!(config.api_origin, "https://v1.listenbox.app");
        assert_eq!(config.dashboard_origin, "https://web.listenbox.app");
        assert!(!config.print_trace_ids);
    }
    #[test]
    fn generated_config_decodes_yaml_and_normalizes_origins() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.yaml"), "api_origin: http://EXAMPLE.com:80/\ndashboard_origin: https://WEB.example.com:443/\nprint_trace_ids: true\n").unwrap();
        let config = Config::load_in(None, home.path().into()).unwrap();
        assert_eq!(config.api_origin, "http://example.com");
        assert_eq!(config.dashboard_origin, "https://web.example.com");
        assert!(config.print_trace_ids);
    }
}
