use std::path::PathBuf;

use serde::Deserialize;

/// Which transport an *input* uses. Explicitly tagged by
/// `transport = "srt" | "ndi" | "omt" | "media"` — every `[[inputs]]` entry
/// must set it.
///
/// This used to be an untagged enum that picked SRT vs NDI implicitly from
/// their disjoint `mode` values (`listener`/`caller` vs `receiver`/
/// `sender`), so existing configs needed no `transport =` field. That trick
/// stopped being safe once OMT joined: OMT's `Endpoint` also uses
/// `receiver`/`sender`, and its `Sender` variant is shape-identical to
/// NDI's (`{mode: "sender", name: "..."}`) — untagged resolution can't
/// tell those apart by content, it would silently always pick whichever
/// variant is declared first. An explicit tag is unambiguous regardless of
/// how many transports share `mode` values with each other.
#[derive(Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum InputTransport {
    Srt(srt_io::Endpoint),
    #[cfg(feature = "ndi")]
    Ndi(ndi_io::Endpoint),
    #[cfg(feature = "omt")]
    Omt(omt_io::Endpoint),
    Media(media_io::Endpoint),
}

/// Which transport an *output* uses — a strict subset of [`InputTransport`]:
/// stills/local media/a scaler tap only ever make sense as something the
/// crosspoint reads *from*, never as a destination a router sends routed
/// video *to*, so there's no `Media` variant here the way there is above.
#[derive(Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum OutputTransport {
    Srt(srt_io::Endpoint),
    #[cfg(feature = "ndi")]
    Ndi(ndi_io::Endpoint),
    #[cfg(feature = "omt")]
    Omt(omt_io::Endpoint),
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
    pub endpoint: InputTransport,
}

#[derive(Deserialize)]
pub struct OutputConfig {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: OutputTransport,
    /// Source this output is routed from at startup.
    pub default_source: String,
}
