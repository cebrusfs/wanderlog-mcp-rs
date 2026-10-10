//! Application credentials; no plaintext persistent fallback outside macOS.
use anyhow::{Result, anyhow, bail};
pub use wanderlog_client::session::normalize;
/// Environment variable that injects a session instead of the stored one.
pub const ENV_VAR: &str = "WANDERLOG_COOKIE";
/// Where the session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The [`ENV_VAR`] environment variable.
    Env,
    /// The OS credential store.
    Keychain,
}
/// Persistent session storage; tests substitute an in-memory store.
pub trait CredentialStore {
    /// The stored session, if any.
    fn load(&self) -> Result<Option<String>>;
    /// Replace the stored session.
    fn store(&self, value: &str) -> Result<()>;
    /// Delete the stored session; `false` if there was none.
    fn clear(&self) -> Result<bool>;
}
/// The session from `raw` (the environment) if set, else from `store`, normalized.
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
/// The macOS Keychain; elsewhere an empty store that refuses to save.
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
/// The session from [`ENV_VAR`] or the OS credential store.
pub fn load() -> Result<(String, Source)> {
    load_with(std::env::var(ENV_VAR).ok().as_deref(), &OsCredentialStore)
}
/// Normalize and save a session in the OS credential store.
pub fn store(value: &str) -> Result<()> {
    OsCredentialStore.store(&normalize(value)?)
}
/// Delete the stored session; `false` if there was none.
pub fn clear() -> Result<bool> {
    OsCredentialStore.clear()
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
