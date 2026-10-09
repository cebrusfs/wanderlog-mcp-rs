//! Minimal proleptic-Gregorian helpers for Wanderlog's `YYYY-MM-DD` day strings.
//!
//! Only what day sections need (validate, add days, weekday); the conversions are Howard Hinnant's
//! public-domain `days_from_civil` / `civil_from_days` algorithms.

use anyhow::{Result, bail};

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Days since 1970-01-01 for a civil date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date for a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parse a strict `YYYY-MM-DD` date into a day number.
pub fn parse(s: &str) -> Result<i64> {
    let b = s.as_bytes();
    let digits_ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !digits_ok {
        bail!("invalid date {s:?}: expected YYYY-MM-DD");
    }
    let y: i64 = s[0..4].parse()?;
    let m: u32 = s[5..7].parse()?;
    let d: u32 = s[8..10].parse()?;
    let n = days_from_civil(y, m, d);
    if !(1..=12).contains(&m) || civil_from_days(n) != (y, m, d) {
        bail!("invalid date {s:?}: no such calendar day");
    }
    Ok(n)
}

pub fn format(n: i64) -> String {
    let (y, m, d) = civil_from_days(n);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `date` shifted by `days` (may be negative).
pub fn add(date: &str, days: i64) -> Result<String> {
    Ok(format(parse(date)? + days))
}

/// Short English weekday name ("Mon".."Sun").
pub fn weekday(date: &str) -> Result<&'static str> {
    // 1970-01-01 was a Thursday (index 3 in WEEKDAYS).
    Ok(WEEKDAYS[(parse(date)? + 3).rem_euclid(7) as usize])
}

#[cfg(test)]
#[path = "tests/dates.rs"]
mod tests;
