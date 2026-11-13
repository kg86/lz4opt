//! Encodes standard input as an LZ4 frame on standard output.

use std::io::{self, BufWriter, Write};

use lz4opt::lz4::{self, Options, Parser};

#[path = "common/cli.rs"]
mod cli;

#[path = "common/diagnostics.rs"]
mod diagnostics;
use diagnostics::{fail, usage};

const USAGE: &str = "usage: lz4opt [-block-size bytes] [-parser greedy|optimal] < input > output.lz4";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut options = Options { block_size: lz4::BLOCK_4_MIB, parser: Parser::Optimal };
    let rest = cli::parse_flags(&args, |name, value| {
        match name {
            "block-size" => options.block_size = value.parse().ok()?,
            "parser" => {
                options.parser = match value {
                    "greedy" => Parser::Greedy,
                    "optimal" => Parser::Optimal,
                    _ => return None,
                }
            }
            _ => return None,
        }
        Some(())
    })
    .unwrap_or_else(|| usage(USAGE));
    if !rest.is_empty() {
        usage(USAGE);
    }
    options.validate().unwrap_or_else(|err| usage(err));
    let mut out = BufWriter::new(io::stdout().lock());
    if let Err(err) =
        lz4::encode(&mut out, io::stdin().lock(), &options).and_then(|()| out.flush().map_err(lz4::Error::Write))
    {
        fail(err);
    }
}
