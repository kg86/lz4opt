use std::io::{self, Read};

/// Reads as many bytes as it can to fill `buf`. Returns how many were read
/// and a read error other than reaching the end of input, which is not an
/// error here: the caller tells a short read apart by the count.
pub(crate) fn read_full(r: &mut impl Read, buf: &mut [u8]) -> (usize, Option<io::Error>) {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return (n, Some(err)),
        }
    }
    (n, None)
}
