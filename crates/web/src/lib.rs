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

/// Given a source or output id, what transport kind is it (`"srt"`,
/// `"ndi"`, ...)? `None` if unknown. Supplied by the caller (the router
/// binary, which owns that bookkeeping in its `Registry`) so this crate
/// stays transport-agnostic — it only uses this to reject routing a source
/// to an output of a different kind, never to interpret the kind itself.
pub type KindLookup = Arc<dyn Fn(&str) -> Option<&'static str> + Send + Sync>;

#[derive(Clone)]
struct AppState {
    crosspoint: Arc<Crosspoint>,
    kind_of: Option<KindLookup>,
}

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

#[derive(Debug, Serialize, Deserialize)]
struct RouteResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    error: Option<String>,
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

async fn get_state(State(state): State<AppState>) -> Json<StateResponse> {
    Json(snapshot(&state.crosspoint))
}

async fn post_route(
    State(state): State<AppState>,
    Json(req): Json<RouteRequest>,
) -> Json<RouteResponse> {
    if let Some(kind_of) = &state.kind_of {
        if let (Some(out_kind), Some(src_kind)) = (kind_of(&req.output), kind_of(&req.source)) {
            if out_kind != src_kind {
                return Json(RouteResponse {
                    ok: false,
                    error: Some(format!(
                        "can't route a {src_kind} source to a {out_kind} output without transcoding"
                    )),
                });
            }
        }
    }
    let ok = state.crosspoint.route(&req.output, &req.source);
    Json(RouteResponse { ok, error: None })
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| push_state(socket, state.crosspoint))
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

fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/route", post(post_route))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

/// The crosspoint UI/API as a standalone `Router`, for callers (like
/// `srtrouter`) that want to `.merge()` in their own additional routes
/// (e.g. transport-specific source/output management) before serving. No
/// cross-kind route validation — use [`app_with_kind_lookup`] for that.
pub fn app(crosspoint: Arc<Crosspoint>) -> Router {
    build_app(AppState {
        crosspoint,
        kind_of: None,
    })
}

/// Same as [`app`], but rejects `POST /api/route` when the source and
/// output are known (via `kind_of`) to be different transport kinds — e.g.
/// an SRT source into an NDI output — since that would relay one
/// transport's envelope into a socket expecting another's, not a valid
/// stream. See `docs/architecture.md`'s note on why this isn't just a
/// wire-format difference.
pub fn app_with_kind_lookup(crosspoint: Arc<Crosspoint>, kind_of: KindLookup) -> Router {
    build_app(AppState {
        crosspoint,
        kind_of: Some(kind_of),
    })
}

/// Bind and serve the crosspoint web UI on its own, with no additional
/// routes merged in. Runs until the process exits.
pub async fn serve(bind: SocketAddr, crosspoint: Arc<Crosspoint>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "crosspoint web UI listening");
    axum::serve(listener, app(crosspoint)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn post_route_json(app: &Router, output: &str, source: &str) -> RouteResponse {
        let req = Request::builder()
            .method("POST")
            .uri("/api/route")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "output": output, "source": source }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn same_kind_route_is_allowed() {
        let xp = Crosspoint::new();
        xp.register_source("cam1");
        xp.register_output("program", "cam1");
        let kind_of: KindLookup = Arc::new(|_id: &str| Some("srt"));
        let app = app_with_kind_lookup(xp, kind_of);

        let res = post_route_json(&app, "program", "cam1").await;
        assert!(res.ok, "same-kind route should be allowed: {res:?}");
        assert!(res.error.is_none());
    }

    #[tokio::test]
    async fn cross_kind_route_is_rejected() {
        let xp = Crosspoint::new();
        xp.register_source("srt-cam"); // the output's untouched initial route
        xp.register_source("ndi-cam");
        xp.register_output("srt-out", "srt-cam");
        let kind_of: KindLookup = Arc::new(|id: &str| {
            if id == "ndi-cam" {
                Some("ndi")
            } else {
                Some("srt")
            }
        });
        let app = app_with_kind_lookup(xp.clone(), kind_of);

        let res = post_route_json(&app, "srt-out", "ndi-cam").await;
        assert!(!res.ok, "cross-kind route should be rejected");
        assert!(res.error.unwrap().contains("ndi"));
        // Rejected in the handler before ever reaching Crosspoint::route, so
        // the output's route must still be exactly what it started as.
        assert_eq!(
            xp.routes().get("srt-out").map(String::as_str),
            Some("srt-cam")
        );
    }

    #[tokio::test]
    async fn unknown_kind_falls_back_to_allowing() {
        // No guard at all (the `app()` constructor, not `app_with_kind_lookup`)
        // must behave exactly as before this feature existed.
        let xp = Crosspoint::new();
        xp.register_source("cam1");
        xp.register_output("program", "cam1");
        let app = app(xp);

        let res = post_route_json(&app, "program", "cam1").await;
        assert!(res.ok);
    }
}
