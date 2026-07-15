//! Local web UI + REST API for operating the crosspoint: a grid of
//! output-rows x source-columns, click a cell to route that output from
//! that source. No auth, no TLS — this is meant to run on a trusted
//! operations network, same as a hardware router's control port.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use crosspoint_core::Crosspoint;
use serde::{Deserialize, Serialize};

const INDEX_HTML: &str = include_str!("../static/index.html");

/// How often the websocket handler checks for a routing change to push.
/// Polling (rather than a change-hook on `crosspoint-core`) keeps the core
/// engine free of any web-specific API — same tradeoff the REST endpoint
/// and the router binary's own state-persistence task already make.
const PUSH_POLL_INTERVAL: Duration = Duration::from_millis(200);

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

fn snapshot(crosspoint: &Crosspoint) -> StateResponse {
    StateResponse {
        sources: crosspoint.source_ids(),
        outputs: crosspoint.output_ids(),
        routes: crosspoint.routes(),
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn get_state(State(crosspoint): State<Arc<Crosspoint>>) -> Json<StateResponse> {
    Json(snapshot(&crosspoint))
}

async fn post_route(
    State(crosspoint): State<Arc<Crosspoint>>,
    Json(req): Json<RouteRequest>,
) -> Json<RouteResponse> {
    let ok = crosspoint.route(&req.output, &req.source);
    Json(RouteResponse { ok })
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(crosspoint): State<Arc<Crosspoint>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| push_state(socket, crosspoint))
}

/// Push the crosspoint state to the client on connect and again every time
/// it changes, so the grid updates live without waiting on its own poll.
async fn push_state(mut socket: WebSocket, crosspoint: Arc<Crosspoint>) {
    let mut last: Option<String> = None;
    loop {
        let Ok(body) = serde_json::to_string(&snapshot(&crosspoint)) else {
            return;
        };
        if last.as_deref() != Some(body.as_str()) {
            if socket.send(Message::Text(body.clone())).await.is_err() {
                return;
            }
            last = Some(body);
        }
        tokio::select! {
            _ = tokio::time::sleep(PUSH_POLL_INTERVAL) => {}
            msg = socket.recv() => {
                // The client doesn't send anything meaningful; only its
                // disconnect (None) or an error needs to stop this task.
                if !matches!(msg, Some(Ok(_))) {
                    return;
                }
            }
        }
    }
}

/// The crosspoint UI/API as a standalone `Router`, for callers (like
/// `srtrouter`) that want to `.merge()` in their own additional routes
/// (e.g. transport-specific source/output management) before serving.
pub fn app(crosspoint: Arc<Crosspoint>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/route", post(post_route))
        .route("/ws", get(ws_handler))
        .with_state(crosspoint)
}

/// Bind and serve the crosspoint web UI on its own, with no additional
/// routes merged in. Runs until the process exits.
pub async fn serve(bind: SocketAddr, crosspoint: Arc<Crosspoint>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "crosspoint web UI listening");
    axum::serve(listener, app(crosspoint)).await
}
