//! Where the Wanderlog session cookie (`connect.sid`) lives.
//!
//! Lookup order: the `WANDERLOG_COOKIE` environment variable (handy for CI or other OSes), then
//! the OS credential store (Keychain on macOS). The value is a full login session: it is never
//! printed, logged or passed to models.

use anyhow::{Context, Result, bail};

pub const ENV_VAR: &str = "WANDERLOG_COOKIE";
const SERVICE: &str = "wanderlog-mcp";
const ACCOUNT: &str = "connect.sid";

/// Where a cookie was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Keychain,
}

/// Accept `connect.sid=<value>`, a bare value, or a pasted `Set-Cookie`-style line.
pub fn normalize(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let value = raw.strip_prefix("connect.sid=").unwrap_or(raw);
    let value = value.split(';').next().unwrap_or_default().trim();
    if value.is_empty() {
        bail!("empty cookie value");
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_graphic() && b != b',' && b != b'"' && b != b'\\')
    {
        bail!("cookie value contains characters a cookie cannot hold");
    }
    Ok(value.to_owned())
}

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).context("cannot open the OS credential store")
}

pub fn load() -> Result<(String, Source)> {
    if let Ok(raw) = std::env::var(ENV_VAR)
        && !raw.trim().is_empty()
    {
        return Ok((normalize(&raw).context(ENV_VAR)?, Source::Env));
    }
    match entry()?.get_password() {
        Ok(raw) => Ok((normalize(&raw)?, Source::Keychain)),
        Err(keyring::Error::NoEntry) => {
            bail!(
                "no Wanderlog session stored: run `wanderlog-mcp auth login`, supply a cookie with `auth set`, or set {ENV_VAR}"
            )
        }
        Err(e) => Err(e).context("cannot read the Wanderlog session from the OS credential store"),
    }
}

pub fn store(value: &str) -> Result<()> {
    entry()?
        .set_password(value)
        .context("cannot save the session to the OS credential store")
}

/// Remove the stored cookie; `Ok(false)` when there was none.
pub fn clear() -> Result<bool> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e).context("cannot delete the stored session"),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalizes_pasted_forms() {
        assert_eq!(normalize(" s%3Aabc.def ").unwrap(), "s%3Aabc.def");
        assert_eq!(normalize("connect.sid=s%3Aabc.def").unwrap(), "s%3Aabc.def");
        assert_eq!(
            normalize("connect.sid=s%3Aabc; Path=/; HttpOnly").unwrap(),
            "s%3Aabc"
        );
        assert!(normalize("").is_err());
        assert!(normalize("a b").is_err());
    }
}
