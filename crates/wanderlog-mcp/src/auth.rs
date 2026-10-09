//! Application credentials; no plaintext persistent fallback outside macOS.
use anyhow::{Result, anyhow, bail};
pub use wanderlog_client::session::normalize;
pub const ENV_VAR: &str = "WANDERLOG_COOKIE";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Keychain,
}
pub trait CredentialStore {
    fn load(&self) -> Result<Option<String>>;
    fn store(&self, value: &str) -> Result<()>;
    fn clear(&self) -> Result<bool>;
}
pub fn load_with(raw: Option<&str>, store: &dyn CredentialStore) -> Result<(String, Source)> {
    if let Some(raw) = raw.filter(|s| !s.trim().is_empty()) {
        return Ok((normalize(raw)?, Source::Env));
    }
    match store.load().map_err(|_| {
        anyhow!("cannot read the OS credential store; set {ENV_VAR} to inject a session")
    })? {
        Some(raw) => Ok((normalize(&raw)?, Source::Keychain)),
        None => bail!(
            "no Wanderlog session stored: run `wanderlog-mcp auth login`, supply a cookie with `auth set`, or set {ENV_VAR}"
        ),
    }
}
pub struct OsCredentialStore;
#[cfg(target_os = "macos")]
fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new("wanderlog-mcp", "connect.sid")
        .map_err(|_| anyhow!("cannot open the OS credential store"))
}
#[cfg(target_os = "macos")]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<String>> {
        match entry()?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => bail!("cannot read the OS credential store"),
        }
    }
    fn store(&self, v: &str) -> Result<()> {
        entry()?
            .set_password(v)
            .map_err(|_| anyhow!("cannot save the session to the OS credential store"))
    }
    fn clear(&self) -> Result<bool> {
        match entry()?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(_) => bail!("cannot delete the stored session"),
        }
    }
}
#[cfg(not(target_os = "macos"))]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn store(&self, _: &str) -> Result<()> {
        bail!("persistent credential storage is supported on macOS; set {ENV_VAR} on this platform")
    }
    fn clear(&self) -> Result<bool> {
        Ok(false)
    }
}
pub fn load() -> Result<(String, Source)> {
    load_with(std::env::var(ENV_VAR).ok().as_deref(), &OsCredentialStore)
}
pub fn store(value: &str) -> Result<()> {
    OsCredentialStore.store(&normalize(value)?)
}
pub fn clear() -> Result<bool> {
    OsCredentialStore.clear()
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
