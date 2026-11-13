use std::fmt;
use std::io;

/// The cause of a [`ParseError`].
#[derive(Debug)]
pub enum ParseCause {
    /// The input ended before a field was filled. Keeps the expected and
    /// actual byte counts available to callers that need to tell "nothing
    /// read yet" apart from a field truncated partway through.
    ShortRead { expected: u64, actual: u64 },
    /// Reading the input failed.
    Io(io::Error),
    /// The input is not a valid LZ4 frame stream.
    Message(String),
}

impl fmt::Display for ParseCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseCause::ShortRead { expected, actual } => {
                write!(f, "unexpected EOF: expected {expected} bytes, read {actual}")
            }
            ParseCause::Io(err) => write!(f, "{err}"),
            ParseCause::Message(msg) => write!(f, "{msg}"),
        }
    }
}

/// A parse failure tied to the byte offset and field path being read when the
/// failure happened, e.g.:
///
/// ```text
/// parse error at byte 27 (data_blocks[1].data): unexpected EOF: expected 1024 bytes, read 800
/// ```
#[derive(Debug)]
pub struct ParseError {
    pub offset: u64,
    pub path: String,
    pub cause: ParseCause,
}

impl ParseError {
    /// Builds a `ParseError` from a byte offset, field path, and cause.
    pub fn new(offset: u64, path: impl Into<String>, cause: ParseCause) -> ParseError {
        ParseError { offset, path: path.into(), cause }
    }

    /// Builds a `ParseError` whose cause is a message.
    pub fn msg(offset: u64, path: impl Into<String>, message: impl Into<String>) -> ParseError {
        ParseError::new(offset, path, ParseCause::Message(message.into()))
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at byte {} ({}): {}", self.offset, self.path, self.cause)
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            ParseCause::Io(err) => Some(err),
            _ => None,
        }
    }
}

/// A violation of the restore schema, tied to the JSON path of the offending
/// value, e.g.:
///
/// ```text
/// invalid restore JSON ($.frames[0].data_blocks): missing required field
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct SchemaError {
    pub path: String,
    pub message: String,
}

impl SchemaError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> SchemaError {
        SchemaError { path: path.into(), message: message.into() }
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid restore JSON ({}): {}", self.path, self.message)
    }
}

impl std::error::Error for SchemaError {}

/// An error from [`restore`](super::restore).
#[derive(Debug)]
pub enum RestoreError {
    /// The document is not a valid restore document.
    Schema(SchemaError),
    /// Writing the output failed.
    Io(io::Error),
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RestoreError::Schema(err) => err.fmt(f),
            RestoreError::Io(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for RestoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RestoreError::Schema(err) => Some(err),
            RestoreError::Io(err) => Some(err),
        }
    }
}

impl From<SchemaError> for RestoreError {
    fn from(err: SchemaError) -> RestoreError {
        RestoreError::Schema(err)
    }
}

/// Quotes `s` with double quotes and backslash escapes for the strings that
/// appear in error messages.
pub(super) fn quote_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests;
