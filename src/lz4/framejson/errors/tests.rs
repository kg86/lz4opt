use super::*;

#[test]
fn parse_error_message() {
    let err = ParseError::msg(27, "data_blocks[1].data", "unexpected EOF: expected 1024 bytes, read 800");

    const WANT: &str = "parse error at byte 27 (data_blocks[1].data): unexpected EOF: expected 1024 bytes, read 800";
    assert_eq!(err.to_string(), WANT, "to_string()");
}

#[test]
fn parse_error_source() {
    // The wrapped cause is reachable through the source chain.
    let cause = io::Error::new(io::ErrorKind::UnexpectedEof, "truncated");
    let err = ParseError::new(5, "magic_number", ParseCause::Io(cause));

    let source = std::error::Error::source(&err).expect("source() = None, want the wrapped cause");
    let io_err = source.downcast_ref::<io::Error>().expect("source() is not an io::Error");
    assert_eq!(io_err.kind(), io::ErrorKind::UnexpectedEof, "source kind");
    assert_eq!(io_err.to_string(), "truncated", "source message");
}

#[test]
fn parse_error_formatted_message() {
    // ParseError::msg takes a formatted message as the cause.
    let err = ParseError::msg(10, "flg", format!("unsupported version {}", 3));

    const WANT: &str = "parse error at byte 10 (flg): unsupported version 3";
    assert_eq!(err.to_string(), WANT, "to_string()");
}
