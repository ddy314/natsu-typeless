use std::sync::Arc;

use anyhow::{Context, Result};
use zbus::{Connection, fdo, interface, message::Header, object_server::SignalEmitter};

use crate::{
    BUS_NAME, OBJECT_PATH,
    pipeline::Pipeline,
    protocol::{RuntimeConfig, SessionOptions},
};

pub struct TypelessService {
    pipeline: Arc<Pipeline>,
    connection: Connection,
}

impl TypelessService {
    pub fn new(pipeline: Arc<Pipeline>, connection: Connection) -> Self {
        Self {
            pipeline,
            connection,
        }
    }
}

#[interface(name = "io.github.ddy314.NatsuTypeless", spawn = false)]
impl TypelessService {
    async fn configure(
        &self,
        config_json: &str,
        #[zbus(header)] _header: Header<'_>,
    ) -> fdo::Result<()> {
        let config: RuntimeConfig = serde_json::from_str(config_json).map_err(failed)?;
        self.pipeline.configure(config).await;
        Ok(())
    }

    async fn begin(
        &self,
        session_id: &str,
        options_json: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        let owner = caller(&header)?;
        let options: SessionOptions = serde_json::from_str(options_json).map_err(failed)?;
        self.pipeline
            .begin(owner, session_id.to_owned(), options)
            .await
            .map_err(failed)?;
        emit_state(&self.connection, session_id, "recording", "")
            .await
            .map_err(failed)?;

        let worker = Arc::clone(self.pipeline.worker());
        tokio::spawn(async move {
            if let Err(error) = worker.warm().await {
                tracing::warn!(error = %error, "ASR prewarm failed; finish will retry");
            }
        });
        Ok(())
    }

    async fn end(&self, session_id: &str, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let owner = caller(&header)?;
        let id = session_id.to_owned();
        let pipeline = Arc::clone(&self.pipeline);
        let connection = self.connection.clone();
        tokio::spawn(async move {
            let session = match pipeline.end(&owner, &id).await {
                Ok(session) => session,
                Err(error) => {
                    let _ = emit_state(&connection, &id, "error", &error_code(&error)).await;
                    return;
                }
            };
            let _ = emit_state(&connection, &id, "transcribing", "").await;
            let transcript = match pipeline.transcribe(&session).await {
                Ok(transcript) => transcript,
                Err(error) => {
                    let _ = emit_state(&connection, &id, "error", &error_code(&error)).await;
                    return;
                }
            };
            if !pipeline.mark_polishing(&id).await {
                return;
            }
            let _ = emit_state(&connection, &id, "polishing", "").await;
            if pipeline
                .polish_and_store(session, transcript)
                .await
                .is_none()
            {
                return;
            }
            let _ = emit_state(&connection, &id, "result_ready", "").await;
            if let Ok(emitter) = SignalEmitter::new(&connection, OBJECT_PATH) {
                let _ = TypelessService::result_ready(&emitter, &id).await;
            }
        });
        Ok(())
    }

    async fn cancel(
        &self,
        session_id: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        let owner = caller(&header)?;
        self.pipeline
            .cancel(&owner, session_id)
            .await
            .map_err(failed)?;
        emit_state(&self.connection, session_id, "idle", "")
            .await
            .map_err(failed)
    }

    #[zbus(out_args("state", "model_ready", "detail"))]
    async fn get_status(&self) -> (String, bool, String) {
        let (state, ready, detail) = self.pipeline.status().await;
        (state.as_str().into(), ready, detail)
    }

    #[zbus(out_args("text", "used_fallback", "timings_json"))]
    async fn take_result(
        &self,
        session_id: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<(String, bool, String)> {
        let owner = caller(&header)?;
        let result = self
            .pipeline
            .take_result(&owner, session_id)
            .await
            .map_err(failed)?;
        // Do not emit `idle` here. A D-Bus signal sent from inside this method
        // can be delivered before the method reply. The Fcitx client must keep
        // its session alive until it has consumed the reply containing text.
        // `Pipeline::take_result` has already moved the observable status to
        // Idle, and the client clears its local state after committing.
        Ok((result.text, result.used_fallback, result.timings_json))
    }

    #[zbus(signal)]
    async fn state_changed(
        emitter: &SignalEmitter<'_>,
        session_id: &str,
        state: &str,
        detail: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn result_ready(emitter: &SignalEmitter<'_>, session_id: &str) -> zbus::Result<()>;
}

pub async fn serve(pipeline: Arc<Pipeline>) -> Result<Connection> {
    let connection = Connection::session()
        .await
        .context("connect to session DBus")?;
    connection
        .object_server()
        .at(
            OBJECT_PATH,
            TypelessService::new(Arc::clone(&pipeline), connection.clone()),
        )
        .await
        .context("register DBus object")?;
    connection
        .request_name(BUS_NAME)
        .await
        .context("request Natsu Typeless DBus name")?;
    Ok(connection)
}

async fn emit_state(
    connection: &Connection,
    session_id: &str,
    state: &str,
    detail: &str,
) -> zbus::Result<()> {
    let emitter = SignalEmitter::new(connection, OBJECT_PATH)?;
    TypelessService::state_changed(&emitter, session_id, state, detail).await
}

fn caller(header: &Header<'_>) -> fdo::Result<String> {
    header
        .sender()
        .map(ToString::to_string)
        .ok_or_else(|| fdo::Error::AccessDenied("DBus caller has no unique name".into()))
}

fn failed(error: impl std::fmt::Display) -> fdo::Error {
    fdo::Error::Failed(error.to_string())
}

fn error_code(error: &anyhow::Error) -> String {
    let message = error.to_string().to_lowercase();
    if message.contains("no speech") || message.contains("audio") {
        "audio_failed"
    } else if message.contains("worker") || message.contains("asr") {
        "asr_failed"
    } else {
        "pipeline_failed"
    }
    .into()
}
