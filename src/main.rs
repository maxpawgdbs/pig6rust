use anyhow::{Result, ensure};
use clap::{Parser, ValueEnum};
use pig6bot::{
    bot::{App, Bot, Kind},
    config::Settings,
    web::{self, WebState},
};
use std::path::PathBuf;
#[derive(Clone, Copy, ValueEnum, Debug)]
enum Service {
    All,
    Main,
    Cert,
    Logs,
    Jday,
    Ai,
    Web,
    CheckConfig,
}
#[derive(Parser)]
#[command(version, about = "Pig-6: Telegram bots and market API")]
struct Args {
    #[arg(value_enum, default_value = "all")]
    service: Service,
    #[arg(long, default_value = ".")]
    data_dir: PathBuf,
    #[arg(long, default_value = "web")]
    web_dir: PathBuf,
    #[arg(long, default_value = ".env")]
    env_file: PathBuf,
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.env_file.exists() {
        dotenvy::from_path(&args.env_file)?;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pig6bot=info,tower_http=info".into()),
        )
        .init();
    let settings = Settings::from_env(args.data_dir, args.web_dir)?;
    let app = App::new(settings.clone())?;
    if matches!(args.service, Service::CheckConfig) {
        pig6bot::shadow::validate(&app.shadow.read()?)?;
        println!("Configuration and database are valid.");
        return Ok(());
    }
    let kinds: Vec<Kind> = match args.service {
        Service::All => vec![Kind::Main, Kind::Cert, Kind::Logs, Kind::Jday, Kind::Ai],
        Service::Main => vec![Kind::Main],
        Service::Cert => vec![Kind::Cert],
        Service::Logs => vec![Kind::Logs],
        Service::Jday => vec![Kind::Jday],
        Service::Ai => vec![Kind::Ai],
        _ => vec![],
    };
    let mut jobs = tokio::task::JoinSet::new();
    let mut tokens = std::collections::HashSet::new();
    for kind in kinds {
        let token = match std::env::var(kind.token_env())
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            Some(t) => t,
            None if matches!(args.service, Service::All) => {
                tracing::info!(variable = kind.token_env(), "Token not set; skipping bot");
                continue;
            }
            None => anyhow::bail!("{} is required", kind.token_env()),
        };
        ensure!(
            settings.owner != 0 && settings.main_channel != 0,
            "OWNER_ID and MAIN_CHANNEL_ID must be set before starting bots"
        );
        ensure!(
            tokens.insert(token.clone()),
            "Each bot must use a distinct token"
        );
        let app = app.clone();
        jobs.spawn(async move { Bot::run(app, kind, token).await });
    }
    if matches!(args.service, Service::All | Service::Main | Service::Web) {
        let state = WebState {
            settings,
            config: app.config.clone(),
            economy: app.economy.clone(),
            certificates: app.certificates.clone(),
        };
        jobs.spawn(web::serve(state));
    }
    ensure!(!jobs.is_empty(), "No services configured");
    if matches!(args.service, Service::All | Service::Main) {
        let app = app.clone();
        jobs.spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));
            loop {
                interval.tick().await;
                let capacity = pig6bot::config::id(&app.config.read()?["count"]);
                app.economy.replenish(capacity)?;
            }
        });
    }
    tokio::select! {
        result = jobs.join_next() => {
            jobs.abort_all();
            result.ok_or_else(|| anyhow::anyhow!("No running service"))???;
        },
        result = shutdown_signal() => {
            result?;
            tracing::info!("Stopping services");
            jobs.abort_all();
        }
    }
    while jobs.join_next().await.is_some() {}
    Ok(())
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! { _=terminate.recv()=>{}, result=tokio::signal::ctrl_c()=>{result?;} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
