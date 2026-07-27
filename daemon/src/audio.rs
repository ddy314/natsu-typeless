use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::JoinHandle,
    time::timeout,
};

const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u32 = 1;
const BYTES_PER_SAMPLE: usize = 2;

pub struct Recording {
    child: Child,
    reader: JoinHandle<Result<Vec<u8>>>,
    started: Instant,
}

impl Recording {
    pub async fn start(max_seconds: u64) -> Result<Self> {
        let max_bytes =
            SAMPLE_RATE as usize * CHANNELS as usize * BYTES_PER_SAMPLE * max_seconds as usize;
        let mut child = Command::new("pw-record")
            .args([
                "--raw",
                "--format",
                "s16",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--latency",
                "50ms",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start pw-record; PipeWire tools are required")?;

        let mut stdout = child
            .stdout
            .take()
            .context("pw-record stdout was not available")?;
        let reader = tokio::spawn(async move {
            let mut pcm = Vec::with_capacity((SAMPLE_RATE as usize * 10).min(max_bytes));
            let mut chunk = [0_u8; 8192];
            loop {
                let read = stdout
                    .read(&mut chunk)
                    .await
                    .context("read pw-record PCM")?;
                if read == 0 {
                    break;
                }
                if pcm.len() + read > max_bytes {
                    bail!("recording exceeded the configured maximum duration");
                }
                pcm.extend_from_slice(&chunk[..read]);
            }
            Ok(pcm)
        });

        Ok(Self {
            child,
            reader,
            started: Instant::now(),
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub async fn finish(mut self) -> Result<Vec<u8>> {
        self.interrupt();
        let _ = timeout(Duration::from_secs(2), self.child.wait()).await;
        let pcm = timeout(Duration::from_secs(2), self.reader)
            .await
            .context("timed out draining PipeWire audio")?
            .context("audio reader task failed")??;
        if pcm.len() < SAMPLE_RATE as usize * BYTES_PER_SAMPLE / 10 {
            bail!("no speech audio was captured");
        }
        Ok(pcm)
    }

    pub async fn cancel(mut self) {
        self.interrupt();
        let _ = timeout(Duration::from_secs(1), self.child.wait()).await;
        self.reader.abort();
    }

    fn interrupt(&mut self) {
        if let Some(pid) = self.child.id() {
            // SAFETY: kill only receives the pid returned for our direct child process.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGINT);
            }
        }
    }
}
