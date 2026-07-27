use anyhow::{Context, Result};

const SERVICE: &str = "io.github.ddy314.NatsuTypeless";
const ACCOUNT: &str = "openai-compatible-api-key";
const LEGACY_ACCOUNT: &str = "gemini-api-key";

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).context("initialize Secret Service entry")
}

pub fn get_cloud_api_key() -> Result<String> {
    for variable in ["OPENAI_API_KEY", "GEMINI_API_KEY"] {
        let Ok(value) = std::env::var(variable) else {
            continue;
        };
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }
    if let Ok(value) = entry(ACCOUNT)?.get_password() {
        return Ok(value);
    }
    entry(LEGACY_ACCOUNT)?
        .get_password()
        .context("cloud API key is not configured; run `natsu-typelessctl key set`")
}

pub fn set_cloud_api_key(value: &str) -> Result<()> {
    entry(ACCOUNT)?
        .set_password(value.trim())
        .context("store cloud API key in Secret Service")
}

pub fn clear_cloud_api_key() -> Result<()> {
    for account in [ACCOUNT, LEGACY_ACCOUNT] {
        if let Ok(entry) = entry(account) {
            let _ = entry.delete_credential();
        }
    }
    Ok(())
}

pub fn has_cloud_api_key() -> bool {
    get_cloud_api_key().is_ok()
}
