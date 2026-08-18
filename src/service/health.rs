//! TCP health-check probes for container services.
//!
//! Provides duration parsing, single TCP probes, and retry-based
//! health-check loops.  All I/O is synchronous — no async runtime.

use std::time::Duration;

use crate::{config::HealthCheck, error::ForgeError};

// -------------------------------------------------------------
// Duration parsing
// -------------------------------------------------------------

/// Parse a human-readable duration string into a [`Duration`].
///
/// Supported formats: `"2s"` (seconds), `"500ms"` (milliseconds).
///
/// # Errors
///
/// Returns [`ForgeError::Config`] if the format is unrecognised or
/// the numeric portion cannot be parsed.
pub fn parse_duration(s: &str) -> Result<Duration, ForgeError> {
    if let Some(ms) = s.strip_suffix("ms") {
        let val: u64 = ms
            .parse()
            .map_err(|e| ForgeError::Config(format!("invalid duration '{s}': {e}")))?;
        return bounded(Duration::from_millis(val), s);
    }
    if let Some(secs) = s.strip_suffix('s') {
        let val: u64 = secs
            .parse()
            .map_err(|e| ForgeError::Config(format!("invalid duration '{s}': {e}")))?;
        return bounded(Duration::from_secs(val), s);
    }
    Err(ForgeError::Config(format!(
        "unsupported duration format '{s}' (expected '2s' or '500ms')"
    )))
}

/// Upper bound for a parsed duration (24 hours).
///
/// Callers add the result to [`std::time::Instant`], which panics on overflow.
/// Validation rejects oversized values first; this is the backstop for any path
/// that reaches the parser without having gone through it.
const MAX_DURATION: Duration = Duration::from_secs(86_400);

/// Reject a duration beyond [`MAX_DURATION`].
fn bounded(value: Duration, raw: &str) -> Result<Duration, ForgeError> {
    if value > MAX_DURATION {
        return Err(ForgeError::Config(format!(
            "duration '{raw}' exceeds the maximum of {}s",
            MAX_DURATION.as_secs()
        )));
    }
    Ok(value)
}

// -------------------------------------------------------------
// TCP probe
// -------------------------------------------------------------

/// Attempt a single TCP connection to `addr:port` with a timeout.
///
/// Returns `true` if the connection succeeds, `false` otherwise.
pub fn tcp_probe(addr: &str, port: u16, timeout: Duration) -> bool {
    let target = format!("{addr}:{port}");
    let socket_addr: std::net::SocketAddr = match target.parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    std::net::TcpStream::connect_timeout(&socket_addr, timeout).is_ok()
}

// -------------------------------------------------------------
// Retry loop
// -------------------------------------------------------------

/// Block until a TCP health check passes or all retries are exhausted.
///
/// Parses interval and timeout from the [`HealthCheck`] config, then
/// probes up to `retries` times with a sleep between attempts.
///
/// Returns `Ok(true)` if healthy, `Ok(false)` if all retries fail.
///
/// # Errors
///
/// Returns [`ForgeError`] if the interval or timeout strings are
/// invalid.
pub fn wait_for_healthy(addr: &str, port: u16, check: &HealthCheck) -> Result<bool, ForgeError> {
    let interval = parse_duration(&check.interval)?;
    let timeout = parse_duration(&check.timeout)?;
    for _ in 0..check.retries {
        if tcp_probe(addr, port, timeout) {
            return Ok(true);
        }
        sleep_interval(interval);
    }
    Ok(false)
}

/// Sleep for the given duration (synchronous).
fn sleep_interval(dur: Duration) {
    std::thread::sleep(dur);
}

#[cfg(test)]
mod bound_tests {
    use super::*;

    #[test]
    fn parse_duration_rejects_values_that_would_overflow_instant() {
        // Instant::now() + Duration::from_secs(u64::MAX) panics inside std, so
        // the parser must refuse the value rather than hand it to a caller.
        let result = parse_duration(&format!("{}s", u64::MAX));
        assert!(
            result.is_err(),
            "u64::MAX seconds must be rejected, not returned"
        );
    }

    #[test]
    fn parse_duration_accepts_a_full_day() {
        let result = parse_duration("86400s");
        assert!(result.is_ok(), "24h is within the supported range");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_seconds() {
        let d = parse_duration("2s").unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        assert_eq!(d, Duration::from_secs(2), "should parse 2 seconds");
    }

    #[test]
    fn parse_duration_millis() {
        let d = parse_duration("500ms").unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        assert_eq!(
            d,
            Duration::from_millis(500),
            "should parse 500 milliseconds"
        );
    }

    #[test]
    fn parse_duration_invalid() {
        let result = parse_duration("abc");
        assert!(result.is_err(), "should reject invalid duration string");
    }

    #[test]
    fn tcp_probe_unreachable() {
        let result = tcp_probe("127.0.0.1", 59_999, Duration::from_millis(50));
        assert!(!result, "probe should fail on unreachable port");
    }
}
