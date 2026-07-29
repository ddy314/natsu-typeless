use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, multipart};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    protocol::{AsrProvider, RuntimeConfig, TranscriptResult},
    secrets,
    worker::WorkerManager,
};

#[derive(Clone)]
pub struct AsrRouter {
    local: Arc<WorkerManager>,
    api: AsrApiClient,
}

impl AsrRouter {
    pub fn new(local: Arc<WorkerManager>) -> Result<Self> {
        Ok(Self {
            local,
            api: AsrApiClient::new()?,
        })
    }

    pub async fn configure(&self, config: &RuntimeConfig) {
        self.local.set_idle_minutes(config.model_idle_minutes);
        if config.asr_provider != AsrProvider::Local {
            self.local.stop().await;
        }
    }

    pub async fn prewarm(&self, config: &RuntimeConfig) -> Result<()> {
        if config.asr_provider == AsrProvider::Local {
            self.local.warm().await?;
            self.local.schedule_idle_unload();
        }
        Ok(())
    }

    pub async fn transcribe(
        &self,
        config: &RuntimeConfig,
        request_id: &str,
        pcm: &[u8],
        language: &str,
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        match config.asr_provider {
            AsrProvider::Local => {
                self.local
                    .transcribe(request_id, pcm, language, vocabulary)
                    .await
            }
            AsrProvider::QwenApi | AsrProvider::OpenaiCompatible => {
                let api_key = tokio::task::spawn_blocking(secrets::get_asr_api_key)
                    .await
                    .ok()
                    .and_then(Result::ok);
                if api_key.is_none() && config.asr_api_key_required {
                    bail!("ASR API key is unavailable; run `natsu-typelessctl asr-key set`");
                }
                self.api
                    .transcribe(config, api_key.as_deref(), pcm, language, vocabulary)
                    .await
            }
        }
    }

    pub async fn is_ready(&self, config: &RuntimeConfig) -> bool {
        if config.asr_provider == AsrProvider::Local {
            return self.local.is_ready();
        }
        if !config.asr_api_key_required {
            return true;
        }
        tokio::task::spawn_blocking(secrets::has_asr_api_key)
            .await
            .unwrap_or(false)
    }
}

#[derive(Clone)]
struct AsrApiClient {
    client: Client,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<AsrChoice>,
}

#[derive(Debug, Deserialize)]
struct AsrChoice {
    message: AsrMessage,
}

#[derive(Debug, Deserialize)]
struct AsrMessage {
    content: String,
    #[serde(default)]
    annotations: Vec<AsrAnnotation>,
}

#[derive(Debug, Deserialize)]
struct AsrAnnotation {
    #[serde(default)]
    language: String,
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
    #[serde(default)]
    language: String,
}

impl AsrApiClient {
    fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("natsu-typeless/", env!("CARGO_PKG_VERSION")))
            .http2_adaptive_window(true)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .context("build ASR API HTTP client")?;
        Ok(Self { client })
    }

    async fn transcribe(
        &self,
        config: &RuntimeConfig,
        api_key: Option<&str>,
        pcm: &[u8],
        language: &str,
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        match config.asr_provider {
            AsrProvider::QwenApi => {
                self.transcribe_qwen(config, api_key, pcm, language, vocabulary)
                    .await
            }
            AsrProvider::OpenaiCompatible => {
                self.transcribe_openai(config, api_key, pcm, language, vocabulary)
                    .await
            }
            AsrProvider::Local => bail!("local ASR was routed to the API client"),
        }
    }

    async fn transcribe_qwen(
        &self,
        config: &RuntimeConfig,
        api_key: Option<&str>,
        pcm: &[u8],
        language: &str,
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        let wav = pcm_to_wav(pcm)?;
        let data_url = format!("data:audio/wav;base64,{}", STANDARD.encode(wav));
        if data_url.len() > 10 * 1024 * 1024 {
            bail!("recording exceeds Qwen ASR's 10 MB Data URL limit");
        }
        let mut messages = Vec::new();
        if !vocabulary.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": [{
                    "type": "text",
                    "text": format!("语音识别上下文与实体词表：{}", vocabulary.join("、"))
                }]
            }));
        }
        messages.push(json!({
            "role": "user",
            "content": [{
                "type": "input_audio",
                "input_audio": {"data": data_url}
            }]
        }));
        let mut asr_options = json!({"enable_itn": true});
        if language != "auto" {
            asr_options["language"] = Value::String(language.to_owned());
        }
        let body = json!({
            "model": config.asr_model,
            "messages": messages,
            "stream": false,
            "asr_options": asr_options
        });
        let url = format!(
            "{}/chat/completions",
            config.asr_base_url.trim_end_matches('/')
        );
        let response = self
            .authorized(self.client.post(url), api_key)
            .timeout(Duration::from_millis(config.asr_timeout_ms))
            .json(&body)
            .send()
            .await
            .context("Qwen ASR request failed")?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            bail!(
                "Qwen ASR endpoint returned {status}: {}",
                compact_error(&message)
            );
        }
        let parsed: ChatCompletionResponse =
            response.json().await.context("decode Qwen ASR response")?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .context("Qwen ASR endpoint returned no choices")?;
        let text = nonempty_text(choice.message.content, "Qwen ASR")?;
        let detected_language = choice
            .message
            .annotations
            .into_iter()
            .find_map(|annotation| {
                (!annotation.language.trim().is_empty()).then_some(annotation.language)
            })
            .unwrap_or_else(|| language.to_owned());
        Ok(TranscriptResult {
            text,
            language: detected_language,
        })
    }

    async fn transcribe_openai(
        &self,
        config: &RuntimeConfig,
        api_key: Option<&str>,
        pcm: &[u8],
        language: &str,
        vocabulary: &[String],
    ) -> Result<TranscriptResult> {
        let wav = pcm_to_wav(pcm)?;
        let file = multipart::Part::bytes(wav)
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .context("construct ASR audio upload")?;
        let mut form = multipart::Form::new()
            .text("model", config.asr_model.clone())
            .text("response_format", "json")
            .part("file", file);
        if language != "auto" {
            form = form.text("language", language.to_owned());
        }
        if !vocabulary.is_empty() {
            form = form.text("prompt", vocabulary.join(", "));
        }
        let url = format!(
            "{}/audio/transcriptions",
            config.asr_base_url.trim_end_matches('/')
        );
        let response = self
            .authorized(self.client.post(url), api_key)
            .timeout(Duration::from_millis(config.asr_timeout_ms))
            .multipart(form)
            .send()
            .await
            .context("OpenAI-compatible ASR request failed")?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            bail!(
                "OpenAI-compatible ASR endpoint returned {status}: {}",
                compact_error(&message)
            );
        }
        let parsed: TranscriptionResponse = response
            .json()
            .await
            .context("decode OpenAI-compatible ASR response")?;
        Ok(TranscriptResult {
            text: nonempty_text(parsed.text, "OpenAI-compatible ASR")?,
            language: if parsed.language.trim().is_empty() {
                language.to_owned()
            } else {
                parsed.language
            },
        })
    }

    fn authorized(
        &self,
        request: reqwest::RequestBuilder,
        api_key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        match api_key.filter(|key| !key.trim().is_empty()) {
            Some(api_key) => request.bearer_auth(api_key),
            None => request,
        }
    }
}

fn pcm_to_wav(pcm: &[u8]) -> Result<Vec<u8>> {
    let data_len = u32::try_from(pcm.len()).context("recording is too large for WAV")?;
    let riff_len = 36_u32
        .checked_add(data_len)
        .context("recording is too large for WAV")?;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_len.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&32_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

fn nonempty_text(value: String, provider: &str) -> Result<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{provider} returned no speech");
    }
    Ok(value)
}

fn compact_error(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread::JoinHandle,
        time::Duration,
    };

    use super::*;

    #[test]
    fn wraps_pcm_as_mono_16khz_wav() {
        let wav = pcm_to_wav(&[1, 2, 3, 4]).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            16_000
        );
        assert_eq!(&wav[44..], &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn qwen_provider_uses_audio_chat_contract() {
        let (address, request_rx, server) = serve_once(
            r#"{"choices":[{"message":{"content":"你好，世界。","annotations":[{"language":"zh"}]}}]}"#,
        );
        let mut config = RuntimeConfig {
            asr_provider: AsrProvider::QwenApi,
            asr_base_url: format!("http://{address}/v1"),
            asr_model: "qwen3-asr-flash".into(),
            ..RuntimeConfig::default()
        };
        config.asr_timeout_ms = 2_000;
        let output = AsrApiClient::new()
            .unwrap()
            .transcribe_qwen(
                &config,
                Some("asr-secret"),
                &[0, 0, 1, 0],
                "zh",
                &["Natsu Typeless".into()],
            )
            .await
            .unwrap();
        assert_eq!(output.text, "你好，世界。");
        assert_eq!(output.language, "zh");

        let request = request_rx.recv().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer asr-secret")
        );
        assert!(request.contains(r#""model":"qwen3-asr-flash""#));
        assert!(request.contains(r#""type":"input_audio""#));
        assert!(request.contains("data:audio/wav;base64,UklGR"));
        assert!(request.contains(r#""language":"zh""#));
        assert!(request.contains("Natsu Typeless"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn generic_provider_uses_multipart_transcriptions_contract() {
        let (address, request_rx, server) =
            serve_once(r#"{"text":"generic transcript","language":"en"}"#);
        let mut config = RuntimeConfig {
            asr_provider: AsrProvider::OpenaiCompatible,
            asr_base_url: format!("http://{address}/v1"),
            asr_model: "whisper-1".into(),
            ..RuntimeConfig::default()
        };
        config.asr_timeout_ms = 2_000;
        let output = AsrApiClient::new()
            .unwrap()
            .transcribe_openai(&config, None, &[0, 0, 1, 0], "en", &["Typeless".into()])
            .await
            .unwrap();
        assert_eq!(output.text, "generic transcript");
        assert_eq!(output.language, "en");

        let request = request_rx.recv().unwrap();
        assert!(request.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
        assert!(request.contains(r#"name="model""#));
        assert!(request.contains("whisper-1"));
        assert!(request.contains(r#"name="file"; filename="recording.wav""#));
        assert!(request.contains("audio/wav"));
        assert!(request.contains("RIFF"));
        assert!(request.contains(r#"name="prompt""#));
        assert!(request.contains("Typeless"));
        server.join().unwrap();
    }

    fn serve_once(response_body: &'static str) -> (String, mpsc::Receiver<String>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (request_tx, request_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let read = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..read]);
                let Some(header_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length: ")
                            .or_else(|| line.strip_prefix("Content-Length: "))
                    })
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                if request.len() >= header_end + content_length {
                    break;
                }
            }
            request_tx
                .send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });
        (address, request_rx, server)
    }
}
