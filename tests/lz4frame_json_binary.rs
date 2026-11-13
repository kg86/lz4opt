//! Runs the built `lz4frame-json` binary as a real OS process. The in-process tests of `run` pin argument parsing,
//! I/O plumbing, and exit codes; these tests check that `main` actually wires the process arguments, standard
//! streams, and exit status to it.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_lz4frame-json");

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

/// A temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("lz4frame-json-bin-{}-{n}-{label}", std::process::id()));
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

/// Runs an inspect-then-restore round trip through file arguments, confirming `main`'s argument and file-based I/O
/// paths actually reach the behavior the in-process tests already pin.
#[test]
fn built_binary_inspect_restore_round_trip() {
    let dir = TempDir::new("round-trip");
    let in_path = dir.join("sample.lz4");
    let json_path = dir.join("sample.json");
    let out_path = dir.join("restored.lz4");

    let want = minimal_frame_bytes();
    fs::write(&in_path, &want).unwrap();

    let inspect = Command::new(BIN).arg("inspect").arg(&in_path).output().unwrap();
    assert!(
        inspect.status.success(),
        "inspect: {:?}, stderr = {}",
        inspect.status,
        String::from_utf8_lossy(&inspect.stderr)
    );
    fs::write(&json_path, &inspect.stdout).unwrap();

    let restore = Command::new(BIN).arg("restore").arg(&json_path).arg(&out_path).output().unwrap();
    assert!(
        restore.status.success(),
        "restore: {:?}, stderr = {}",
        restore.status,
        String::from_utf8_lossy(&restore.stderr)
    );

    assert_eq!(fs::read(&out_path).unwrap(), want, "restored bytes");
}

/// Confirms `main` passes `run`'s return value to the process exit status: an in-process call to `run` can observe
/// only its return value, never whether `main` propagates it.
#[test]
fn built_binary_usage_error_exit_code() {
    let out = Command::new(BIN).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "exit status = {:?}", out.status);
    assert!(!out.stderr.is_empty(), "stderr is empty, want a usage message");
}

/// A character-device stdout (here /dev/null, standing in for a terminal) is rejected by restore with exit code 1,
/// confirming `main` detects it and passes the flag to `run`.
#[cfg(unix)]
#[test]
fn built_binary_restore_rejects_character_device_stdout() {
    let dir = TempDir::new("chardev");
    let json_path = dir.join("x.json");
    let frame_path = dir.join("x.lz4");
    fs::write(&frame_path, minimal_frame_bytes()).unwrap();
    let inspect = Command::new(BIN).arg("inspect").arg(&frame_path).output().unwrap();
    assert!(inspect.status.success(), "inspect: {:?}", inspect.status);
    fs::write(&json_path, &inspect.stdout).unwrap();

    let out = Command::new(BIN)
        .arg("restore")
        .arg(&json_path)
        .stdout(Stdio::from(fs::File::create("/dev/null").unwrap()))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "exit status = {:?}", out.status);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to write binary LZ4 data to a terminal"),
        "stderr = {stderr:?}, want the terminal refusal message"
    );
}
