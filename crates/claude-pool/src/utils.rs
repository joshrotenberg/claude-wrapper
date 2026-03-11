//! Shared utilities used across the claude-pool crate.

/// Generate a short unique ID based on the current timestamp in nanoseconds.
///
/// Returns a hex-encoded string derived from the number of nanoseconds elapsed
/// since the Unix epoch. Suitable for generating task, chain, and message IDs.
pub(crate) fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}")
}
