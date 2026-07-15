//! Runtime add/remove API for sources and outputs — the dynamic counterpart
//! to the static `[[inputs]]`/`[[outputs]]` in the TOML config. Both paths
//! converge on the same `Registry`, so a config-defined source is exactly
//! as listable/removable here as one added later through this API.
//!
//! SRT-first: this only knows how to spawn SRT endpoints today. NDI has a
//! real, tested transport crate (`ndi-io`) but isn't wired in here yet —
//! see `docs/roadmap.md`.

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

#[derive(Deserialize)]
pub struct AddSourceRequest {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: SrtEndpointRequest,
}

#[derive(Deserialize)]
pub struct AddOutputRequest {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: SrtEndpointRequest,
    pub default_source: String,
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
    tracing::info!(id = %req.id, "adding SRT source via management API");
    let cancel = srt_io::spawn_input(
        req.id.clone(),
        req.endpoint.into(),
        state.crosspoint.clone(),
    );
    state.registry.insert_source(req.id.clone(), "srt", cancel);
    Ok(Json(ManageEntry {
        id: req.id,
        kind: "srt",
    }))
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
    tracing::info!(id = %req.id, source = %req.default_source, "adding SRT output via management API");
    let cancel = srt_io::spawn_output(
        req.id.clone(),
        req.endpoint.into(),
        req.default_source,
        state.crosspoint.clone(),
    );
    state.registry.insert_output(req.id.clone(), "srt", cancel);
    Ok(Json(ManageEntry {
        id: req.id,
        kind: "srt",
    }))
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
        .route("/api/manage/sources", get(list_sources).post(add_source))
        .route("/api/manage/sources/:id", delete(remove_source))
        .route("/api/manage/outputs", get(list_outputs).post(add_output))
        .route("/api/manage/outputs/:id", delete(remove_output))
        .with_state(state)
}
