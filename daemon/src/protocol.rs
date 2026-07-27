use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_CLOUD_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
pub const DEFAULT_CLOUD_MODEL: &str = "gemini-3.5-flash-lite";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineState {
    Idle,
    Recording,
    LoadingModel,
    Transcribing,
    Polishing,
    ResultReady,
    Error,
}

impl PipelineState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::LoadingModel => "loading_model",
            Self::Transcribing => "transcribing",
            Self::Polishing => "polishing",
            Self::ResultReady => "result_ready",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionOptions {
    pub language: String,
    pub vocabulary: Vec<String>,
    pub context_before: String,
    pub context_after: String,
    pub cloud_enabled: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            vocabulary: Vec::new(),
            context_before: String::new(),
            context_after: String::new(),
            cloud_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub cloud_base_url: String,
    pub cloud_model: String,
    pub cloud_api_key_required: bool,
    pub cloud_timeout_ms: u64,
    pub model_idle_minutes: u64,
    pub max_recording_seconds: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            cloud_base_url: DEFAULT_CLOUD_BASE_URL.into(),
            cloud_model: DEFAULT_CLOUD_MODEL.into(),
            cloud_api_key_required: true,
            cloud_timeout_ms: 4_000,
            model_idle_minutes: 15,
            max_recording_seconds: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptResult {
    pub text: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalResult {
    pub text: String,
    pub used_fallback: bool,
    pub timings_json: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerRequest {
    pub version: u32,
    pub request_id: String,
    pub sample_rate: u32,
    pub language: String,
    pub vocabulary: Vec<String>,
    pub max_new_tokens: u32,
    pub pcm_bytes: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerResponse {
    pub version: u32,
    pub request_id: String,
    pub ok: bool,
    pub text: String,
    pub language: String,
    pub error: String,
    pub inference_ms: u64,
}
