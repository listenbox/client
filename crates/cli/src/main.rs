mod commands;
mod episodes;
use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use listenbox_sync_engine::{api, auth, config, youtube};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "listenbox",
    version,
    about = "Publish and manage podcasts on Listenbox"
)]
struct Cli {
    #[arg(long, global = true, help = "Path to CLI config file")]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authorize this CLI with Listenbox
    Login,
    /// Show current CLI authorization
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Import an RSS feed or public YouTube video, playlist or channel
    Import {
        #[arg(long)]
        slug: Option<String>,
        source_url: String,
    },
    /// Import or remove the YouTube session shared with Listenbox desktop
    YoutubeCookies {
        #[command(subcommand)]
        command: CookieCommand,
    },
    /// Manage shows by slug
    Shows {
        #[command(subcommand)]
        command: ShowCommand,
    },
    /// Manage episodes
    Episodes {
        #[command(subcommand)]
        command: EpisodeCommand,
    },
    /// Manage team or show members
    Members {
        #[command(subcommand)]
        command: MemberCommand,
    },
}

#[derive(Subcommand)]
enum CookieCommand {
    /// Replace saved cookies with a Netscape cookies.txt export
    Import { file: std::path::PathBuf },
    /// Remove the saved session and return to anonymous access
    Remove,
}

#[derive(Subcommand)]
enum AuthCommand {
    Status,
    /// Remove the credential shared with Listenbox desktop
    Logout,
}

#[derive(Subcommand)]
enum ShowCommand {
    List,
    /// Read feed order, or move the supplied episode IDs to the front in sequence
    Order {
        #[arg(long, value_parser = slug)]
        show: String,
        #[arg(long, value_parser = episode_id)]
        episode: Vec<String>,
    },
    /// Sync an imported podcast by slug using its saved YouTube source
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    Create {
        #[arg(long, value_parser = nonblank)]
        title: String,
        #[arg(long, value_parser = slug)]
        slug: String,
        #[arg(long = "type", value_enum)]
        source_kind: ShowKind,
        #[arg(long, value_parser = language)]
        language: String,
        #[arg(long)]
        artwork: Option<std::path::PathBuf>,
    },
    Delete {
        #[arg(long, value_parser = slug)]
        show: String,
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SyncCommand {
    /// No URL needed: the source was saved when the podcast was imported
    Youtube {
        #[arg(long, value_parser = slug)]
        show: String,
        /// Repeat in the foreground until SIGINT or SIGTERM
        #[arg(long)]
        watch: bool,
    },
}

#[derive(Clone, Copy, ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ShowKind {
    Audio,
    Video,
}

#[derive(Subcommand)]
enum EpisodeCommand {
    List {
        #[arg(long, value_parser = slug)]
        show: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=500))]
        limit: u16,
    },
    Create(EpisodeCreate),
    Delete {
        #[arg(long, value_parser = episode_id)]
        episode: String,
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Args)]
struct EpisodeCreate {
    #[arg(long, value_parser = slug)]
    show: String,
    #[arg(long, value_parser = nonblank)]
    title: String,
    #[arg(long, value_parser = nonblank)]
    description: Option<String>,
    #[arg(long)]
    file: std::path::PathBuf,
    #[arg(long, value_enum, default_value = "draft")]
    publication: Publication,
}

#[derive(Clone, Copy, PartialEq, ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Publication {
    Draft,
    Publish,
}

#[derive(Subcommand)]
enum MemberCommand {
    List {
        #[arg(long, value_parser = slug)]
        show: Option<String>,
    },
    Invite {
        #[arg(long, value_parser = nonblank)]
        email: String,
        #[arg(long, value_enum)]
        role: Role,
        #[arg(long, value_parser = slug)]
        show: Option<String>,
    },
    Role {
        #[arg(long, value_parser = nonblank)]
        member: String,
        #[arg(long, value_enum)]
        role: Role,
        #[arg(long, value_parser = slug)]
        show: Option<String>,
    },
    Remove {
        #[arg(long, value_parser = nonblank)]
        member: String,
        #[arg(long, required = true)]
        yes: bool,
        #[arg(long, value_parser = slug)]
        show: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Read,
    Write,
}

fn nonblank(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("must not be blank".into());
    }
    Ok(value.into())
}

fn slug(value: &str) -> Result<String, String> {
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

fn language(value: &str) -> Result<String, String> {
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

fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 16
            && suffix
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

fn episode_id(value: &str) -> Result<String, String> {
    if !valid_id(value, "ep_") {
        return Err(format!("invalid episode ID {value:?}"));
    }
    Ok(value.into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut terminate) => {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
                }
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        signal_cancel.cancel();
    });
    match run(cli, cancel.clone()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::ExitCode::from(if cancel.is_cancelled() { 130 } else { 1 })
        }
    }
}

async fn run(cli: Cli, cancel: CancellationToken) -> Result<()> {
    let config = config::Config::load(cli.config.as_deref())?;
    let mut api = api::Api::new(config, cancel)?;
    match cli.command {
        Command::YoutubeCookies { command } => {
            let jar = listenbox_sync_engine::cookies::CookieJar::new(&api.config.directory);
            let cancel = api.cancel.clone();
            tokio::task::spawn_blocking(move || match command {
                CookieCommand::Import { file } => jar.import_file(&file, &cancel),
                CookieCommand::Remove => jar.remove(&cancel),
            })
            .await??;
            println!("YouTube cookies updated. Sync your podcast to continue.");
            Ok(())
        }
        Command::Login => auth::login(&mut api).await,
        Command::Auth {
            command: AuthCommand::Logout,
        } => auth::logout(&api.config),
        command => {
            if api.credential.is_none() {
                bail!("not logged in; run listenbox login");
            }
            match command {
                Command::Auth {
                    command: AuthCommand::Status,
                } => auth::status(&api).await,
                Command::Import { slug, source_url } => {
                    if youtube::is_source(&source_url) {
                        youtube::import(&api, &listenbox_sync_engine::sync::Engine::default(), &source_url, slug.as_deref(), listenbox_sync_engine::publicapi::ShowSourceKind::Video, |event| {
                            if let youtube::ImportEvent::Created(show) = event {
                                eprintln!("Created podcast {:?}. Resume with shows sync youtube --show {}", show.slug, show.slug);
                            }
                        }).await.map(|(show, report)| {
                            println!("{}", show.slug);
                            eprintln!("{} added, {} skipped", report.added, report.skipped);
                            eprintln!("100%");
                            if let Ok(url) = api.config.show_url(&show.team_id, &show.id) { eprintln!("Open in Listenbox: {url}"); }
                        })
                    } else {
                        commands::import_rss(&api, &source_url, slug.as_deref()).await
                    }
                }
                Command::Shows { command } => commands::shows(&api, command).await,
                Command::Episodes { command } => episodes::run(&api, command).await,
                Command::Members { command } => commands::members(&api, command).await,
                Command::YoutubeCookies { .. }
                | Command::Login
                | Command::Auth {
                    command: AuthCommand::Logout,
                } => unreachable!(),
            }
        }
    }
}
