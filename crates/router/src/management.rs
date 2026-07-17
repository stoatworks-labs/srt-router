//! Runtime add/remove API for sources and outputs — the dynamic counterpart
//! to the static `[[inputs]]`/`[[outputs]]` in the TOML config. Both paths
//! converge on the same `Registry`, so a config-defined source is exactly
//! as listable/removable here as one added later through this API.
//!
//! Transport support here mirrors `config::Transport`: SRT always, NDI when
//! built with `--features ndi` (see `available_transports`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use crosspoint_core::Crosspoint;
use serde::{Deserialize, Serialize};

use crate::registry::Registry;

#[derive(Clone)]
pub struct ManageState {
    pub crosspoint: Arc<Crosspoint>,
    pub registry: Arc<Registry>,
}

/// Mirrors `srt_io::Endpoint`'s shape for JSON request bodies — kept as a
/// separate type rather than deriving `Deserialize` directly on
/// `srt_io::Endpoint` so this crate's wire format doesn't silently change
/// if that type's internals ever do.
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum SrtEndpointRequest {
    Listener { bind: String },
    Caller { connect: String },
}

impl From<SrtEndpointRequest> for srt_io::Endpoint {
    fn from(req: SrtEndpointRequest) -> Self {
        match req {
            SrtEndpointRequest::Listener { bind } => srt_io::Endpoint::Listener { bind },
            SrtEndpointRequest::Caller { connect } => srt_io::Endpoint::Caller { connect },
        }
    }
}

/// Mirrors `ndi_io::Endpoint`, same reasoning as `SrtEndpointRequest`.
#[cfg(feature = "ndi")]
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum NdiEndpointRequest {
    Receiver { source_name: String },
    Sender { name: String },
}

#[cfg(feature = "ndi")]
impl From<NdiEndpointRequest> for ndi_io::Endpoint {
    fn from(req: NdiEndpointRequest) -> Self {
        match req {
            NdiEndpointRequest::Receiver { source_name } => {
                ndi_io::Endpoint::Receiver { source_name }
            }
            NdiEndpointRequest::Sender { name } => ndi_io::Endpoint::Sender { name },
        }
    }
}

/// Untagged: SRT's `mode` is `listener`/`caller`, NDI's is
/// `receiver`/`sender` — disjoint, so serde picks the right variant from
/// the request body alone, same trick as `config::Transport`.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum EndpointRequest {
    Srt(SrtEndpointRequest),
    #[cfg(feature = "ndi")]
    Ndi(NdiEndpointRequest),
}

#[derive(Deserialize)]
pub struct AddSourceRequest {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: EndpointRequest,
}

#[derive(Deserialize)]
pub struct AddOutputRequest {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: EndpointRequest,
    pub default_source: String,
}

/// Transport kinds this build can actually spawn — `["srt"]` normally,
/// `["srt", "ndi"]` when built with `--features ndi`. The web UI fetches
/// this on load to decide which options its transport dropdowns enable,
/// rather than guessing from a compile-time constant baked into the page.
fn available_transports() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut kinds = vec!["srt"];
    #[cfg(feature = "ndi")]
    kinds.push("ndi");
    kinds
}

async fn list_transports() -> Json<Vec<&'static str>> {
    Json(available_transports())
}

#[derive(Serialize)]
pub struct ManageEntry {
    pub id: String,
    pub kind: &'static str,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

type ApiError = (StatusCode, Json<ErrorBody>);

fn conflict(msg: impl Into<String>) -> ApiError {
    (StatusCode::CONFLICT, Json(ErrorBody { error: msg.into() }))
}

fn not_found(msg: impl Into<String>) -> ApiError {
    (StatusCode::NOT_FOUND, Json(ErrorBody { error: msg.into() }))
}

async fn list_sources(State(state): State<ManageState>) -> Json<Vec<ManageEntry>> {
    let mut entries: Vec<_> = state
        .registry
        .sources()
        .into_iter()
        .map(|(id, kind)| ManageEntry { id, kind })
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Json(entries)
}

async fn list_outputs(State(state): State<ManageState>) -> Json<Vec<ManageEntry>> {
    let mut entries: Vec<_> = state
        .registry
        .outputs()
        .into_iter()
        .map(|(id, kind)| ManageEntry { id, kind })
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Json(entries)
}

async fn add_source(
    State(state): State<ManageState>,
    Json(req): Json<AddSourceRequest>,
) -> Result<Json<ManageEntry>, ApiError> {
    if req.id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "id must not be empty".into(),
            }),
        ));
    }
    if state.crosspoint.has_source(&req.id) {
        return Err(conflict(format!("source '{}' already exists", req.id)));
    }
    let kind = match req.endpoint {
        EndpointRequest::Srt(ep) => {
            tracing::info!(id = %req.id, "adding SRT source via management API");
            let cancel = srt_io::spawn_input(req.id.clone(), ep.into(), state.crosspoint.clone());
            state.registry.insert_source(req.id.clone(), "srt", cancel);
            "srt"
        }
        #[cfg(feature = "ndi")]
        EndpointRequest::Ndi(ep) => {
            tracing::info!(id = %req.id, "adding NDI source via management API");
            let cancel = ndi_io::spawn_input(req.id.clone(), ep.into(), state.crosspoint.clone());
            state.registry.insert_source(req.id.clone(), "ndi", cancel);
            "ndi"
        }
    };
    Ok(Json(ManageEntry { id: req.id, kind }))
}

async fn remove_source(
    State(state): State<ManageState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let Some(entry) = state.registry.remove_source(&id) else {
        return Err(not_found(format!("source '{id}' not found")));
    };
    tracing::info!(%id, "removing source via management API");
    entry.cancel.cancel();
    state.crosspoint.deregister_source(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn add_output(
    State(state): State<ManageState>,
    Json(req): Json<AddOutputRequest>,
) -> Result<Json<ManageEntry>, ApiError> {
    if req.id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "id must not be empty".into(),
            }),
        ));
    }
    if state.crosspoint.has_output(&req.id) {
        return Err(conflict(format!("output '{}' already exists", req.id)));
    }
    let kind = match req.endpoint {
        EndpointRequest::Srt(ep) => {
            tracing::info!(id = %req.id, source = %req.default_source, "adding SRT output via management API");
            let cancel = srt_io::spawn_output(
                req.id.clone(),
                ep.into(),
                req.default_source,
                state.crosspoint.clone(),
            );
            state.registry.insert_output(req.id.clone(), "srt", cancel);
            "srt"
        }
        #[cfg(feature = "ndi")]
        EndpointRequest::Ndi(ep) => {
            tracing::info!(id = %req.id, source = %req.default_source, "adding NDI output via management API");
            let cancel = ndi_io::spawn_output(
                req.id.clone(),
                ep.into(),
                req.default_source,
                state.crosspoint.clone(),
            );
            state.registry.insert_output(req.id.clone(), "ndi", cancel);
            "ndi"
        }
    };
    Ok(Json(ManageEntry { id: req.id, kind }))
}

async fn remove_output(
    State(state): State<ManageState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let Some(entry) = state.registry.remove_output(&id) else {
        return Err(not_found(format!("output '{id}' not found")));
    };
    tracing::info!(%id, "removing output via management API");
    entry.cancel.cancel();
    state.crosspoint.deregister_output(&id);
    Ok(StatusCode::NO_CONTENT)
}

pub fn router(state: ManageState) -> Router {
    Router::new()
        .route("/api/manage/transports", get(list_transports))
        .route("/api/manage/sources", get(list_sources).post(add_source))
        .route("/api/manage/sources/:id", delete(remove_source))
        .route("/api/manage/outputs", get(list_outputs).post(add_output))
        .route("/api/manage/outputs/:id", delete(remove_output))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> ManageState {
        ManageState {
            crosspoint: Crosspoint::new(),
            registry: Registry::new(),
        }
    }

    /// A real bind failure (address in use) is the only way to know for
    /// certain the SRT listener spawned by `add_source` actually holds the
    /// UDP socket, and that removing it actually released it — mirrors how
    /// this was checked by hand with `lsof` during development.
    fn udp_port_is_free(port: u16) -> bool {
        std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok()
    }

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = app.clone().oneshot(req).await.expect("request failed");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("read body");
        let body = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            // Axum's own extractor rejections (e.g. a malformed request
            // body) render as plain text, not JSON — fall back to a string
            // value instead of panicking so those cases stay testable too.
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
            })
        };
        (status, body)
    }

    fn post(uri: &str, json: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .unwrap()
    }

    fn delete(uri: &str) -> Request<Body> {
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn add_then_remove_source_really_binds_and_frees_the_port() {
        let state = test_state();
        let app = router(state.clone());
        const PORT: u16 = 19801;
        assert!(udp_port_is_free(PORT), "test port must start free");

        let (status, body) = call(
            &app,
            post(
                "/api/manage/sources",
                &format!(r#"{{"id":"test-src","mode":"listener","bind":"127.0.0.1:{PORT}"}}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["kind"], "srt");

        // The listener binds inside the spawned task, not synchronously
        // within the request — give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(state.crosspoint.has_source("test-src"));
        assert!(
            !udp_port_is_free(PORT),
            "port should be held by the spawned SRT listener"
        );

        let (status, _) = call(&app, delete("/api/manage/sources/test-src")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!state.crosspoint.has_source("test-src"));
        assert!(
            udp_port_is_free(PORT),
            "removing the source should free its port"
        );
    }

    #[tokio::test]
    async fn add_duplicate_id_is_a_conflict() {
        let state = test_state();
        state.crosspoint.register_source("dup");
        let app = router(state);

        let (status, body) = call(
            &app,
            post(
                "/api/manage/sources",
                r#"{"id":"dup","mode":"listener","bind":"127.0.0.1:19802"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].as_str().unwrap().contains("already exists"));
    }

    #[tokio::test]
    async fn remove_unknown_source_is_not_found() {
        let app = router(test_state());
        let (status, body) = call(&app, delete("/api/manage/sources/nope")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn add_output_requires_default_source_field() {
        let app = router(test_state());
        // Missing `default_source` — axum's Json extractor should reject
        // this as a deserialize error (422), not silently default it.
        let (status, _) = call(
            &app,
            post(
                "/api/manage/outputs",
                r#"{"id":"out1","mode":"listener","bind":"127.0.0.1:19803"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn transports_lists_srt_and_ndi_iff_the_feature_is_on() {
        let app = router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/api/manage/transports")
            .body(Body::empty())
            .unwrap();
        let (status, body) = call(&app, req).await;
        assert_eq!(status, StatusCode::OK);
        let kinds: Vec<String> = serde_json::from_value(body).unwrap();
        assert!(kinds.contains(&"srt".to_string()));
        assert_eq!(kinds.contains(&"ndi".to_string()), cfg!(feature = "ndi"));
    }

    #[cfg(feature = "ndi")]
    #[tokio::test]
    async fn add_ndi_source_is_dispatched_by_its_disjoint_mode_value() {
        // No real NDI SDK interaction here (that's ndi-io's own integration
        // test) — this only checks the untagged EndpointRequest picks the
        // NDI variant from `mode: "receiver"` and the registry ends up
        // tagged "ndi", not "srt".
        let state = test_state();
        let app = router(state.clone());
        let (status, body) = call(
            &app,
            post(
                "/api/manage/sources",
                r#"{"id":"ndi-src","mode":"receiver","source_name":"some-camera"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["kind"], "ndi");
        assert_eq!(state.registry.kind_of("ndi-src"), Some("ndi"));

        // The spawned NDI input's discovery loop runs forever looking for
        // "some-camera" (which doesn't exist) until cancelled — clean it up
        // explicitly so this test doesn't rely on ndi-io's ~5s cancellation
        // polling bound to finish promptly. Not just tidiness: leaving this
        // running is exactly the bug that made this test hang the whole
        // binary during development (tokio::test's implicit Runtime::drop
        // blocks on outstanding spawn_blocking tasks — see the note on
        // shutdown_background in crates/ndi-io/tests/relay.rs).
        state
            .registry
            .remove_source("ndi-src")
            .unwrap()
            .cancel
            .cancel();
    }
}
