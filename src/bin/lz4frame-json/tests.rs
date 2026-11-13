use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

// Fixtures

const MAGIC_NORMAL: u32 = 0x184D2204;
const MAGIC_SKIPPABLE_MIN: u32 = 0x184D2A50;
const MAGIC_SKIPPABLE_MAX: u32 = 0x184D2A5F;

fn le32(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn concat_all(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// Returns the smallest valid LZ4 Frame: Magic Number, FLG (version 1, all flags off), BD (Block Maximum Size code 4),
/// a Header Checksum byte (its value is never validated), and an EndMark with no Data Blocks.
fn minimal_frame_bytes() -> Vec<u8> {
    vec![
        0x04, 0x22, 0x4d, 0x18, // magic number, little-endian
        0x40, // FLG: version 1, no optional fields
        0x40, // BD: block max size code 4
        0x00, // header checksum (unvalidated)
        0x00, 0x00, 0x00, 0x00, // end mark
    ]
}

/// Builds a valid compressed Data Block's data: a single literal-only Block Format sequence (token low nibble 0, no
/// match) whose literals are exactly `payload`, which must be shorter than 15 bytes.
fn literal_only_block_bytes(payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 15, "literal_only_block_bytes: payload too long for a single-byte token nibble");
    concat_all(&[&[(payload.len() as u8) << 4], payload])
}

/// Returns a single-block normal Frame with both Block Checksum and Content Checksum present. Checksum values are
/// never validated by this tool, so arbitrary bytes suffice.
fn checksummed_frame_bytes() -> Vec<u8> {
    const FLG: u8 = 0x54; // version 1, block checksum, content checksum
    const BD: u8 = 0x40; // block max size code 4
    let data = literal_only_block_bytes(&[0x01, 0x02, 0x03]);
    concat_all(&[
        &le32(MAGIC_NORMAL),
        &[FLG, BD, 0x2a], // header checksum (unvalidated)
        &le32(data.len() as u32),
        &data,
        &le32(0x12345678), // block checksum
        &le32(0),          // end mark
        &le32(0xf0debc9a), // content checksum
    ])
}

/// Returns a normal Frame with three Data Blocks: one compressed, one uncompressed, and a trailing empty (size 0)
/// uncompressed block sharing its offset with the following EndMark.
fn multi_block_frame_bytes() -> Vec<u8> {
    const FLG: u8 = 0x40; // version 1, no optional fields
    const BD: u8 = 0x40;
    let block0 = literal_only_block_bytes(&[0x01, 0x02, 0x03]);
    let block1 = [0xaa, 0xbb];
    concat_all(&[
        &le32(MAGIC_NORMAL),
        &[FLG, BD, 0x00], // header checksum
        &le32(block0.len() as u32),
        &block0,
        &le32(0x80000000 | block1.len() as u32),
        &block1,
        &le32(0x80000000), // empty uncompressed block
        &le32(0),          // end mark
    ])
}

/// Returns a standalone Skippable Frame carrying `data` (which may be empty).
fn skippable_frame_fixture_bytes(magic: u32, data: &[u8]) -> Vec<u8> {
    concat_all(&[&le32(magic), &le32(data.len() as u32), data])
}

/// Mixes a fully-checksummed normal Frame, a Skippable Frame carrying data, the minimal normal Frame, and a size-0
/// Skippable Frame, exercising Frame Concatenation across both Frame kinds.
fn concatenated_stream_bytes() -> Vec<u8> {
    concat_all(&[
        &checksummed_frame_bytes(),
        &skippable_frame_fixture_bytes(MAGIC_SKIPPABLE_MIN, &[0x01, 0x02]),
        &minimal_frame_bytes(),
        &skippable_frame_fixture_bytes(MAGIC_SKIPPABLE_MAX, &[]),
    ])
}

// Helpers

/// A temporary directory removed on drop; permissions are reset first so a read-only directory can still be removed.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("lz4frame-json-test-{}-{n}-{label}", process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The captured result of one `run` call.
struct Outcome {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn run_with(list: &[&str], stdin: &[u8]) -> Outcome {
    let mut stdin = stdin;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run(&args(list), &mut stdin, &mut stdout, &mut stderr, false);
    Outcome { code, stdout, stderr: String::from_utf8_lossy(&stderr).into_owned() }
}

/// Runs inspect on `minimal_frame_bytes()` and returns the resulting JSON, so restore tests exercise a real
/// inspect->restore round trip through the CLI itself rather than a hand-built fixture.
fn minimal_frame_json() -> Vec<u8> {
    let out = run_with(&["inspect"], &minimal_frame_bytes());
    assert_eq!(out.code, 0, "inspect for fixture: stderr = {:?}", out.stderr);
    out.stdout
}

fn is_json(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok()
}

// Tests

#[test]
fn run_no_subcommand() {
    let out = run_with(&[], b"");
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(out.stderr.contains("usage"), "stderr = {:?}, want usage message", out.stderr);
}

#[test]
fn run_unknown_subcommand() {
    let out = run_with(&["bogus"], b"");
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(out.stderr.contains("usage"), "stderr = {:?}, want usage message", out.stderr);
}

#[test]
fn run_inspect_empty_input_is_parse_error() {
    let out = run_with(&["inspect"], b"");
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(!out.stderr.is_empty(), "stderr is empty, want an error message");
}

/// A file path, an omitted argument, and an explicit "-" argument all read the same input and produce identical JSON:
/// "-" is inspect's alias for stdin, the same as it is for restore's input and output.
#[test]
fn run_inspect_file_stdin_and_dash_produce_same_json() {
    let input = minimal_frame_bytes();
    let dir = TempDir::new("inspect-same");
    let path = dir.join("sample.lz4");
    fs::write(&path, &input).unwrap();
    let path = path.to_string_lossy().into_owned();

    let file = run_with(&["inspect", &path], b"");
    let stdin = run_with(&["inspect"], &input);
    let dash = run_with(&["inspect", "-"], &input);
    for (label, out) in [("file input", &file), ("stdin input", &stdin), ("dash input", &dash)] {
        assert_eq!(out.code, 0, "{label}: stderr = {:?}", out.stderr);
        assert!(out.stderr.is_empty(), "{label}: stderr = {:?}, want empty", out.stderr);
    }

    assert_eq!(file.stdout, stdin.stdout, "file and stdin produced different JSON");
    assert_eq!(file.stdout, dash.stdout, "file and dash produced different JSON");
    assert!(is_json(&file.stdout), "stdout is not valid JSON: {}", String::from_utf8_lossy(&file.stdout));
}

#[test]
fn run_inspect_malformed_input_no_partial_output() {
    // Truncated magic number: only 2 of the required 4 bytes.
    let out = run_with(&["inspect"], &[0x04, 0x22]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty on parse failure", out.stdout);
    assert!(out.stderr.contains("parse error at byte"), "stderr = {:?}, want a parse error message", out.stderr);
}

#[test]
fn run_inspect_missing_file() {
    let dir = TempDir::new("inspect-missing");
    let missing = dir.join("does-not-exist.lz4").to_string_lossy().into_owned();

    let out = run_with(&["inspect", &missing], b"");
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(out.stderr.contains(&missing), "stderr = {:?}, want an open error message naming {missing:?}", out.stderr);
}

#[test]
fn run_inspect_usage_error_too_many_args() {
    let out = run_with(&["inspect", "a.lz4", "b.lz4"], b"");
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(out.stderr.contains("usage"), "stderr = {:?}, want usage message", out.stderr);
}

#[test]
fn run_restore_empty_input_is_schema_error() {
    let out = run_with(&["restore"], b"");
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(!out.stderr.is_empty(), "stderr is empty, want an error message");
}

// restore: file/stdin x file/stdout combinations

#[test]
fn run_restore_file_to_file() {
    let want = minimal_frame_bytes();
    let dir = TempDir::new("restore-file-file");
    let in_path = dir.join("sample.json");
    let out_path = dir.join("restored.lz4");
    fs::write(&in_path, minimal_frame_json()).unwrap();

    let out = run_with(&["restore", in_path.to_str().unwrap(), out_path.to_str().unwrap()], b"");
    assert_eq!(out.code, 0, "stderr = {:?}", out.stderr);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty (output went to a file)", out.stdout);
    assert!(out.stderr.is_empty(), "stderr = {:?}, want empty", out.stderr);
    assert_eq!(fs::read(&out_path).unwrap(), want, "restored bytes");
}

#[test]
fn run_restore_file_to_stdout() {
    let dir = TempDir::new("restore-file-stdout");
    let in_path = dir.join("sample.json");
    fs::write(&in_path, minimal_frame_json()).unwrap();

    let out = run_with(&["restore", in_path.to_str().unwrap()], b"");
    assert_eq!(out.code, 0, "stderr = {:?}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr = {:?}, want empty", out.stderr);
    assert_eq!(out.stdout, minimal_frame_bytes(), "stdout");
}

#[test]
fn run_restore_stdin_to_stdout() {
    let out = run_with(&["restore"], &minimal_frame_json());
    assert_eq!(out.code, 0, "stderr = {:?}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr = {:?}, want empty", out.stderr);
    assert_eq!(out.stdout, minimal_frame_bytes(), "stdout");
}

#[test]
fn run_restore_stdin_to_file() {
    let dir = TempDir::new("restore-stdin-file");
    let out_path = dir.join("restored.lz4");

    // "-" selects stdin explicitly, since a file output argument alone would otherwise be read as the input
    // positional argument.
    let out = run_with(&["restore", "-", out_path.to_str().unwrap()], &minimal_frame_json());
    assert_eq!(out.code, 0, "stderr = {:?}", out.stderr);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty (output went to a file)", out.stdout);
    assert!(out.stderr.is_empty(), "stderr = {:?}, want empty", out.stderr);
    assert_eq!(fs::read(&out_path).unwrap(), minimal_frame_bytes(), "restored bytes");
}

#[test]
fn run_restore_dash_output_means_stdout() {
    let out = run_with(&["restore", "-", "-"], &minimal_frame_json());
    assert_eq!(out.code, 0, "stderr = {:?}", out.stderr);
    assert_eq!(out.stdout, minimal_frame_bytes(), "stdout");
}

// restore: stdout terminal rejection vs pipe/redirect allowance

#[test]
fn run_restore_rejects_character_device_stdout() {
    let mut stdin: &[u8] = &minimal_frame_json();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run(&args(&["restore"]), &mut stdin, &mut stdout, &mut stderr, true);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(code, 1);
    assert!(stdout.is_empty(), "stdout = {stdout:?}, want empty");
    assert!(stderr.contains("terminal"), "stderr = {stderr:?}, want an error message about a terminal");
}

/// An explicit output file is allowed even when stdout is a character device.
#[test]
fn run_restore_character_device_stdout_flag_ignored_with_output_file() {
    let dir = TempDir::new("restore-chardev-file");
    let out_path = dir.join("restored.lz4");
    let mut stdin: &[u8] = &minimal_frame_json();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run(&args(&["restore", "-", out_path.to_str().unwrap()]), &mut stdin, &mut stdout, &mut stderr, true);
    assert_eq!(code, 0, "stderr = {:?}", String::from_utf8_lossy(&stderr));
    assert_eq!(fs::read(&out_path).unwrap(), minimal_frame_bytes(), "restored bytes");
}

/// A pipe is not a character device, so restore writes to it; the writer must be dropped before reading to the end.
#[cfg(unix)]
#[test]
fn run_restore_allows_pipe_stdout() {
    let mut child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut pipe = child.stdin.take().unwrap();
    let mut stdin: &[u8] = &minimal_frame_json();
    let mut stderr = Vec::new();
    let code = run(&args(&["restore"]), &mut stdin, &mut pipe, &mut stderr, false);
    assert_eq!(code, 0, "stderr = {:?}", String::from_utf8_lossy(&stderr));
    drop(pipe);
    let got = child.wait_with_output().unwrap().stdout;
    assert_eq!(got, minimal_frame_bytes(), "piped bytes");
}

#[test]
fn run_restore_allows_redirected_file_stdout() {
    let dir = TempDir::new("restore-redirect");
    let path = dir.join("redirected.lz4");
    let mut f = File::create(&path).unwrap();

    let mut stdin: &[u8] = &minimal_frame_json();
    let mut stderr = Vec::new();
    let code = run(&args(&["restore"]), &mut stdin, &mut f, &mut stderr, false);
    assert_eq!(code, 0, "stderr = {:?}", String::from_utf8_lossy(&stderr));
    drop(f);
    assert_eq!(fs::read(&path).unwrap(), minimal_frame_bytes(), "redirected bytes");
}

// restore: atomic output on failure

#[test]
fn run_restore_malformed_json_leaves_existing_output_untouched() {
    let dir = TempDir::new("restore-malformed");
    let out_path = dir.join("existing.lz4");
    let original = b"pre-existing bytes that must survive";
    fs::write(&out_path, original).unwrap();

    let out = run_with(&["restore", "-", out_path.to_str().unwrap()], b"not json");
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(!out.stderr.is_empty(), "stderr is empty, want an error message");
    assert_eq!(fs::read(&out_path).unwrap(), original, "existing output changed");
    assert_eq!(dir.entries(), ["existing.lz4"], "want exactly 1 entry (no leftover temp file)");
}

/// Always reports writing one fewer byte than it was given, without returning an error: a technically
/// `io::Write`-contract-violating but real-world-observed behavior that restore must still report as a failure, since
/// a truncated .lz4 file is worse than a reported failure.
struct ShortWriter;

impl Write for ShortWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len().saturating_sub(1))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn run_restore_short_stdout_write_is_reported_as_failure() {
    let mut stdin: &[u8] = &minimal_frame_json();
    let mut stderr = Vec::new();
    let code = run(&args(&["restore"]), &mut stdin, &mut ShortWriter, &mut stderr, false);
    assert_eq!(code, 1);
    assert!(!stderr.is_empty(), "stderr is empty, want an error message about a short write");
}

/// Forces `write_file_atomic` to fail at its first step (creating the temporary file, because the destination
/// directory is not writable) rather than at JSON validation, so it pins an actual output failure distinctly from a
/// malformed-input failure.
#[cfg(unix)]
#[test]
fn run_restore_genuine_output_failure_leaves_existing_file_untouched() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("restore-readonly");
    let out_path = dir.join("existing.lz4");
    let original = b"pre-existing bytes that must survive a real I/O failure";
    fs::write(&out_path, original).unwrap();
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o555)).unwrap();

    // Privileged users can write into a read-only directory, so the failure cannot be forced.
    if File::create(dir.join("probe")).is_ok() {
        eprintln!("directory permissions are not enforced (running as root?); skipping");
        return;
    }

    let out = run_with(&["restore", "-", out_path.to_str().unwrap()], &minimal_frame_json());
    assert_eq!(out.code, 1);
    assert!(!out.stderr.is_empty(), "stderr is empty, want an error message");

    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(fs::read(&out_path).unwrap(), original, "existing output changed");
}

/// Forces `write_file_atomic` to fail at its last step (renaming, because the destination is an existing directory
/// rather than a file rename can replace) so the cleanup path past a successful write/close/chmod is exercised, not
/// just the early temporary-file creation failure.
#[test]
fn run_restore_rename_failure_cleans_up_temp_file() {
    let dir = TempDir::new("restore-rename");
    let out_path = dir.join("existing-dir.lz4");
    fs::create_dir(&out_path).unwrap();

    let out = run_with(&["restore", "-", out_path.to_str().unwrap()], &minimal_frame_json());
    assert_eq!(out.code, 1);
    assert!(!out.stderr.is_empty(), "stderr is empty, want an error message");
    assert!(out_path.is_dir(), "out_path is no longer a directory after a failed restore");
    assert_eq!(dir.entries(), ["existing-dir.lz4"], "want exactly 1 entry (no leftover temp file)");
}

#[test]
fn run_restore_malformed_json_creates_no_output_file() {
    let dir = TempDir::new("restore-no-output");
    let out_path = dir.join("never-created.lz4");

    let out = run_with(&["restore", "-", out_path.to_str().unwrap()], b"not json");
    assert_eq!(out.code, 1);
    assert!(!out_path.exists(), "{out_path:?} exists, want not-exist");
    assert!(dir.entries().is_empty(), "dir has entries {:?}, want 0 (no leftover temp file)", dir.entries());
}

// restore: usage errors

#[test]
fn run_restore_usage_error_too_many_args() {
    let dir = TempDir::new("restore-usage");
    let out_path = dir.join("b.lz4");
    let original = b"pre-existing bytes that must survive a usage error";
    fs::write(&out_path, original).unwrap();

    let out = run_with(&["restore", "a.json", out_path.to_str().unwrap(), "extra"], b"");
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(out.stderr.contains("usage"), "stderr = {:?}, want usage message", out.stderr);
    assert_eq!(fs::read(&out_path).unwrap(), original, "existing output changed");
    assert_eq!(dir.entries(), ["b.lz4"], "want exactly 1 entry (no leftover temp file)");
}

#[test]
fn run_restore_missing_input_file() {
    let dir = TempDir::new("restore-missing");
    let missing = dir.join("does-not-exist.json").to_string_lossy().into_owned();

    let out = run_with(&["restore", &missing], b"");
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(out.stderr.contains(&missing), "stderr = {:?}, want an open error message naming {missing:?}", out.stderr);
}

// Round trips

/// Drives `run` through inspect then restore for each fixture shape (a minimal Frame, a Frame with all checksums,
/// multiple Data Blocks, a standalone Skippable Frame, and a concatenated stream mixing normal and Skippable Frames)
/// and confirms the restored bytes are identical to the original input.
#[test]
fn run_inspect_restore_round_trip_fixtures() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("minimal frame", minimal_frame_bytes()),
        ("checksummed frame", checksummed_frame_bytes()),
        ("multiple data blocks", multi_block_frame_bytes()),
        ("skippable frame alone", skippable_frame_fixture_bytes(MAGIC_SKIPPABLE_MIN, &[0xaa, 0xbb, 0xcc])),
        ("concatenated stream mixing normal and skippable frames", concatenated_stream_bytes()),
    ];
    for (name, input) in cases {
        let inspected = run_with(&["inspect"], &input);
        assert_eq!(inspected.code, 0, "{name}: inspect stderr = {:?}", inspected.stderr);
        assert!(is_json(&inspected.stdout), "{name}: inspect output is not valid JSON");

        let restored = run_with(&["restore"], &inspected.stdout);
        assert_eq!(restored.code, 0, "{name}: restore stderr = {:?}", restored.stderr);
        assert_eq!(restored.stdout, input, "{name}: round trip mismatch");
    }
}

/// JSON that is syntactically valid but violates the restore schema (here, the required "frames" array is absent) is
/// rejected without disturbing an existing output file or leaving a temporary file behind.
#[test]
fn run_restore_schema_invalid_json_leaves_existing_output_untouched() {
    let dir = TempDir::new("restore-schema");
    let out_path = dir.join("existing.lz4");
    let original = b"pre-existing bytes that must survive a schema-invalid restore JSON";
    fs::write(&out_path, original).unwrap();

    let out = run_with(&["restore", "-", out_path.to_str().unwrap()], br#"{"not_frames": []}"#);
    assert_eq!(out.code, 1);
    assert!(out.stdout.is_empty(), "stdout = {:?}, want empty", out.stdout);
    assert!(!out.stderr.is_empty(), "stderr is empty, want an error message");
    assert_eq!(fs::read(&out_path).unwrap(), original, "existing output changed");
    assert_eq!(dir.entries(), ["existing.lz4"], "want exactly 1 entry (no leftover temp file)");
}
