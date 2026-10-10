//! Opt-in timings of each tool call's stages on stderr, for finding where slow calls spend their
//! time. Enabled by `WANDERLOG_MCP_TIMING=1`; lines hold tool and stage names and durations only,
//! never trip keys, cookies or content.

use std::sync::OnceLock;
use std::time::Instant;

pub const ENV_VAR: &str = "WANDERLOG_MCP_TIMING";

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var(ENV_VAR).is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Measures one tool call; prints a single line when dropped, so early returns are covered.
pub struct Timing {
    tool: &'static str,
    start: Instant,
    last: Instant,
    stages: Vec<(&'static str, u128)>,
}

impl Timing {
    pub fn start(tool: &'static str) -> Self {
        let now = Instant::now();
        Self {
            tool,
            start: now,
            last: now,
            stages: Vec::new(),
        }
    }

    /// Close the stage that ran since the previous mark.
    pub fn stage(&mut self, name: &'static str) {
        let now = Instant::now();
        self.stages
            .push((name, now.duration_since(self.last).as_millis()));
        self.last = now;
    }

    fn line(&self) -> String {
        let mut line = format!("wanderlog-mcp timing: {}", self.tool);
        for (name, ms) in &self.stages {
            line.push_str(&format!(" {name}={ms}ms"));
        }
        line.push_str(&format!(" total={}ms", self.start.elapsed().as_millis()));
        line
    }
}

impl Drop for Timing {
    fn drop(&mut self) {
        if enabled() {
            eprintln!("{}", self.line());
        }
    }
}

#[cfg(test)]
#[path = "tests/timing.rs"]
mod tests;
