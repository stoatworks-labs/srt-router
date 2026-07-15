mod config;
mod state;

use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use config::Config;
use crosspoint_core::Crosspoint;

/// Crosspoint-based SRT router.
#[derive(Parser)]
#[command(name = "srtrouter")]
struct Args {
    /// Path to the TOML config file.
    #[arg(short, long, default_value = "config/example.toml")]
    config: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();
    let raw = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading config file {}", args.config.display()))?;
    let config: Config = toml::from_str(&raw)
        .with_context(|| format!("parsing config file {}", args.config.display()))?;

    let crosspoint = Crosspoint::new();

    for input in config.inputs {
        tracing::info!(id = %input.id, "starting SRT input");
        srt_io::spawn_input(input.id, input.endpoint, crosspoint.clone());
    }

    let persisted_routes: HashMap<String, String> = match &config.state {
        Some(state_cfg) => {
            let routes = state::load_routes(&state_cfg.path);
            if !routes.is_empty() {
                tracing::info!(
                    path = %state_cfg.path.display(),
                    count = routes.len(),
                    "loaded persisted routing state"
                );
            }
            routes
        }
        None => HashMap::new(),
    };

    for output in config.outputs {
        let initial_source = persisted_routes
            .get(&output.id)
            .cloned()
            .unwrap_or(output.default_source);
        tracing::info!(id = %output.id, source = %initial_source, "starting SRT output");
        srt_io::spawn_output(
            output.id,
            output.endpoint,
            initial_source,
            crosspoint.clone(),
        );
    }

    if let Some(state_cfg) = config.state {
        state::spawn_persistence(state_cfg.path, crosspoint.clone());
    }

    let bind: SocketAddr = config
        .web
        .bind
        .parse()
        .with_context(|| format!("invalid web.bind address {:?}", config.web.bind))?;

    crosspoint_web::serve(bind, crosspoint).await?;
    Ok(())
}
