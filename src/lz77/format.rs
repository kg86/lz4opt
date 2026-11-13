use std::fmt;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

use miniz_oxide::deflate::core::{
    compress, create_comp_flags_from_zip_params, CompressionStrategy, CompressorOxide, TDEFLFlush, TDEFLStatus,
};
use miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
use miniz_oxide::inflate::core::{decompress, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

use super::parser::{Options, Parser, Token};
use super::UNBOUNDED;

/// The first bytes of every encoded stream: "LZ77" followed by format version 1.
/// The version stays 1 while the format is a prototype.
const MAGIC: [u8; 5] = [b'L', b'Z', b'7', b'7', 1];

/// Size of the chunks read from the input and flushed from the decoder.
const CHUNK_SIZE: usize = 1 << 16;

/// Most tokens in one Huffman block.
pub(super) const BLOCK_TOKENS: usize = 1 << 16;

/// The most bytes a uvarint takes.
const MAX_VARINT_LEN: usize = 10;

/// An error from [`encode`] or [`decode`].
#[derive(Debug)]
pub enum Error {
    /// The encoded stream is malformed, truncated, or has trailing input.
    Format(String),
    /// Reading the input or writing the output failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Format(msg) => write!(f, "lz77: invalid encoded stream: {msg}"),
            Error::Io(err) => write!(f, "lz77: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Format(_) => None,
            Error::Io(err) => Some(err),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

fn format_error<T>(msg: String) -> Result<T, Error> {
    Err(Error::Format(msg))
}

/// Selects how a record field is written. The values of `Fixed` and `Varint`
/// are also their codes in the stream header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Coding {
    /// `Fixed` for distances and `Varint` for lengths.
    #[default]
    Default = 0,
    /// Little-endian, in the fewest of 1, 2, 4, or 8 bytes that hold the
    /// field's largest value.
    Fixed = 1,
    /// Unsigned LEB128.
    Varint = 2,
}

/// Selects whether record fields are entropy coded. The values are also their
/// codes in the stream header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Entropy {
    /// Records are written one after another as bytes.
    #[default]
    None = 0,
    /// Records are grouped into blocks of up to 65536 tokens, whose fields are
    /// split into byte streams and compressed with Huffman-only deflate.
    Huffman = 1,
}

/// Configures [`encode`]: the parser options and the coding of each record
/// field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncodeOptions {
    pub options: Options,

    /// Coding of distances, stored as distance - 1 with largest value
    /// `window - 1`. `Default` selects `Fixed`.
    pub distance: Coding,

    /// Coding of lengths, with largest value `max_length`. `Default` selects
    /// `Varint`.
    pub length: Coding,

    /// Entropy coding of the records.
    pub entropy: Entropy,
}

/// Parses all of `src` with a [`Parser`] and writes the tokens to `dst` in the
/// format described in `docs/lz77/README.md`: a header followed by one
/// record per token, grouped into blocks when `opts.entropy` is `Huffman`.
/// Neither stream is closed. Output may be incomplete on error.
pub fn encode(dst: impl Write, mut src: impl Read, opts: &EncodeOptions) -> Result<(), Error> {
    let parser_opts = opts.options.with_defaults();
    let distance_coding = if opts.distance == Coding::Default { Coding::Fixed } else { opts.distance };
    let length_coding = if opts.length == Coding::Default { Coding::Varint } else { opts.length };
    let distance = Field::new("distance", distance_coding, (parser_opts.window - 1) as u64);
    let length = Field::new("length", length_coding, parser_opts.max_length as u64);

    let mut w = BufWriter::new(dst);
    let mut header = MAGIC.to_vec();
    put_uvarint(&mut header, parser_opts.window as u64);
    put_uvarint(&mut header, parser_opts.max_length as u64);
    header.extend_from_slice(&[distance_coding as u8, length_coding as u8, opts.entropy as u8]);
    w.write_all(&header)?;

    let mut p = Parser::new(parser_opts);
    let mut sink = match opts.entropy {
        Entropy::None => Sink::Records { length, distance, record: Vec::new() },
        Entropy::Huffman => Sink::Blocks(Box::new(HuffmanBlock::new(length, distance))),
    };
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let n = match src.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        let mut data = &buffer[..n];
        while !data.is_empty() {
            let (token, consumed) = p.push(data);
            data = &data[consumed..];
            if let Some(token) = token {
                sink.write(&mut w, token)?;
            }
        }
    }
    if let Some(token) = p.finish() {
        sink.write(&mut w, token)?;
    }
    if let Sink::Blocks(block) = &mut sink {
        if block.tokens > 0 {
            block.flush(&mut w)?;
        }
    }
    w.flush()?;
    Ok(())
}

/// Receives the tokens of [`encode`], either writing each as a record or
/// collecting them into Huffman blocks.
enum Sink {
    Records { length: Field, distance: Field, record: Vec<u8> },
    Blocks(Box<HuffmanBlock>),
}

impl Sink {
    fn write(&mut self, w: &mut impl Write, t: Token) -> Result<(), Error> {
        match self {
            Sink::Records { length, distance, record } => {
                record.clear();
                length.append(record, t.length as u64);
                if t.length > 0 {
                    distance.append(record, (t.distance - 1) as u64);
                }
                record.push(t.next);
                w.write_all(record)?;
            }
            Sink::Blocks(block) => {
                if block.add(t) >= BLOCK_TOKENS {
                    block.flush(w)?;
                }
            }
        }
        Ok(())
    }
}

/// Describes how one record field is written.
#[derive(Clone, Copy)]
pub(super) struct Field {
    name: &'static str,
    /// Bytes of a `Fixed` field; zero for `Varint`.
    pub(super) width: usize,
    max: u64,
}

impl Field {
    pub(super) fn new(name: &'static str, c: Coding, max: u64) -> Field {
        let mut width = 0;
        if c == Coding::Fixed {
            width = 8;
            for w in [1, 2, 4] {
                if max < 1 << (8 * w) {
                    width = w;
                    break;
                }
            }
        }
        Field { name, width, max }
    }

    fn append(&self, dst: &mut Vec<u8>, v: u64) {
        if self.width == 0 {
            put_uvarint(dst, v);
        } else {
            dst.extend_from_slice(&v.to_le_bytes()[..self.width]);
        }
    }

    /// Returns the number of byte streams the field takes in a Huffman block:
    /// one per byte of a `Fixed` field, or one for `Varint`.
    fn streams(&self) -> usize {
        self.width.max(1)
    }

    /// Appends `v` to the field's streams in a Huffman block: byte k of a
    /// `Fixed` field goes to stream k, and a `Varint` goes whole to its only
    /// stream.
    fn append_split(&self, streams: &mut [Vec<u8>], v: u64) {
        if self.width == 0 {
            put_uvarint(&mut streams[0], v);
            return;
        }
        for (k, stream) in streams.iter_mut().enumerate() {
            stream.push((v >> (8 * k)) as u8);
        }
    }

    /// Reads one value written by `append_split` and checks that it does not
    /// exceed `max`.
    fn read_split(&self, streams: &mut [&[u8]]) -> Result<u64, Error> {
        if self.width == 0 {
            return self.read(&mut streams[0]);
        }
        let mut v = 0u64;
        for (k, stream) in streams.iter_mut().enumerate() {
            v |= (read_byte(stream, self.name)? as u64) << (8 * k);
        }
        self.check(v)
    }

    fn check(&self, v: u64) -> Result<u64, Error> {
        if v > self.max {
            return format_error(format!("{} {} exceeds {}", self.name, v, self.max));
        }
        Ok(v)
    }

    /// Reads one value and checks that it does not exceed `max`.
    fn read(&self, r: &mut impl Read) -> Result<u64, Error> {
        let v = if self.width == 0 {
            read_uvarint(r, self.name)?
        } else {
            let mut b = [0u8; 8];
            read_exact(r, &mut b[..self.width], self.name)?;
            u64::from_le_bytes(b)
        };
        self.check(v)
    }
}

/// Appends the unsigned LEB128 encoding of `v`.
pub(super) fn put_uvarint(dst: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        dst.push(v as u8 | 0x80);
        v >>= 7;
    }
    dst.push(v as u8);
}

/// Collects the records of one Huffman block as byte streams: the length
/// streams, then the distance streams, then the literals.
struct HuffmanBlock {
    length: Field,
    distance: Field,
    streams: Vec<Vec<u8>>,
    tokens: usize,
    compressor: CompressorOxide,
}

impl HuffmanBlock {
    fn new(length: Field, distance: Field) -> HuffmanBlock {
        let flags = create_comp_flags_from_zip_params(1, 0, CompressionStrategy::HuffmanOnly as i32);
        HuffmanBlock {
            length,
            distance,
            streams: vec![Vec::new(); length.streams() + distance.streams() + 1],
            tokens: 0,
            compressor: CompressorOxide::new(flags),
        }
    }

    /// Adds `t` to the block and returns the number of tokens in it.
    fn add(&mut self, t: Token) -> usize {
        let n = self.length.streams();
        let last = self.streams.len() - 1;
        self.length.append_split(&mut self.streams[..n], t.length as u64);
        if t.length > 0 {
            self.distance.append_split(&mut self.streams[n..last], (t.distance - 1) as u64);
        }
        self.streams[last].push(t.next);
        self.tokens += 1;
        self.tokens
    }

    /// Writes the block and empties it.
    fn flush(&mut self, w: &mut impl Write) -> Result<(), Error> {
        let mut out = Vec::new();
        put_uvarint(&mut out, self.tokens as u64);
        for stream in &mut self.streams {
            let compressed = deflate_huffman_only(&mut self.compressor, stream);
            put_uvarint(&mut out, compressed.len() as u64);
            out.extend_from_slice(&compressed);
            stream.clear();
        }
        self.tokens = 0;
        w.write_all(&out)?;
        Ok(())
    }
}

/// Compresses `input` as one raw deflate stream (RFC 1951) that uses Huffman
/// codes but no matching of its own.
pub(super) fn deflate_huffman_only(compressor: &mut CompressorOxide, mut input: &[u8]) -> Vec<u8> {
    compressor.reset();
    let mut output = vec![0u8; (input.len() / 2).max(64)];
    let mut out_pos = 0;
    loop {
        let (status, bytes_in, bytes_out) = compress(compressor, input, &mut output[out_pos..], TDEFLFlush::Finish);
        out_pos += bytes_out;
        input = &input[bytes_in..];
        match status {
            TDEFLStatus::Done => {
                output.truncate(out_pos);
                return output;
            }
            TDEFLStatus::Okay => {
                if output.len() - out_pos < 64 {
                    output.resize(output.len() * 2, 0);
                }
            }
            status => unreachable!("deflate of an in-memory buffer failed: {status:?}"),
        }
    }
}

/// Reads a stream written by [`encode`] from `src` and writes the original
/// bytes to `dst`. The stream ends at the end of `src`, which must fall on a
/// record or block boundary. Returns [`Error::Format`] if the stream is
/// malformed or ends inside the header, a record, or a block. Neither stream is closed;
/// output may be incomplete on error. Retains O(min(n, window)) bytes of
/// history.
pub fn decode(dst: impl Write, src: impl Read) -> Result<(), Error> {
    let mut r = BufReader::new(src);
    let mut header = [0u8; MAGIC.len()];
    read_exact(&mut r, &mut header, "header")?;
    if header != MAGIC {
        return format_error(format!("bad magic or version {header:02x?}"));
    }
    let window = read_uvarint(&mut r, "window")?;
    let max_length = read_uvarint(&mut r, "max length")?;
    if window == 0 || window > UNBOUNDED as u64 || max_length == 0 || max_length > UNBOUNDED as u64 {
        return format_error(format!("window {window} or max length {max_length} out of range"));
    }
    let mut codings = [0u8; 3];
    read_exact(&mut r, &mut codings, "codings")?;
    for &c in &codings[..2] {
        if c != Coding::Fixed as u8 && c != Coding::Varint as u8 {
            return format_error(format!("invalid coding {c}"));
        }
    }
    if codings[2] > Entropy::Huffman as u8 {
        return format_error(format!("invalid entropy coding {}", codings[2]));
    }
    let coding = |c: u8| if c == Coding::Fixed as u8 { Coding::Fixed } else { Coding::Varint };
    let distance = Field::new("distance", coding(codings[0]), window - 1);
    let length = Field::new("length", coding(codings[1]), max_length);

    let mut d = Decoder { dst, window: window as usize, history: Vec::new(), pending: 0, produced: 0 };
    if codings[2] == Entropy::Huffman as u8 {
        d.huffman_blocks(&mut r, length, distance)?;
    } else {
        d.records(&mut r, length, distance)?;
    }
    d.flush()
}

/// Returns true if `r` has no more input.
fn at_end(r: &mut impl BufRead) -> Result<bool, Error> {
    loop {
        match r.fill_buf() {
            Ok(buf) => return Ok(buf.is_empty()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

/// Converts a read error to a format error when the input ended early.
fn read_error(field: &str, err: io::Error) -> Error {
    if err.kind() == io::ErrorKind::UnexpectedEof {
        Error::Format(format!("truncated {field}"))
    } else {
        Error::Io(err)
    }
}

fn read_exact(r: &mut impl Read, buf: &mut [u8], field: &str) -> Result<(), Error> {
    r.read_exact(buf).map_err(|err| read_error(field, err))
}

fn read_byte(r: &mut impl Read, field: &str) -> Result<u8, Error> {
    let mut b = [0u8; 1];
    read_exact(r, &mut b, field)?;
    Ok(b[0])
}

/// Reads an unsigned LEB128 value, distinguishing overflow from read errors.
fn read_uvarint(r: &mut impl Read, field: &str) -> Result<u64, Error> {
    let mut x = 0u64;
    for i in 0..MAX_VARINT_LEN {
        let b = read_byte(r, field)?;
        if b < 0x80 {
            if i == MAX_VARINT_LEN - 1 && b > 1 {
                break;
            }
            return Ok(x | (b as u64) << (7 * i));
        }
        x |= ((b & 0x7f) as u64) << (7 * i);
    }
    format_error(format!("{field} varint overflow"))
}

/// Decompresses one raw deflate stream that must fill `data` exactly and
/// produce at most `limit` bytes.
fn inflate_stream(data: &[u8], limit: usize, index: usize) -> Result<Vec<u8>, Error> {
    let mut out = vec![0u8; limit + 1];
    let mut state = DecompressorOxide::new();
    let (status, consumed, written) =
        decompress(&mut state, data, &mut out, 0, TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
    match status {
        TINFLStatus::Done if written > limit => format_error(format!("stream {index} is too long")),
        TINFLStatus::Done if consumed < data.len() => {
            format_error(format!("{} bytes after stream {index}", data.len() - consumed))
        }
        TINFLStatus::Done => {
            out.truncate(written);
            Ok(out)
        }
        TINFLStatus::HasMoreOutput => format_error(format!("stream {index} is too long")),
        _ => format_error(format!("corrupt stream {index}")),
    }
}

/// Holds the decoded output: `history[..pending]` was already written to `dst`
/// and `history[pending..]` was not.
struct Decoder<W: Write> {
    dst: W,
    window: usize,
    history: Vec<u8>,
    pending: usize,
    produced: usize,
}

impl<W: Write> Decoder<W> {
    /// Decodes records written as bytes until the end of `r`.
    fn records(&mut self, r: &mut BufReader<impl Read>, length: Field, distance: Field) -> Result<(), Error> {
        while !at_end(r)? {
            let n = length.read(r)?;
            let stored = if n > 0 { distance.read(r)? } else { 0 };
            let c = read_byte(r, "literal")?;
            self.record(n, stored, c)?;
        }
        Ok(())
    }

    /// Decodes Huffman blocks until the end of `r`.
    fn huffman_blocks(&mut self, r: &mut BufReader<impl Read>, length: Field, distance: Field) -> Result<(), Error> {
        let (nl, nd) = (length.streams(), distance.streams());
        let count = nl + nd + 1;
        // Each token puts at most this many bytes into each stream.
        let per_token: Vec<usize> = (0..count)
            .map(|i| {
                let varint = if i < nl { length.width == 0 } else { i < count - 1 && distance.width == 0 };
                if varint {
                    MAX_VARINT_LEN
                } else {
                    1
                }
            })
            .collect();
        let mut raw: Vec<Vec<u8>> = vec![Vec::new(); count];
        while !at_end(r)? {
            let tokens = read_uvarint(r, "block size")?;
            if tokens == 0 || tokens > BLOCK_TOKENS as u64 {
                return format_error(format!("block of {tokens} tokens"));
            }
            let tokens = tokens as usize;
            for i in 0..count {
                let size = read_uvarint(r, "stream size")?;
                let mut data = Vec::new();
                r.by_ref().take(size).read_to_end(&mut data)?;
                if (data.len() as u64) < size {
                    return format_error("truncated stream".to_string());
                }
                raw[i] = inflate_stream(&data, tokens * per_token[i], i)?;
            }
            let mut streams: Vec<&[u8]> = raw.iter().map(|s| s.as_slice()).collect();
            for _ in 0..tokens {
                let n = length.read_split(&mut streams[..nl])?;
                let stored = if n > 0 { distance.read_split(&mut streams[nl..count - 1])? } else { 0 };
                let c = read_byte(&mut streams[count - 1], "literal")?;
                self.record(n, stored, c)?;
            }
            for (i, s) in streams.iter().enumerate() {
                if !s.is_empty() {
                    return format_error(format!("{} unused bytes in stream {i}", s.len()));
                }
            }
        }
        Ok(())
    }

    /// Applies one record: a copy of `n` bytes from `stored + 1` bytes back if
    /// `n > 0`, then the literal `c`.
    fn record(&mut self, n: u64, stored: u64, c: u8) -> Result<(), Error> {
        if n > 0 {
            if stored >= self.produced as u64 {
                return format_error(format!("distance {} at output position {}", stored + 1, self.produced));
            }
            self.copy(stored as usize + 1, n as usize)?;
        }
        self.literal(c)
    }

    fn literal(&mut self, c: u8) -> Result<(), Error> {
        self.history.push(c);
        self.produced += 1;
        self.maybe_flush()
    }

    /// Copies one byte at a time so that length may exceed distance.
    fn copy(&mut self, distance: usize, length: usize) -> Result<(), Error> {
        for _ in 0..length {
            self.history.push(self.history[self.history.len() - distance]);
            self.produced += 1;
            self.maybe_flush()?;
        }
        Ok(())
    }

    /// Writes pending output once a chunk has accumulated and keeps only the
    /// last `window` bytes of history once it has grown to twice the window.
    fn maybe_flush(&mut self) -> Result<(), Error> {
        if self.history.len() - self.pending < CHUNK_SIZE {
            return Ok(());
        }
        self.flush()?;
        let keep = self.window;
        if self.history.len().checked_sub(keep).is_some_and(|extra| extra >= keep) {
            self.history.drain(..self.history.len() - keep);
            self.pending = self.history.len();
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.dst.write_all(&self.history[self.pending..])?;
        self.pending = self.history.len();
        Ok(())
    }
}
