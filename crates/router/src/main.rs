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
#[derive(Parser, serde::Serialize)]
#[command(name = "srtrouter")]
struct Args {
    /// Path to the TOML config file.
    #[arg(short, long, default_value = "config/example.toml")]
    config: std::path::PathBuf,

    /// Write a diagnostics bundle and exit.
    ///
    /// Everything needed to investigate a fault in one file: build
    /// identity, platform, configuration with secrets removed, recent
    /// logs and any crash reports found.
    #[arg(long)]
    collect_diagnostics: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Before anything that can fail, so a failure during startup is logged
    // and lands in a crash report like any other.
    let _diag = diag::init(
        diag::Options::new("srtrouter", "SRT_ROUTER", env!("CARGO_PKG_VERSION"))
            .with_default_filter("info")
            .with_config(&args),
    )?;

    if args.collect_diagnostics {
        println!("{}", diag::collect_diagnostics()?.display());
        return Ok(());
    }

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
            config::InputTransport::Srt(ep) => {
                tracing::info!(id = %input.id, "starting SRT input");
                let cancel = srt_io::spawn_input(input.id.clone(), ep, crosspoint.clone());
                registry.insert_source(input.id, "srt", cancel);
            }
            #[cfg(feature = "ndi")]
            config::InputTransport::Ndi(ep) => {
                tracing::info!(id = %input.id, "starting NDI input");
                let cancel = ndi_io::spawn_input(input.id.clone(), ep, crosspoint.clone());
                registry.insert_source(input.id, "ndi", cancel);
            }
            #[cfg(feature = "omt")]
            config::InputTransport::Omt(ep) => {
                tracing::info!(id = %input.id, "starting OMT input");
                let cancel = omt_io::spawn_input(input.id.clone(), ep, crosspoint.clone());
                registry.insert_source(input.id, "omt", cancel);
            }
            config::InputTransport::Media(ep) => {
                tracing::info!(id = %input.id, "starting media input");
                let cancel = media_io::spawn_input(input.id.clone(), ep, crosspoint.clone());
                registry.insert_source(input.id, "media", cancel);
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
            config::OutputTransport::Srt(ep) => {
                tracing::info!(id = %output.id, source = %initial_source, "starting SRT output");
                let cancel =
                    srt_io::spawn_output(output.id.clone(), ep, initial_source, crosspoint.clone());
                registry.insert_output(output.id, "srt", cancel);
            }
            #[cfg(feature = "ndi")]
            config::OutputTransport::Ndi(ep) => {
                tracing::info!(id = %output.id, source = %initial_source, "starting NDI output");
                let cancel =
                    ndi_io::spawn_output(output.id.clone(), ep, initial_source, crosspoint.clone());
                registry.insert_output(output.id, "ndi", cancel);
            }
            #[cfg(feature = "omt")]
            config::OutputTransport::Omt(ep) => {
                tracing::info!(id = %output.id, source = %initial_source, "starting OMT output");
                let cancel =
                    omt_io::spawn_output(output.id.clone(), ep, initial_source, crosspoint.clone());
                registry.insert_output(output.id, "omt", cancel);
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
        registry: registry.clone(),
    };
    let kind_of: crosspoint_web::KindLookup = {
        let registry = registry.clone();
        std::sync::Arc::new(move |id: &str| registry.kind_of(id))
    };
    let app = crosspoint_web::app_with_kind_lookup(crosspoint, kind_of)
        .merge(management::router(manage_state));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "crosspoint web UI listening");
    axum::serve(listener, app).await?;
    Ok(())
}
