use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tokio::sync::{Mutex, RwLock};

use crate::{
    asr::AsrRouter,
    audio::Recording,
    openai_compat::OpenAiCompatClient,
    prompt::polished_output_rejection,
    protocol::{
        DEFAULT_ASR_BASE_URL, DEFAULT_ASR_MODEL, DEFAULT_CLOUD_BASE_URL, DEFAULT_CLOUD_MODEL,
        FinalResult, PipelineState, RuntimeConfig, SessionOptions, TranscriptResult,
    },
    secrets,
    vocabulary::{load_domain_vocabulary, merge_vocabulary, resolve_contextual_entities},
};

pub struct ProcessingSession {
    pub id: String,
    owner: String,
    options: SessionOptions,
    pcm: Vec<u8>,
    recorded_for: Duration,
    stopped_at: Instant,
}

struct ActiveSession {
    id: String,
    owner: String,
    options: SessionOptions,
    recording: Recording,
}

struct StoredResult {
    owner: String,
    value: FinalResult,
    expires_at: Instant,
}

struct Inner {
    state: PipelineState,
    detail: String,
    active: Option<ActiveSession>,
    processing: Option<(String, String)>,
    cancelled: HashSet<String>,
    results: HashMap<String, StoredResult>,
}

pub struct Pipeline {
    inner: Mutex<Inner>,
    runtime: RwLock<RuntimeConfig>,
    asr: AsrRouter,
    cloud: OpenAiCompatClient,
}

impl Pipeline {
    pub fn new(runtime: RuntimeConfig, asr: AsrRouter) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: Mutex::new(Inner {
                state: PipelineState::Idle,
                detail: String::new(),
                active: None,
                processing: None,
                cancelled: HashSet::new(),
                results: HashMap::new(),
            }),
            runtime: RwLock::new(runtime),
            asr,
            cloud: OpenAiCompatClient::new()?,
        }))
    }

    pub async fn configure(&self, mut runtime: RuntimeConfig) {
        runtime.asr_base_url = normalize_base_url(&runtime.asr_base_url, DEFAULT_ASR_BASE_URL);
        runtime.asr_model = normalize_model(&runtime.asr_model, DEFAULT_ASR_MODEL);
        runtime.asr_timeout_ms = runtime.asr_timeout_ms.clamp(1_000, 120_000);
        runtime.cloud_base_url = normalize_cloud_base_url(&runtime.cloud_base_url);
        runtime.cloud_model = normalize_cloud_model(&runtime.cloud_model);
        runtime.cloud_timeout_ms = runtime.cloud_timeout_ms.clamp(500, 15_000);
        runtime.model_idle_minutes = runtime.model_idle_minutes.clamp(1, 120);
        runtime.max_recording_seconds = runtime.max_recording_seconds.clamp(5, 300);
        self.asr.configure(&runtime).await;
        *self.runtime.write().await = runtime;
    }

    pub async fn prewarm_asr(&self) -> Result<()> {
        let runtime = self.runtime.read().await.clone();
        self.asr.prewarm(&runtime).await
    }

    pub async fn begin(
        self: &Arc<Self>,
        owner: String,
        session_id: String,
        mut options: SessionOptions,
    ) -> Result<()> {
        if session_id.is_empty() || session_id.len() > 128 {
            bail!("invalid session id");
        }
        options.vocabulary = merge_vocabulary(options.vocabulary, load_domain_vocabulary());
        options.vocabulary.truncate(128);
        options
            .vocabulary
            .iter_mut()
            .for_each(|word| *word = word.chars().take(80).collect());
        options.context_before = tail_chars(&options.context_before, 256);
        options.context_after = head_chars(&options.context_after, 64);

        let max_seconds = self.runtime.read().await.max_recording_seconds;
        {
            let inner = self.inner.lock().await;
            if inner.active.is_some() || inner.state != PipelineState::Idle {
                bail!("dictation pipeline is busy");
            }
        }

        let recording = Recording::start(max_seconds).await?;
        let mut inner = self.inner.lock().await;
        if inner.active.is_some() || inner.state != PipelineState::Idle {
            drop(inner);
            recording.cancel().await;
            bail!("dictation pipeline became busy");
        }
        inner.state = PipelineState::Recording;
        inner.detail.clear();
        inner.cancelled.remove(&session_id);
        inner.active = Some(ActiveSession {
            id: session_id.clone(),
            owner: owner.clone(),
            options,
            recording,
        });
        drop(inner);

        let pipeline = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(max_seconds + 2)).await;
            let _ = pipeline.cancel(&owner, &session_id).await;
        });
        Ok(())
    }

    pub async fn end(self: &Arc<Self>, owner: &str, session_id: &str) -> Result<ProcessingSession> {
        let active = {
            let mut inner = self.inner.lock().await;
            let active = inner
                .active
                .take()
                .context("there is no active recording")?;
            if active.id != session_id || active.owner != owner {
                inner.active = Some(active);
                bail!("session owner or id does not match");
            }
            inner.state = PipelineState::Transcribing;
            inner.processing = Some((active.id.clone(), active.owner.clone()));
            active
        };
        let recorded_for = active.recording.elapsed();
        let pcm = match active.recording.finish().await {
            Ok(pcm) => pcm,
            Err(error) => {
                self.fail_if_current(session_id, error.to_string()).await;
                return Err(error);
            }
        };
        Ok(ProcessingSession {
            id: active.id,
            owner: active.owner,
            options: active.options,
            pcm,
            recorded_for,
            stopped_at: Instant::now(),
        })
    }

    pub async fn transcribe(
        self: &Arc<Self>,
        session: &ProcessingSession,
    ) -> Result<TranscriptResult> {
        let runtime = self.runtime.read().await.clone();
        let result = self
            .asr
            .transcribe(
                &runtime,
                &session.id,
                &session.pcm,
                &session.options.language,
                &session.options.vocabulary,
            )
            .await;
        if let Err(error) = &result {
            self.fail_if_current(&session.id, error.to_string()).await;
        }
        result
    }

    pub async fn mark_polishing(&self, session_id: &str) -> bool {
        let mut inner = self.inner.lock().await;
        if inner.cancelled.remove(session_id) {
            return false;
        }
        if !matches!(&inner.processing, Some((id, _)) if id == session_id) {
            return false;
        }
        inner.state = PipelineState::Polishing;
        inner.detail.clear();
        true
    }

    pub async fn polish_and_store(
        self: &Arc<Self>,
        session: ProcessingSession,
        mut transcript: TranscriptResult,
    ) -> Option<FinalResult> {
        transcript.text = resolve_contextual_entities(&transcript.text);
        let asr_elapsed = session.stopped_at.elapsed();
        let cloud_started = Instant::now();
        let mut used_fallback = false;
        let mut final_text = transcript.text.clone();

        if session.options.cloud_enabled {
            let runtime = self.runtime.read().await.clone();
            let api_key = tokio::task::spawn_blocking(secrets::get_cloud_api_key)
                .await
                .ok()
                .and_then(Result::ok);
            if api_key.is_some() || !runtime.cloud_api_key_required {
                let mut cloud_options = session.options.clone();
                if cloud_options.language == "auto" && !transcript.language.is_empty() {
                    cloud_options.language = transcript.language.clone();
                }
                match self
                    .cloud
                    .polish(
                        api_key.as_deref(),
                        &runtime.cloud_base_url,
                        &runtime.cloud_model,
                        runtime.cloud_timeout_ms,
                        &transcript.text,
                        &cloud_options,
                    )
                    .await
                {
                    Ok(polished) => {
                        if let Some(reason) = polished_output_rejection(
                            &transcript.text,
                            &polished,
                            &session.options.vocabulary,
                        ) {
                            used_fallback = true;
                            tracing::warn!(
                                reason,
                                "cloud output failed validation; using local transcript"
                            );
                        } else {
                            final_text = polished;
                        }
                    }
                    Err(error) => {
                        used_fallback = true;
                        tracing::warn!(
                            error = %error,
                            "OpenAI-compatible post-processing failed; using local transcript"
                        );
                    }
                }
            } else {
                used_fallback = true;
                tracing::warn!("cloud API key unavailable; using local transcript");
            }
        }

        let timings = serde_json::json!({
            "recording_ms": session.recorded_for.as_millis(),
            "asr_ms": asr_elapsed.as_millis(),
            "cloud_ms": cloud_started.elapsed().as_millis(),
        });
        let result = FinalResult {
            text: final_text,
            used_fallback,
            timings_json: timings.to_string(),
        };
        let mut inner = self.inner.lock().await;
        if inner.cancelled.remove(&session.id)
            || !matches!(
                &inner.processing,
                Some((id, owner)) if id == &session.id && owner == &session.owner
            )
        {
            return None;
        }
        inner
            .results
            .retain(|_, item| item.expires_at > Instant::now());
        inner.results.insert(
            session.id.clone(),
            StoredResult {
                owner: session.owner,
                value: result.clone(),
                expires_at: Instant::now() + Duration::from_secs(30),
            },
        );
        inner.processing = None;
        inner.state = PipelineState::ResultReady;
        inner.detail.clear();
        drop(inner);

        let pipeline = Arc::clone(self);
        let session_id = session.id;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            pipeline.expire_result(&session_id).await;
        });
        Some(result)
    }

    pub async fn take_result(&self, owner: &str, session_id: &str) -> Result<FinalResult> {
        let mut inner = self.inner.lock().await;
        let item = inner
            .results
            .remove(session_id)
            .context("result is not ready or has expired")?;
        if item.owner != owner {
            inner.results.insert(session_id.to_owned(), item);
            bail!("result belongs to another DBus caller");
        }
        inner.state = PipelineState::Idle;
        inner.detail.clear();
        Ok(item.value)
    }

    pub async fn cancel(&self, owner: &str, session_id: &str) -> Result<()> {
        let active = {
            let mut inner = self.inner.lock().await;
            if let Some(active) = inner.active.take() {
                if active.owner != owner || active.id != session_id {
                    inner.active = Some(active);
                    bail!("session owner or id does not match");
                }
                Some(active)
            } else {
                let owns_processing = matches!(
                    &inner.processing,
                    Some((id, processing_owner))
                        if id == session_id && processing_owner == owner
                );
                let owns_result = matches!(
                    inner.results.get(session_id),
                    Some(result) if result.owner == owner
                );
                if !owns_processing && !owns_result {
                    bail!("session owner or id does not match");
                }
                None
            }
        };
        if let Some(active) = active {
            active.recording.cancel().await;
        }
        let mut inner = self.inner.lock().await;
        if let Some(result) = inner.results.get(session_id) {
            if result.owner != owner {
                bail!("result belongs to another DBus caller");
            }
        }
        let was_processing = matches!(
            &inner.processing,
            Some((id, processing_owner)) if id == session_id && processing_owner == owner
        );
        inner.processing = None;
        if was_processing && inner.state != PipelineState::Error {
            inner.cancelled.insert(session_id.to_owned());
        }
        inner.results.remove(session_id);
        inner.state = PipelineState::Idle;
        inner.detail.clear();
        Ok(())
    }

    pub async fn status(&self) -> (PipelineState, bool, String) {
        let inner = self.inner.lock().await;
        let state = inner.state;
        let detail = inner.detail.clone();
        drop(inner);
        let runtime = self.runtime.read().await.clone();
        (state, self.asr.is_ready(&runtime).await, detail)
    }

    async fn fail_if_current(self: &Arc<Self>, session_id: &str, detail: String) {
        let mut inner = self.inner.lock().await;
        if !matches!(&inner.processing, Some((id, _)) if id == session_id) {
            return;
        }
        inner.state = PipelineState::Error;
        inner.detail = detail;
        drop(inner);

        let pipeline = Arc::clone(self);
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let mut inner = pipeline.inner.lock().await;
            if inner.state == PipelineState::Error
                && matches!(&inner.processing, Some((id, _)) if id == &session_id)
            {
                inner.processing = None;
                inner.state = PipelineState::Idle;
                inner.detail.clear();
            }
        });
    }

    async fn expire_result(&self, session_id: &str) {
        let mut inner = self.inner.lock().await;
        if inner.results.remove(session_id).is_some() && inner.state == PipelineState::ResultReady {
            inner.state = PipelineState::Idle;
            inner.detail.clear();
        }
    }
}

fn head_chars(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn tail_chars(value: &str, count: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

fn normalize_cloud_model(value: &str) -> String {
    normalize_model(value, DEFAULT_CLOUD_MODEL)
}

fn normalize_model(value: &str, default: &str) -> String {
    let value = value.trim();
    if !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control) {
        value.to_owned()
    } else {
        default.to_owned()
    }
}

fn normalize_cloud_base_url(value: &str) -> String {
    normalize_base_url(value, DEFAULT_CLOUD_BASE_URL)
}

fn normalize_base_url(value: &str, default: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    let Ok(url) = reqwest::Url::parse(value) else {
        return default.to_owned();
    };
    if matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
    {
        value.to_owned()
    } else {
        default.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_bounded_on_unicode_boundaries() {
        assert_eq!(head_chars("你好世界", 2), "你好");
        assert_eq!(tail_chars("你好世界", 2), "世界");
    }

    #[test]
    fn openai_compatible_endpoint_and_model_are_bounded() {
        assert_eq!(normalize_cloud_model(" Qwen/Qwen3-32B "), "Qwen/Qwen3-32B");
        assert_eq!(normalize_cloud_model(""), DEFAULT_CLOUD_MODEL);
        assert_eq!(
            normalize_cloud_base_url("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1"
        );
        assert_eq!(
            normalize_cloud_base_url("file:///tmp/socket"),
            DEFAULT_CLOUD_BASE_URL
        );
    }
}
