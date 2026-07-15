use serde::Deserialize;
use srt_io::Endpoint;

#[derive(Deserialize)]
pub struct Config {
    pub web: WebConfig,
    #[serde(default)]
    pub inputs: Vec<InputConfig>,
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
}

#[derive(Deserialize)]
pub struct WebConfig {
    pub bind: String,
}

#[derive(Deserialize)]
pub struct InputConfig {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: Endpoint,
}

#[derive(Deserialize)]
pub struct OutputConfig {
    pub id: String,
    #[serde(flatten)]
    pub endpoint: Endpoint,
    /// Source this output is routed from at startup.
    pub default_source: String,
}
