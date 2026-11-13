//! Shared diagnostics and exit codes for command-line programs.

use std::fmt::Display;
use std::io::{self, Write};

pub const USAGE_ERROR: i32 = 2;
pub const RUNTIME_ERROR: i32 = 1;

/// Writes a diagnostic and returns the requested exit code.
pub fn report(stderr: &mut dyn Write, message: impl Display, code: i32) -> i32 {
    let _ = writeln!(stderr, "{message}");
    code
}

/// Reports an execution failure and exits with code 1.
// lz4frame-json uses report directly so it can inject stderr in tests.
#[allow(dead_code)]
pub fn fail(message: impl Display) -> ! {
    std::process::exit(report(&mut io::stderr().lock(), message, RUNTIME_ERROR));
}

/// Reports invalid arguments and exits with code 2.
// lz4frame-json uses report directly so it can inject stderr in tests.
#[allow(dead_code)]
pub fn usage(message: impl Display) -> ! {
    std::process::exit(report(&mut io::stderr().lock(), message, USAGE_ERROR));
}
