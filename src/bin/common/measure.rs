//! Helpers shared by the measurement tools for labeling inputs and rates.

use std::path::Path;
use std::time::Duration;

/// Returns the file name of `path`, or `path` itself if it has none.
pub fn input_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Returns the nanoseconds per input byte, or zero for empty input.
// lz77stats reports seconds only.
#[allow(dead_code)]
pub fn ns_per_byte(elapsed: Duration, bytes: usize) -> f64 {
    if bytes == 0 {
        0.0
    } else {
        elapsed.as_nanos() as f64 / bytes as f64
    }
}
