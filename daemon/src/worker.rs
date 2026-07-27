use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    time::timeout,
};

use crate::{
    config::ProcessConfig,
    protocol::{PROTOCOL_VERSION, TranscriptResult, WorkerRequest, WorkerResponse},
};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(180);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(180);

struct WorkerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct WorkerManager {
    config: ProcessConfig,
    process: Mutex<Option<WorkerProcess>>,
    idle_minutes: AtomicU64,
    idle_generation: AtomicU64,
    ready: AtomicBool,
}

impl WorkerManager {
    pub fn new(config: ProcessConfig) -> Arc<Self> {
        Arc::new(Self {
            idle_minutes: AtomicU64::new(config.runtime.model_idle_minutes),
            config,
            process: Mutex::new(None),
            idle_generation: AtomicU64::new(0),
            ready: AtomicBool::new(false),
        })
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn set_idle_minutes(&self, value: u64) {
        self.idle_minutes
            .store(value.clamp(1, 120), Ordering::Release);
    }

    pub async fn warm(self: &Arc<Self>) -> Result<()> {
        let mut guard = self.process.lock().await;
        self.ensure_started(&mut guard).await
    }

    pub async fn transcribe(
        self: &Arc<Self>,
        request_id: &str,
        pcm: &[u8],
        language: &str,
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        let first = self
            .transcribe_once(request_id, pcm, language, vocabulary)
            .await;
        let result = match first {
            Ok(value) => Ok(value),
            Err(first_error) => {
                tracing::warn!(error = %first_error, "ASR worker failed; restarting once");
                self.stop().await;
                self.transcribe_once(request_id, pcm, language, vocabulary)
                    .await
                    .with_context(|| format!("ASR worker failed after restart: {first_error}"))
            }
        };
        self.schedule_idle_unload();
        result
    }

    async fn transcribe_once(
        &self,
        request_id: &str,
        pcm: &[u8],
        language: &str,
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        let mut guard = self.process.lock().await;
        self.ensure_started(&mut guard).await?;
        let worker = guard.as_mut().context("ASR worker was not started")?;
        let seconds = pcm.len() as f64 / 32_000.0;
        let max_new_tokens = ((seconds * 8.0).ceil() as u32).clamp(128, 2048);
        let pcm_bytes = u32::try_from(pcm.len()).context("recording is too large")?;
        let request = WorkerRequest {
            version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            sample_rate: 16_000,
            language: language.to_owned(),
            vocabulary: vocabulary.to_vec(),
            max_new_tokens,
            pcm_bytes,
        };
        let header = serde_json::to_vec(&request).context("encode worker request")?;
        write_frame_header(&mut worker.stdin, header.len()).await?;
        worker
            .stdin
            .write_all(&header)
            .await
            .context("write ASR request header")?;
        worker
            .stdin
            .write_all(pcm)
            .await
            .context("write ASR audio")?;
        worker.stdin.flush().await.context("flush ASR request")?;

        let response: WorkerResponse =
            timeout(INFERENCE_TIMEOUT, read_json_frame(&mut worker.stdout))
                .await
                .context("ASR inference timed out")??;
        if response.version != PROTOCOL_VERSION || response.request_id != request_id {
            bail!("ASR worker returned a mismatched protocol response");
        }
        if !response.ok {
            bail!("ASR inference failed: {}", response.error);
        }
        let text = response.text.trim().to_owned();
        if text.is_empty() {
            bail!("ASR returned no speech");
        }
        Ok(TranscriptResult {
            text,
            language: response.language,
        })
    }

    async fn ensure_started(&self, guard: &mut Option<WorkerProcess>) -> Result<()> {
        if guard.is_some() {
            return Ok(());
        }
        self.ready.store(false, Ordering::Release);
        let mut child = Command::new(&self.config.worker_python)
            .args(["-m", &self.config.worker_module])
            .env("PYTHONUNBUFFERED", "1")
            .env("NATSU_TYPELESS_MODEL", &self.config.model_id)
            .env("NATSU_TYPELESS_MODEL_REVISION", &self.config.model_revision)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "start ASR worker with {}",
                    self.config.worker_python.display()
                )
            })?;
        let stdin = child.stdin.take().context("ASR worker stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("ASR worker stdout unavailable")?;
        let mut process = WorkerProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let ready: WorkerResponse = timeout(STARTUP_TIMEOUT, read_json_frame(&mut process.stdout))
            .await
            .context("ASR model load timed out")??;
        if !ready.ok || ready.request_id != "__ready__" {
            bail!("ASR worker failed to initialize: {}", ready.error);
        }
        self.ready.store(true, Ordering::Release);
        *guard = Some(process);
        Ok(())
    }

    pub async fn stop(&self) {
        self.ready.store(false, Ordering::Release);
        if let Some(mut process) = self.process.lock().await.take() {
            let _ = process.child.kill().await;
        }
    }

    pub fn schedule_idle_unload(self: &Arc<Self>) {
        let generation = self.idle_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let minutes = self.idle_minutes.load(Ordering::Acquire);
        let this = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(minutes * 60)).await;
            if this.idle_generation.load(Ordering::Acquire) == generation {
                tracing::info!(minutes, "unloading idle ASR model");
                this.stop().await;
            }
        });
    }
}

async fn write_frame_header(writer: &mut ChildStdin, length: usize) -> Result<()> {
    let length = u32::try_from(length).context("worker frame too large")?;
    writer
        .write_all(&length.to_le_bytes())
        .await
        .context("write worker frame length")
}

async fn read_json_frame<T: serde::de::DeserializeOwned>(
    reader: &mut BufReader<ChildStdout>,
) -> Result<T> {
    let mut length = [0_u8; 4];
    reader
        .read_exact(&mut length)
        .await
        .context("read worker frame length")?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_RESPONSE_BYTES {
        bail!("invalid worker response length: {length}");
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .context("read worker response")?;
    serde_json::from_slice(&payload).context("decode worker response")
}
