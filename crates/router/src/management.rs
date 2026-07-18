//! Runtime add/remove API for sources and outputs — the dynamic counterpart
//! to the static `[[inputs]]`/`[[outputs]]` in the TOML config. Both paths
//! converge on the same `Registry`, so a config-defined source is exactly
//! as listable/removable here as one added later through this API.
//!
//! Transport support here mirrors `config::InputTransport`/
//! `OutputTransport`, including using the same explicit `transport` tag for
//! the same reason (see that module's doc comment: NDI's and OMT's
//! `Sender` request shapes are identical, so nothing short of an explicit
//! tag can tell them apart). SRT is always available; NDI/OMT depend on the
//! `ndi`/`omt` Cargo features — see `available_transports`. `media`
//! (stills/media-player/scaler) is source-only, same reasoning as
//! `config::OutputTransport` having no `Media` variant — hence the split
//! between `SourceEndpointRequest` and `EndpointRequest` below.

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

/// Mirrors `omt_io::Endpoint`. Its `mode` values (`receiver`/`sender`) and
/// `Sender` shape (`{mode: "sender", name: "..."}`) are identical to
/// NDI's — this is exactly why `EndpointRequest` below needs an explicit
/// tag rather than the untagged trick `SrtEndpointRequest`/
/// `NdiEndpointRequest` alone could once get away with.
#[cfg(feature = "omt")]
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum OmtEndpointRequest {
    Receiver { address: String },
    Sender { name: String },
}

#[cfg(feature = "omt")]
impl From<OmtEndpointRequest> for omt_io::Endpoint {
    fn from(req: OmtEndpointRequest) -> Self {
        match req {
            OmtEndpointRequest::Receiver { address } => omt_io::Endpoint::Receiver { address },
            OmtEndpointRequest::Sender { name } => omt_io::Endpoint::Sender { name },
        }
    }
}

/// Mirrors `media_io::Endpoint`, same reasoning as `SrtEndpointRequest`.
/// Fields are `Option` here (rather than reusing `media_io`'s own
/// `#[serde(default = ...)]` fns) so the "what happens if omitted" default
/// lives in one place, the `From` impl below, reusing `media_io`'s own
/// exported default functions rather than duplicating their literal
/// values.
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum MediaEndpointRequest {
    Stills {
        image_path: std::path::PathBuf,
        width: Option<u32>,
        height: Option<u32>,
    },
    MediaPlayer {
        file_path: std::path::PathBuf,
        loop_playback: Option<bool>,
        width: Option<u32>,
        height: Option<u32>,
    },
    Scaler {
        source: String,
        width: Option<u32>,
        height: Option<u32>,
    },
}

impl From<MediaEndpointRequest> for media_io::Endpoint {
    fn from(req: MediaEndpointRequest) -> Self {
        match req {
            MediaEndpointRequest::Stills {
                image_path,
                width,
                height,
            } => media_io::Endpoint::Stills {
                image_path,
                width: width.unwrap_or_else(media_io::default_width),
                height: height.unwrap_or_else(media_io::default_height),
            },
            MediaEndpointRequest::MediaPlayer {
                file_path,
                loop_playback,
                width,
                height,
            } => media_io::Endpoint::MediaPlayer {
                file_path,
                loop_playback: loop_playback.unwrap_or_else(media_io::default_loop_playback),
                width: width.unwrap_or_else(media_io::default_width),
                height: height.unwrap_or_else(media_io::default_height),
            },
            MediaEndpointRequest::Scaler {
                source,
                width,
                height,
            } => media_io::Endpoint::Scaler {
                source,
                width: width.unwrap_or_else(media_io::default_width),
                height: height.unwrap_or_else(media_io::default_height),
            },
        }
    }
}

/// Explicitly tagged by `transport` — see `config::InputTransport`'s doc
/// comment for why an implicit (untagged-by-`mode`) trick isn't safe once
/// more than one frame-based transport can be compiled in. Used for
/// `POST /api/manage/sources`; a superset of [`EndpointRequest`] (used for
/// outputs) since `media` sources have no output-side counterpart — see
/// `config::OutputTransport`.
#[derive(Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum SourceEndpointRequest {
    Srt(SrtEndpointRequest),
    #[cfg(feature = "ndi")]
    Ndi(NdiEndpointRequest),
    #[cfg(feature = "omt")]
    Omt(OmtEndpointRequest),
    Media(MediaEndpointRequest),
}

/// Explicitly tagged by `transport`. Used for `POST /api/manage/outputs` —
/// see [`SourceEndpointRequest`] for why this is a strict subset of it.
#[derive(Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum EndpointRequest {
    Srt(SrtEndpointRequest),
    #[cfg(feature = "ndi")]
    Ndi(NdiEndpointRequest),
    #[cfg(feature = "omt")]
    Omt(OmtEndpointRequest),
}

#[derive(Deserialize)]
pub struct AddSourceRequest {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: SourceEndpointRequest,
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
    let mut kinds = vec!["srt"];
    #[cfg(feature = "ndi")]
    kinds.push("ndi");
    #[cfg(feature = "omt")]
    kinds.push("omt");
    // media (stills/media-player/scaler) has no SDK/build dependency, so
    // it's always available — unlike ndi/omt it isn't gated by a Cargo
    // feature.
    kinds.push("media");
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
        SourceEndpointRequest::Srt(ep) => {
            tracing::info!(id = %req.id, "adding SRT source via management API");
            let cancel = srt_io::spawn_input(req.id.clone(), ep.into(), state.crosspoint.clone());
            state.registry.insert_source(req.id.clone(), "srt", cancel);
            "srt"
        }
        #[cfg(feature = "ndi")]
        SourceEndpointRequest::Ndi(ep) => {
            tracing::info!(id = %req.id, "adding NDI source via management API");
            let cancel = ndi_io::spawn_input(req.id.clone(), ep.into(), state.crosspoint.clone());
            state.registry.insert_source(req.id.clone(), "ndi", cancel);
            "ndi"
        }
        #[cfg(feature = "omt")]
        SourceEndpointRequest::Omt(ep) => {
            tracing::info!(id = %req.id, "adding OMT source via management API");
            let cancel = omt_io::spawn_input(req.id.clone(), ep.into(), state.crosspoint.clone());
            state.registry.insert_source(req.id.clone(), "omt", cancel);
            "omt"
        }
        SourceEndpointRequest::Media(ep) => {
            tracing::info!(id = %req.id, "adding media source via management API");
            let cancel = media_io::spawn_input(req.id.clone(), ep.into(), state.crosspoint.clone());
            state
                .registry
                .insert_source(req.id.clone(), "media", cancel);
            "media"
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
        #[cfg(feature = "omt")]
        EndpointRequest::Omt(ep) => {
            tracing::info!(id = %req.id, source = %req.default_source, "adding OMT output via management API");
            let cancel = omt_io::spawn_output(
                req.id.clone(),
                ep.into(),
                req.default_source,
                state.crosspoint.clone(),
            );
            state.registry.insert_output(req.id.clone(), "omt", cancel);
            "omt"
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
                &format!(
                    r#"{{"id":"test-src","transport":"srt","mode":"listener","bind":"127.0.0.1:{PORT}"}}"#
                ),
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
                r#"{"id":"dup","transport":"srt","mode":"listener","bind":"127.0.0.1:19802"}"#,
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
                r#"{"id":"out1","transport":"srt","mode":"listener","bind":"127.0.0.1:19803"}"#,
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
    async fn add_ndi_source_is_dispatched_by_its_transport_tag() {
        // No real NDI SDK interaction here (that's ndi-io's own integration
        // test) — this only checks EndpointRequest picks the Ndi variant
        // from `"transport":"ndi"` and the registry ends up tagged "ndi",
        // not "srt".
        let state = test_state();
        let app = router(state.clone());
        let (status, body) = call(
            &app,
            post(
                "/api/manage/sources",
                r#"{"id":"ndi-src","transport":"ndi","mode":"receiver","source_name":"some-camera"}"#,
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

    #[cfg(feature = "omt")]
    #[tokio::test]
    async fn add_omt_source_is_dispatched_by_its_transport_tag() {
        // Mirrors add_ndi_source_is_dispatched_by_its_transport_tag above,
        // for OMT. No real OMT SDK interaction (that's omt-io's own
        // integration test).
        let state = test_state();
        let app = router(state.clone());
        let (status, body) = call(
            &app,
            post(
                "/api/manage/sources",
                r#"{"id":"omt-src","transport":"omt","mode":"receiver","address":"some-camera"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["kind"], "omt");
        assert_eq!(state.registry.kind_of("omt-src"), Some("omt"));

        // Same reasoning as the NDI test: clean up the forever-discovering
        // task explicitly rather than relying on omt-io's cancellation
        // polling bound.
        state
            .registry
            .remove_source("omt-src")
            .unwrap()
            .cancel
            .cancel();
    }

    #[cfg(all(feature = "ndi", feature = "omt"))]
    #[tokio::test]
    async fn ndi_and_omt_sender_requests_are_not_confused_with_each_other() {
        // The actual regression test for the bug documented in
        // docs/roadmap.md: NDI's and OMT's Sender request shape is
        // identical ({mode: "sender", name: "..."}). Before the explicit
        // `transport` tag, an untagged EndpointRequest would have always
        // resolved both of these to whichever variant was declared first
        // in the enum — silently misrouting one as the other. With the
        // tag, each must resolve to its own kind despite the identical
        // shape everywhere except that one field.
        let state = test_state();
        let app = router(state.clone());

        let (status, body) = call(
            &app,
            post(
                "/api/manage/outputs",
                r#"{"id":"ndi-out","transport":"ndi","mode":"sender","name":"Shared Name","default_source":"nope"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "ndi request body: {body:?}");
        assert_eq!(body["kind"], "ndi");

        let (status, body) = call(
            &app,
            post(
                "/api/manage/outputs",
                r#"{"id":"omt-out","transport":"omt","mode":"sender","name":"Shared Name","default_source":"nope"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "omt request body: {body:?}");
        assert_eq!(body["kind"], "omt");

        assert_eq!(state.registry.kind_of("ndi-out"), Some("ndi"));
        assert_eq!(state.registry.kind_of("omt-out"), Some("omt"));

        state
            .registry
            .remove_output("ndi-out")
            .unwrap()
            .cancel
            .cancel();
        state
            .registry
            .remove_output("omt-out")
            .unwrap()
            .cancel
            .cancel();
    }

    /// Runs real `ffmpeg` (generating its own test image via lavfi, like
    /// `media-io`'s own integration test) — this checks the management API
    /// actually dispatches a `"transport":"media"` source request to
    /// `media_io::spawn_input` and registers it with kind `"media"`, not
    /// just that `MediaEndpointRequest` deserializes.
    #[tokio::test]
    async fn add_media_stills_source_is_dispatched_by_its_transport_tag() {
        let image_path = std::env::temp_dir().join("management-test-stills.png");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=green:s=64x64",
                "-frames:v",
                "1",
                image_path.to_str().unwrap(),
            ])
            .status()
            .expect("failed to run ffmpeg to generate test image");
        assert!(status.success());

        let state = test_state();
        let app = router(state.clone());
        let (status, body) = call(
            &app,
            post(
                "/api/manage/sources",
                &format!(
                    r#"{{"id":"stills-src","transport":"media","mode":"stills","image_path":"{}"}}"#,
                    image_path.display()
                ),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "response body: {body:?}");
        assert_eq!(body["kind"], "media");
        assert_eq!(state.registry.kind_of("stills-src"), Some("media"));

        state
            .registry
            .remove_source("stills-src")
            .unwrap()
            .cancel
            .cancel();
    }

    /// `media` (stills/media-player/scaler) is source-only — `Transport` has
    /// no `Media` variant on the output side (`config::OutputTransport`),
    /// and `EndpointRequest` mirrors that. A `"transport":"media"` output
    /// body should fail to deserialize, not silently fall through to some
    /// other variant.
    #[tokio::test]
    async fn media_transport_is_rejected_for_outputs() {
        let app = router(test_state());
        let (status, _) = call(
            &app,
            post(
                "/api/manage/outputs",
                r#"{"id":"bad-out","transport":"media","mode":"stills","image_path":"/tmp/x.png","default_source":"nope"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
