use std::path::PathBuf;

use serde::Deserialize;

/// Which transport an input/output uses. Untagged: `srt_io::Endpoint`'s
/// `mode` is `listener`/`caller` and `ndi_io::Endpoint`'s is
/// `receiver`/`sender` — disjoint, so serde picks the right variant from
/// the `mode` value alone with no explicit `transport =` field needed.
/// Existing SRT-only config files keep working unchanged.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Transport {
    Srt(srt_io::Endpoint),
    #[cfg(feature = "ndi")]
    Ndi(ndi_io::Endpoint),
}

#[derive(Deserialize)]
pub struct Config {
    pub web: WebConfig,
    #[serde(default)]
    pub inputs: Vec<InputConfig>,
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
    /// If present, routing changes are persisted to disk and reloaded on
    /// startup (overriding each output's `default_source`). Omit to keep
    /// routing in-memory only, reset to `default_source` on every restart.
    #[serde(default)]
    pub state: Option<StateConfig>,
}

#[derive(Deserialize)]
pub struct StateConfig {
    pub path: PathBuf,
}

#[derive(Deserialize)]
pub struct WebConfig {
    pub bind: String,
}

#[derive(Deserialize)]
pub struct InputConfig {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: Transport,
}

#[derive(Deserialize)]
pub struct OutputConfig {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: Transport,
    /// Source this output is routed from at startup.
    pub default_source: String,
}
