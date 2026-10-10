//! Typed errors carried by compatibility-preserving anyhow::Result APIs.
/// A write whose result was not confirmed: it may or may not have been applied. Holds the
/// redacted reason.
#[derive(Debug)]
pub struct OutcomeUnknown(pub String);
impl std::fmt::Display for OutcomeUnknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}. The edit may or may not have been applied: re-read the trip (or list trips after creation) before retrying",
            self.0
        )
    }
}
impl std::error::Error for OutcomeUnknown {}
/// The trip moved past the revision the caller built on; nothing was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionConflict {
    /// The revision the caller expected.
    pub expected: u64,
    /// The trip's current revision.
    pub actual: u64,
}
impl std::fmt::Display for RevisionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "trip changed since revision {} (now {}); nothing applied. Read it again and confirm the new result",
            self.expected, self.actual
        )
    }
}
impl std::error::Error for RevisionConflict {}
/// Wanderlog rejected the session (HTTP 401 or 403); it has probably expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationFailed {
    /// The HTTP status code.
    pub status: u16,
}
impl std::fmt::Display for AuthenticationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "not authorised (HTTP {}); the Wanderlog session may have expired; supply a fresh session",
            self.status
        )
    }
}
impl std::error::Error for AuthenticationFailed {}
