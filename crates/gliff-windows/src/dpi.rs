//! The result of asking Windows for per-monitor DPI awareness (v2).
//!
//! The Win32 call stays in `app`, before any window. A failure becomes one
//! log line. A success has no line.

use std::fmt::Display;

/// The client-log line for a failed `SetProcessDpiAwarenessContext`.
pub fn failure_line(error: &str) -> String {
    format!("dpi awareness: SetProcessDpiAwarenessContext failed: {error}")
}

/// Turns a `SetProcessDpiAwarenessContext` result into a log line.
///
/// `Ok(())` means the call succeeded, and there is nothing to write. `Err`
/// is the line to write. That line is not success. The process still starts.
pub fn report(result: Result<(), impl Display>) -> Result<(), String> {
    result.map_err(|error| failure_line(&error.to_string()))
}

#[cfg(test)]
mod tests;
