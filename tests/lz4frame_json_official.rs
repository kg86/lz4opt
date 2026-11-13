//! Drives the inspect/restore round trip against files produced by the official `lz4` CLI, which is skipped when the
//! CLI is not installed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_lz4frame-json");

/// A temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("lz4frame-json-official-{}-{n}-{label}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Returns whether the official `lz4` CLI can be run.
fn lz4_available() -> bool {
    Command::new("lz4").arg("-V").output().is_ok_and(|out| out.status.success())
}

fn sample() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("sample.txt");
    fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

/// Returns whether lz4's combined stdout+stderr output looks like a command-line parsing failure (an unrecognized
/// flag) rather than an unrelated I/O or environment failure. An unrecognized argument makes lz4 print "Incorrect
/// parameters" followed by its usage text starting with "Usage :", while file I/O failures report a plain one-line
/// error with no usage dump. This lets the option-bearing subcases skip only on a genuine flag-support gap and still
/// fail loudly on any other kind of compress failure.
fn looks_like_unsupported_option(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).contains("Usage")
}

/// Runs `lz4` with `args`, returning its combined output on failure.
fn lz4(args: &[&str]) -> Result<(), Vec<u8>> {
    let out = Command::new("lz4").args(args).output().unwrap();
    if out.status.success() {
        Ok(())
    } else {
        Err([out.stdout, out.stderr].concat())
    }
}

/// Compresses the sample with `compress_flags`, then checks the whole inspect/restore pipeline in `dir`: the restored
/// .lz4 is byte-identical to the original and both decompress back to the sample. Returns the inspect JSON text, or
/// `None` when this lz4 build does not support the flags (the case is then skipped).
fn round_trip(case: &str, dir: &TempDir, compress_flags: &[&str], sample: &[u8]) -> Option<Vec<u8>> {
    let sample_path = dir.join("sample.txt");
    fs::write(&sample_path, sample).unwrap();
    let original_lz4 = dir.join("original.lz4");

    let mut compress_args: Vec<&str> = compress_flags.to_vec();
    compress_args.extend(["-q", "-f", "-k", sample_path.to_str().unwrap(), original_lz4.to_str().unwrap()]);
    if let Err(out) = lz4(&compress_args) {
        // Skip only when this case actually passed a version-dependent flag and lz4's own output says it did not
        // recognize the command line. Any other failure is fatal, so a broken lz4 install cannot hide behind
        // "unsupported option".
        if !compress_flags.is_empty() && looks_like_unsupported_option(&out) {
            eprintln!(
                "{case}: lz4 {compress_flags:?} not supported by this lz4 build: {}",
                String::from_utf8_lossy(&out)
            );
            return None;
        }
        panic!("{case}: lz4 compress failed: {}", String::from_utf8_lossy(&out));
    }

    let inspect = Command::new(BIN).arg("inspect").arg(&original_lz4).output().unwrap();
    assert!(
        inspect.status.success(),
        "{case}: inspect: {:?}, stderr = {}",
        inspect.status,
        String::from_utf8_lossy(&inspect.stderr)
    );
    let frame_json_path = dir.join("frame.json");
    fs::write(&frame_json_path, &inspect.stdout).unwrap();

    let restored_lz4 = dir.join("restored.lz4");
    let restore = Command::new(BIN).arg("restore").arg(&frame_json_path).arg(&restored_lz4).output().unwrap();
    assert!(
        restore.status.success(),
        "{case}: restore: {:?}, stderr = {}",
        restore.status,
        String::from_utf8_lossy(&restore.stderr)
    );

    let original_bytes = fs::read(&original_lz4).unwrap();
    let restored_bytes = fs::read(&restored_lz4).unwrap();
    assert!(
        original_bytes == restored_bytes,
        "{case}: restored .lz4 differs from original:\noriginal: {original_bytes:x?}\nrestored: {restored_bytes:x?}"
    );

    for (name, lz4_path) in [("original", &original_lz4), ("restored", &restored_lz4)] {
        let decompressed = dir.join(&format!("{name}.decompressed"));
        if let Err(out) = lz4(&["-q", "-d", "-f", "-k", lz4_path.to_str().unwrap(), decompressed.to_str().unwrap()]) {
            panic!("{case}: decompressing {name}.lz4: {}", String::from_utf8_lossy(&out));
        }
        let got = fs::read(&decompressed).unwrap();
        assert!(got == sample, "{case}: decompressed {name} differs from sample.txt");
    }
    Some(inspect.stdout)
}

/// Compresses testdata/sample.txt with the official lz4 CLI, inspects the result, restores it, and confirms both the
/// restored .lz4 bytes and the decompressed contents (of both the original and the restored file) match. It covers
/// several representative lz4 CLI options (default, block checksum, content size, an alternate block size code, and a
/// combination) since those toggle different Frame Descriptor and Data Block fields this tool parses and restores.
#[test]
fn official_lz4_integration() {
    if !lz4_available() {
        eprintln!("official lz4 CLI not found on PATH: skipping integration test");
        return;
    }
    let sample = sample();

    let cases: [(&str, &[&str]); 5] = [
        ("default", &[]),
        ("block checksum", &["-BX"]),
        ("content size", &["--content-size"]),
        ("alternate block size code", &["-B4"]),
        ("block checksum and content size", &["-BX", "--content-size"]),
    ];
    for (case, flags) in cases {
        let dir = TempDir::new("integration");
        round_trip(case, &dir, flags, &sample);
    }
}

/// Representative combinations of block independence/dependency, Block Maximum Size, and the block checksum option,
/// each compressing testdata/sample.txt with the official lz4 CLI. For every combination it confirms that inspect
/// decomposes the resulting compressed Data Block into more than one sequence with a non-empty token physical field
/// on every sequence, that restore reproduces the original .lz4 bytes exactly, and that both the original and restored
/// .lz4 decompress back to sample.txt unchanged.
#[test]
fn official_lz4_fixtures_block_format() {
    if !lz4_available() {
        eprintln!("official lz4 CLI not found on PATH: skipping integration test");
        return;
    }
    let sample = sample();

    let cases: [(&str, &[&str]); 9] = [
        // No-option baseline: unlike every other case, this one never skips on "unsupported option", so it anchors
        // the test against all cases silently skipping and verifying nothing.
        ("block independence, default block size, no checksum (baseline)", &[]),
        ("block dependency, default block size, no checksum", &["-BD"]),
        ("block independence, block checksum", &["-BI", "-BX"]),
        ("block dependency, block checksum", &["-BD", "-BX"]),
        ("block max size code 4 (64 KiB)", &["-B4"]),
        ("block max size code 5 (256 KiB)", &["-B5"]),
        ("block max size code 6 (1 MiB)", &["-B6"]),
        ("block max size code 7 (4 MiB, default)", &["-B7"]),
        ("block dependency, block checksum, block max size code 5", &["-BD", "-BX", "-B5"]),
    ];

    let mut ran_structural_assertion = false;
    for (case, flags) in cases {
        let dir = TempDir::new("block-format");
        let Some(json) = round_trip(case, &dir, flags, &sample) else { continue };
        let doc: serde_json::Value =
            serde_json::from_slice(&json).unwrap_or_else(|err| panic!("{case}: decoding inspect JSON: {err}"));
        assert_has_multi_sequence_compressed_block(case, &doc);
        ran_structural_assertion = true;
    }

    assert!(
        ran_structural_assertion,
        "every case skipped before reaching the Block Format structural assertion; this lz4 build may not support the options this test relies on"
    );
}

/// Panics unless `doc` contains at least one compressed Data Block whose data[] array has more than one sequence, with
/// every sequence's token carrying a non-empty hex physical field.
fn assert_has_multi_sequence_compressed_block(case: &str, doc: &serde_json::Value) {
    let mut found = false;
    for frame in doc["frames"].as_array().map(Vec::as_slice).unwrap_or_default() {
        for block in frame["data_blocks"].as_array().map(Vec::as_slice).unwrap_or_default() {
            if block["block_size"]["uncompressed"].as_bool().unwrap_or(false) {
                continue;
            }
            let data = block["data"].as_array().map(Vec::as_slice).unwrap_or_default();
            for seq in data {
                let hex = seq["token"]["hex"].as_str().unwrap_or("");
                assert!(!hex.is_empty(), "{case}: compressed block sequence has empty token.hex");
            }
            if data.len() > 1 {
                found = true;
            }
        }
    }
    assert!(found, "{case}: no compressed Data Block decomposed into more than one sequence in inspect output");
}
