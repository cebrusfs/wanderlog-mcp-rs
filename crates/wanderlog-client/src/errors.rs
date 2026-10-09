//! Typed errors carried by compatibility-preserving anyhow::Result APIs.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionConflict {
    pub expected: u64,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationFailed {
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
