//! Optional persistence of the crosspoint's routing table across restarts.
//! Deliberately polling-based rather than a change-hook on
//! `crosspoint-core`: it keeps the core engine free of any
//! persistence-specific API, the same tradeoff the web UI already makes
//! with its own 1s state poll.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crosspoint_core::Crosspoint;
use tokio::time::sleep;

const PERSIST_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Load a previously persisted output -> source routing table. Any problem
/// reading or parsing the file (missing, corrupt, unreadable) is logged and
/// treated as "nothing persisted yet" rather than failing startup — a
/// router should still come up on its config defaults if its state file is
/// bad, not refuse to start.
pub fn load_routes(path: &Path) -> HashMap<String, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), %err, "state file exists but failed to parse, ignoring");
            HashMap::new()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to read state file, ignoring");
            HashMap::new()
        }
    }
}

fn save_routes(path: &Path, routes: &HashMap<String, String>) {
    let body = match serde_json::to_string_pretty(routes) {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(%err, "failed to serialize routing state");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                tracing::warn!(parent = %parent.display(), %err, "failed to create state directory");
                return;
            }
        }
    }
    // Write-then-rename so a crash mid-write can never leave a truncated
    // state file behind for the next startup to trip over.
    let tmp_path = path.with_extension("json.tmp");
    if let Err(err) = std::fs::write(&tmp_path, body) {
        tracing::warn!(path = %tmp_path.display(), %err, "failed to write state file");
        return;
    }
    if let Err(err) = std::fs::rename(&tmp_path, path) {
        tracing::warn!(path = %path.display(), %err, "failed to move state file into place");
    }
}

/// Spawn a task that watches the crosspoint's routing table and persists it
/// to `path` whenever it changes. Runs until the process exits.
pub fn spawn_persistence(path: PathBuf, crosspoint: Arc<Crosspoint>) {
    tokio::spawn(async move {
        let mut last = crosspoint.routes();
        loop {
            sleep(PERSIST_POLL_INTERVAL).await;
            let current = crosspoint.routes();
            if current != last {
                tracing::debug!(path = %path.display(), "routing changed, persisting");
                save_routes(&path, &current);
                last = current;
            }
        }
    });
}
