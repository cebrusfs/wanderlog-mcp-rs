//! Clock boundary for deterministic rate-limit tests.
use std::time::{Instant, SystemTime};
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn system_time(&self) -> SystemTime;
}
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
