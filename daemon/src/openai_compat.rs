use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::{
    prompt::{DICTATION_PROMPT, apply_vocabulary_corrections},
    protocol::SessionOptions,
};

#[derive(Clone)]
pub struct OpenAiCompatClient {
    client: Client,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: AssistantMessage,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: MessageContent,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Deserialize)]
struct ContentPart {
    #[serde(default)]
    text: String,
}

impl MessageContent {
    fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Parts(parts) => parts
                .into_iter()
                .map(|part| part.text)
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl OpenAiCompatClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("natsu-typeless/", env!("CARGO_PKG_VERSION")))
            .http2_adaptive_window(true)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .context("build OpenAI-compatible HTTP client")?;
        Ok(Self { client })
    }

    pub async fn polish(
        &self,
        api_key: Option<&str>,
        base_url: &str,
        model: &str,
        timeout_ms: u64,
        raw: &str,
        options: &SessionOptions,
    ) -> Result<String> {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let corrected_transcription = apply_vocabulary_corrections(raw, &options.vocabulary);
        let explicit_corrections = options
            .vocabulary
            .iter()
            .filter(|entry| entry.contains("=>") || entry.contains("->"))
            .collect::<Vec<_>>();
        let exact_vocabulary = options
            .vocabulary
            .iter()
            .filter(|entry| {
                !entry.starts_with('@') && !entry.contains("=>") && !entry.contains("->")
            })
            .collect::<Vec<_>>();
        let input = json!({
            "transcription": corrected_transcription,
            "detected_or_forced_language": options.language,
            "exact_vocabulary": exact_vocabulary,
            "explicit_corrections": explicit_corrections,
            "context_before": options.context_before,
            "context_after": options.context_after,
        })
        .to_string();
        let max_tokens = (raw.chars().count().saturating_mul(2) + 64).clamp(128, 2_048);
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": DICTATION_PROMPT},
                {"role": "user", "content": input}
            ],
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "stream": false
        });

        let mut request = self
            .client
            .post(url)
            .timeout(Duration::from_millis(timeout_ms))
            .json(&body);
        if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .context("OpenAI-compatible request failed")?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            bail!(
                "OpenAI-compatible endpoint returned {status}: {}",
                compact_error(&message)
            );
        }
        let parsed: ChatCompletionResponse = response
            .json()
            .await
            .context("decode OpenAI-compatible response")?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content.into_text())
            .map(|text| apply_vocabulary_corrections(&text, &options.vocabulary))
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .context("OpenAI-compatible endpoint returned no text")?;
        Ok(text)
    }
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
        time::Duration,
    };

    use super::*;

    #[tokio::test]
    async fn uses_chat_completions_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
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
                .send(String::from_utf8(request).unwrap())
                .unwrap();
            let body = r#"{"choices":[{"message":{"content":"我们周四发布。"}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let client = OpenAiCompatClient::new().unwrap();
        let options = SessionOptions {
            vocabulary: vec!["@Claude".into(), "Fable".into(), "MesoS => Mythos".into()],
            ..SessionOptions::default()
        };
        let output = client
            .polish(
                Some("test-secret"),
                &format!("http://{address}/v1"),
                "test/model",
                2_000,
                "我们周三发布，不对，是周四发布",
                &options,
            )
            .await
            .unwrap();
        assert_eq!(output, "我们周四发布。");

        let request = request_rx.recv().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-secret")
        );
        assert!(request.contains(r#""model":"test/model""#));
        assert!(request.contains(r#""role":"system""#));
        assert!(request.contains(r#""max_tokens":128"#));
        assert!(request.contains(r#""temperature":0.0"#));
        assert!(request.contains("Do not stop after merely adding punctuation"));
        assert!(request.contains(r#""stream":false"#));
        let body = request.split_once("\r\n\r\n").unwrap().1;
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        let input = body["messages"][1]["content"].as_str().unwrap();
        let input: serde_json::Value = serde_json::from_str(input).unwrap();
        assert_eq!(input["exact_vocabulary"], json!(["Fable"]));
        assert_eq!(input["explicit_corrections"], json!(["MesoS => Mythos"]));
        assert!(!input.to_string().contains("@Claude"));
        server.join().unwrap();
    }
}
