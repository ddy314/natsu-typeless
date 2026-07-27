use std::{env, path::PathBuf};

use crate::protocol::RuntimeConfig;

pub const MODEL_ID: &str = "Qwen/Qwen3-ASR-0.6B-hf";
pub const MODEL_REVISION: &str = "7f1569a48a89f3e3f4dc3a5c9d28bddd903bc76c";

#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub runtime: RuntimeConfig,
    pub model_id: String,
    pub model_revision: String,
    pub worker_python: PathBuf,
    pub worker_module: String,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        let worker_python = env::var_os("NATSU_TYPELESS_WORKER_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let managed = data_home()
                    .join("natsu-typeless")
                    .join("venv")
                    .join("bin")
                    .join("python");
                if managed.exists() {
                    managed
                } else {
                    let development = PathBuf::from("worker/.venv/bin/python");
                    if development.exists() {
                        development
                    } else {
                        PathBuf::from("python3")
                    }
                }
            });
        Self {
            runtime: RuntimeConfig::default(),
            model_id: env::var("NATSU_TYPELESS_MODEL").unwrap_or_else(|_| MODEL_ID.into()),
            model_revision: env::var("NATSU_TYPELESS_MODEL_REVISION")
                .unwrap_or_else(|_| MODEL_REVISION.into()),
            worker_python,
            worker_module: env::var("NATSU_TYPELESS_WORKER_MODULE")
                .unwrap_or_else(|_| "natsu_typeless_asr.worker".into()),
        }
    }
}

pub fn data_home() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/share"))
}

pub fn worker_source() -> PathBuf {
    if let Some(path) = env::var_os("NATSU_TYPELESS_WORKER_SOURCE") {
        return PathBuf::from(path);
    }
    let development = PathBuf::from("worker");
    if development.join("pyproject.toml").exists() {
        development
    } else {
        PathBuf::from("/usr/share/natsu-typeless/worker")
    }
}
