//! Clock boundary for deterministic rate-limit tests.
use std::time::{Instant, SystemTime};
/// Time source for rate-limit cooldowns; inject one with [`crate::rest::Rest::with_clock`].
pub trait Clock: Send + Sync {
    /// Monotonic time, for measuring cooldowns.
    fn now(&self) -> Instant;
    /// Wall-clock time, for `Retry-After` HTTP dates.
    fn system_time(&self) -> SystemTime;
}
/// The real clocks.
#[derive(Debug, Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn system_time(&self) -> SystemTime {
        SystemTime::now()
    }
}
