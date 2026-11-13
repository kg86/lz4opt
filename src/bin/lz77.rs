//! Encodes a file or standard input to the lz77
//! module's format, or decodes it back, writing to standard output.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};

use lz4opt::lz77::{self, Coding, EncodeOptions, Entropy, Options};

#[path = "common/cli.rs"]
mod cli;

#[path = "common/diagnostics.rs"]
mod diagnostics;
use diagnostics::{fail, usage};

const USAGE: &str = "usage:
  lz77 encode [-window bytes] [-max-length bytes]
                      [-distance fixed|varint] [-length fixed|varint]
                      [-entropy none|huffman] [input] > output.lz77
  lz77 decode [input.lz77] > output";

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else { usage(USAGE) };
    let rest: Vec<String> = args.collect();
    let stdout = io::stdout().lock();
    let mut out = BufWriter::new(stdout);
    let result = match command.as_str() {
        "encode" => encode(&rest, &mut out),
        "decode" => decode(&rest, &mut out),
        _ => usage(USAGE),
    };
    if let Err(err) = result.and_then(|()| out.flush().map_err(Into::into)) {
        fail(err);
    }
}

fn encode(args: &[String], out: &mut impl Write) -> Result<(), lz77::Error> {
    // Values above isize::MAX select an unbounded window and length.
    let mut window = usize::MAX;
    let mut max_length = usize::MAX;
    let (mut distance, mut length, mut entropy) = ("fixed".to_string(), "varint".to_string(), "none".to_string());
    let positional = parse_flags(args, |name, value| {
        match name {
            "window" => window = value.parse().ok()?,
            "max-length" => max_length = value.parse().ok()?,
            "distance" => distance = value.to_string(),
            "length" => length = value.to_string(),
            "entropy" => entropy = value.to_string(),
            _ => return None,
        }
        Some(())
    });
    let coding = |s: &str| match s {
        "fixed" => Some(Coding::Fixed),
        "varint" => Some(Coding::Varint),
        _ => None,
    };
    let entropy = match entropy.as_str() {
        "none" => Some(Entropy::None),
        "huffman" => Some(Entropy::Huffman),
        _ => None,
    };
    let (Some(distance), Some(length), Some(entropy)) = (coding(&distance), coding(&length), entropy) else {
        usage(USAGE)
    };
    if positional.len() > 1 || window == 0 || max_length == 0 {
        usage(USAGE);
    }
    let (input, size) = open_input(positional.first().map(String::as_str));
    let mut options = Options { window, max_length };
    // Only a regular file has a size known before reading.
    if let Some(size) = size {
        options = options.fit_to_size(size);
    }
    lz77::encode(out, BufReader::new(input), &EncodeOptions { options, distance, length, entropy })
}

fn decode(args: &[String], out: &mut impl Write) -> Result<(), lz77::Error> {
    let positional = parse_flags(args, |_, _| None);
    if positional.len() > 1 {
        usage(USAGE);
    }
    let (input, _) = open_input(positional.first().map(String::as_str));
    lz77::decode(out, input)
}

/// Parses flags with [`cli::parse_flags`], failing with the usage message.
fn parse_flags(args: &[String], set: impl FnMut(&str, &str) -> Option<()>) -> Vec<String> {
    cli::parse_flags(args, set).unwrap_or_else(|| usage(USAGE))
}

/// Returns standard input for `None` or "-", and the named file otherwise,
/// with its size if it is a regular file.
fn open_input(name: Option<&str>) -> (Box<dyn Read>, Option<u64>) {
    match name {
        None | Some("-") => (Box::new(io::stdin()), stdin_size()),
        Some(name) => {
            let file = File::open(name).unwrap_or_else(|err| fail(err));
            let size = file.metadata().ok().filter(|m| m.is_file()).map(|m| m.len());
            (Box::new(file), size)
        }
    }
}

/// Returns the size of standard input if it is a regular file, as with a
/// `< file` redirection.
#[cfg(unix)]
fn stdin_size() -> Option<u64> {
    use std::os::fd::AsFd;
    let file = File::from(io::stdin().as_fd().try_clone_to_owned().ok()?);
    file.metadata().ok().filter(|m| m.is_file()).map(|m| m.len())
}

#[cfg(not(unix))]
fn stdin_size() -> Option<u64> {
    None
}
