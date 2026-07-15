mod config;
mod management;
mod registry;
mod state;

use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use config::Config;
use crosspoint_core::Crosspoint;
use management::ManageState;
use registry::Registry;

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
    let registry = Registry::new();

    // Config-defined and management-API-added sources/outputs are
    // deliberately the same thing once running — both end up in `registry`
    // via the same spawn_input/spawn_output + insert_* calls, so a
    // config-declared source is exactly as removable via the API (or
    // listable in the web UI's add/remove menus) as one added later.
    for input in config.inputs {
        match input.endpoint {
            config::Transport::Srt(ep) => {
                tracing::info!(id = %input.id, "starting SRT input");
                let cancel = srt_io::spawn_input(input.id.clone(), ep, crosspoint.clone());
                registry.insert_source(input.id, "srt", cancel);
            }
            #[cfg(feature = "ndi")]
            config::Transport::Ndi(ep) => {
                tracing::info!(id = %input.id, "starting NDI input");
                let cancel = ndi_io::spawn_input(input.id.clone(), ep, crosspoint.clone());
                registry.insert_source(input.id, "ndi", cancel);
            }
        }
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
        match output.endpoint {
            config::Transport::Srt(ep) => {
                tracing::info!(id = %output.id, source = %initial_source, "starting SRT output");
                let cancel =
                    srt_io::spawn_output(output.id.clone(), ep, initial_source, crosspoint.clone());
                registry.insert_output(output.id, "srt", cancel);
            }
            #[cfg(feature = "ndi")]
            config::Transport::Ndi(ep) => {
                tracing::info!(id = %output.id, source = %initial_source, "starting NDI output");
                let cancel =
                    ndi_io::spawn_output(output.id.clone(), ep, initial_source, crosspoint.clone());
                registry.insert_output(output.id, "ndi", cancel);
            }
        }
    }

    if let Some(state_cfg) = config.state {
        state::spawn_persistence(state_cfg.path, crosspoint.clone());
    }

    let bind: SocketAddr = config
        .web
        .bind
        .parse()
        .with_context(|| format!("invalid web.bind address {:?}", config.web.bind))?;

    let manage_state = ManageState {
        crosspoint: crosspoint.clone(),
        registry,
    };
    let app = crosspoint_web::app(crosspoint).merge(management::router(manage_state));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "crosspoint web UI listening");
    axum::serve(listener, app).await?;
    Ok(())
}
