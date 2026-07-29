use anyhow::Result;
use natsu_typeless::{
    asr::AsrRouter, config::ProcessConfig, dbus, pipeline::Pipeline, worker::WorkerManager,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("natsu_typeless=info")),
        )
        .with_target(false)
        .compact()
        .init();

    let process_config = ProcessConfig::default();
    let worker = WorkerManager::new(process_config.clone());
    let asr = AsrRouter::new(worker)?;
    let pipeline = Pipeline::new(process_config.runtime.clone(), asr)?;
    let _connection = dbus::serve(pipeline).await?;

    tracing::info!("Natsu Typeless daemon is ready; ASR loads on first use");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
