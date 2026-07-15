//! Local web UI + REST API for operating the crosspoint: a grid of
//! output-rows x source-columns, click a cell to route that output from
//! that source. No auth, no TLS — this is meant to run on a trusted
//! operations network, same as a hardware router's control port.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use crosspoint_core::Crosspoint;
use serde::{Deserialize, Serialize};

const INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Serialize)]
struct StateResponse {
    sources: Vec<String>,
    outputs: Vec<String>,
    routes: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct RouteRequest {
    output: String,
    source: String,
}

#[derive(Serialize)]
struct RouteResponse {
    ok: bool,
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn get_state(State(crosspoint): State<Arc<Crosspoint>>) -> Json<StateResponse> {
    Json(StateResponse {
        sources: crosspoint.source_ids(),
        outputs: crosspoint.output_ids(),
        routes: crosspoint.routes(),
    })
}

async fn post_route(
    State(crosspoint): State<Arc<Crosspoint>>,
    Json(req): Json<RouteRequest>,
) -> Json<RouteResponse> {
    let ok = crosspoint.route(&req.output, &req.source);
    Json(RouteResponse { ok })
}

fn app(crosspoint: Arc<Crosspoint>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/route", post(post_route))
        .with_state(crosspoint)
}

/// Bind and serve the crosspoint web UI. Runs until the process exits.
pub async fn serve(bind: SocketAddr, crosspoint: Arc<Crosspoint>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "crosspoint web UI listening");
    axum::serve(listener, app(crosspoint)).await
}
