use anyhow::Result;
use natsu_typeless::{config::ProcessConfig, dbus, pipeline::Pipeline, worker::WorkerManager};
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
    let pipeline = Pipeline::new(process_config.runtime.clone(), worker.clone())?;
    let _connection = dbus::serve(pipeline).await?;

    tokio::spawn(async move {
        if let Err(error) = worker.warm().await {
            tracing::warn!(error = %error, "initial ASR model prewarm failed");
        } else {
            worker.schedule_idle_unload();
        }
    });

    tracing::info!("Natsu Typeless daemon is ready");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
