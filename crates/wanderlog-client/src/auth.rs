//! Cookie validation only; no persistence.

use anyhow::{Result, bail};

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
