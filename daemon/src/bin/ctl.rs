use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use natsu_typeless::{
    BUS_NAME, INTERFACE_NAME, OBJECT_PATH,
    config::{ProcessConfig, data_home, worker_source},
    secrets,
};
use zbus::{Connection, Proxy};

#[derive(Parser)]
#[command(version, about = "Configure and diagnose Natsu Typeless")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Status,
    Doctor,
    Setup,
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    AsrKey {
        #[command(subcommand)]
        command: AsrKeyCommand,
    },
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
}

#[derive(Subcommand)]
enum KeyCommand {
    Set,
    Clear,
    Status,
}

#[derive(Subcommand)]
enum AsrKeyCommand {
    Set,
    Clear,
    Status,
}

#[derive(Subcommand)]
enum ModelCommand {
    Install,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Status => print_status().await,
        Commands::Doctor => doctor().await,
        Commands::Setup => setup(),
        Commands::Key { command } => key(command),
        Commands::AsrKey { command } => asr_key(command),
        Commands::Model { command } => model(command),
    }
}

fn asr_key(command: AsrKeyCommand) -> Result<()> {
    match command {
        AsrKeyCommand::Set => {
            let value = rpassword::prompt_password("ASR API key: ")?;
            if value.trim().is_empty() {
                bail!("key was empty");
            }
            secrets::set_asr_api_key(&value)?;
            println!("ASR API key stored in Secret Service.");
        }
        AsrKeyCommand::Clear => {
            secrets::clear_asr_api_key()?;
            println!("ASR API key removed.");
        }
        AsrKeyCommand::Status => {
            println!(
                "ASR API key: {}",
                if secrets::has_asr_api_key() {
                    "configured"
                } else {
                    "missing"
                }
            );
        }
    }
    Ok(())
}

fn key(command: KeyCommand) -> Result<()> {
    match command {
        KeyCommand::Set => {
            let value = rpassword::prompt_password("OpenAI-compatible API key: ")?;
            if value.trim().is_empty() {
                bail!("key was empty");
            }
            secrets::set_cloud_api_key(&value)?;
            println!("Cloud API key stored in Secret Service.");
        }
        KeyCommand::Clear => {
            secrets::clear_cloud_api_key()?;
            println!("Cloud API key removed.");
        }
        KeyCommand::Status => {
            println!(
                "Cloud API key: {}",
                if secrets::has_cloud_api_key() {
                    "configured"
                } else {
                    "missing"
                }
            );
        }
    }
    Ok(())
}

fn model(command: ModelCommand) -> Result<()> {
    match command {
        ModelCommand::Install => {
            let config = ProcessConfig::default();
            let status = Command::new(&config.worker_python)
                .args(["-m", "natsu_typeless_asr.setup"])
                .env("NATSU_TYPELESS_MODEL", config.model_id)
                .env("NATSU_TYPELESS_MODEL_REVISION", config.model_revision)
                .status()
                .context("start model installer")?;
            if !status.success() {
                bail!("model installer failed with {status}");
            }
        }
    }
    Ok(())
}

fn setup() -> Result<()> {
    let source = worker_source();
    if !source.join("pyproject.toml").exists() {
        bail!(
            "ASR worker source not found at {}; set NATSU_TYPELESS_WORKER_SOURCE",
            source.display()
        );
    }
    let root = data_home().join("natsu-typeless");
    let venv = root.join("venv");
    std::fs::create_dir_all(&root).context("create Natsu Typeless data directory")?;
    run(
        Command::new("uv")
            .args(["venv", "--python", "3.12"])
            .arg(&venv),
        "create ASR virtual environment",
    )?;
    run(
        Command::new("uv")
            .args(["sync", "--project"])
            .arg(&source)
            .args(["--locked", "--no-dev", "--python", "3.12"])
            .env("UV_PROJECT_ENVIRONMENT", &venv),
        "install ASR worker dependencies",
    )?;
    let python = venv.join("bin/python");
    run(
        Command::new(&python).args(["-m", "natsu_typeless_asr.setup"]),
        "download the pinned Qwen3-ASR model",
    )?;
    println!("ASR runtime installed at {}", venv.display());
    println!("Restart natsu-typeless.service or fcitx5 before testing.");
    Ok(())
}

fn run(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("{description}: failed to start"))?;
    if !status.success() {
        bail!("{description}: exited with {status}");
    }
    Ok(())
}

async fn print_status() -> Result<()> {
    let connection = Connection::session().await?;
    let proxy = Proxy::new(&connection, BUS_NAME, OBJECT_PATH, INTERFACE_NAME).await?;
    let (state, ready, detail): (String, bool, String) = proxy.call("GetStatus", &()).await?;
    println!("state: {state}");
    println!("ASR ready: {ready}");
    if !detail.is_empty() {
        println!("detail: {detail}");
    }
    Ok(())
}

async fn doctor() -> Result<()> {
    let mut failed = false;
    check_command("fcitx5", &mut failed);
    check_command("pw-record", &mut failed);
    let config = ProcessConfig::default();
    check_path_or_command(&config.worker_python, "ASR Python", &mut failed);
    println!(
        "[{}] cloud API key",
        if secrets::has_cloud_api_key() {
            "ok"
        } else {
            "missing"
        }
    );
    if !secrets::has_cloud_api_key() {
        failed = true;
    }
    println!(
        "[{}] ASR API key (only required for remote ASR)",
        if secrets::has_asr_api_key() {
            "ok"
        } else {
            "optional"
        }
    );
    match print_status().await {
        Ok(()) => println!("[ok] daemon DBus"),
        Err(error) => {
            println!("[fail] daemon DBus: {error}");
            failed = true;
        }
    }
    if failed {
        bail!("one or more checks failed");
    }
    println!("All required checks passed.");
    Ok(())
}

fn check_command(command: &str, failed: &mut bool) {
    let ok = Command::new("sh")
        .args(["-c", "command -v \"$1\" >/dev/null 2>&1", "sh", command])
        .status()
        .is_ok_and(|status| status.success());
    println!("[{}] {command}", if ok { "ok" } else { "fail" });
    *failed |= !ok;
}

fn check_path_or_command(path: &Path, label: &str, failed: &mut bool) {
    let ok = path.exists()
        || Command::new("sh")
            .args([
                "-c",
                "command -v \"$1\" >/dev/null 2>&1",
                "sh",
                &path.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success());
    println!("[{}] {label}", if ok { "ok" } else { "fail" });
    *failed |= !ok;
}
