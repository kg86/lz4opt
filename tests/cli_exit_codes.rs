use std::process::{Command, Stdio};

#[test]
fn invalid_arguments_exit_with_two() {
    for (binary, args) in [
        (env!("CARGO_BIN_EXE_bench"), vec![]),
        (env!("CARGO_BIN_EXE_bench"), vec!["-synthetic", "0", "-block-size", "1"]),
        (env!("CARGO_BIN_EXE_matchbench"), vec![]),
        (env!("CARGO_BIN_EXE_lz4opt"), vec!["-block-size", "1"]),
        (env!("CARGO_BIN_EXE_lz77"), vec![]),
        (env!("CARGO_BIN_EXE_lz77stats"), vec![]),
        (env!("CARGO_BIN_EXE_framestats"), vec![]),
        (env!("CARGO_BIN_EXE_lz4frame-json"), vec![]),
    ] {
        let output = Command::new(binary).args(&args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{binary}");
        assert!(!output.stderr.is_empty(), "{binary}");
        if args.contains(&"-block-size") {
            assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported block size 1"));
        }
    }
}

#[test]
fn missing_inputs_exit_with_one() {
    let missing = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/nonexistent-input");
    for (binary, args) in [
        (env!("CARGO_BIN_EXE_bench"), vec![missing]),
        (env!("CARGO_BIN_EXE_matchbench"), vec![missing]),
        (env!("CARGO_BIN_EXE_lz77"), vec!["decode", missing]),
        (env!("CARGO_BIN_EXE_lz77stats"), vec![missing]),
        (env!("CARGO_BIN_EXE_framestats"), vec![missing]),
        (env!("CARGO_BIN_EXE_lz4frame-json"), vec!["inspect", missing]),
    ] {
        let output = Command::new(binary).args(&args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{binary}");
        if binary == env!("CARGO_BIN_EXE_matchbench") {
            assert!(output.stderr.starts_with(b"matchbench: "));
        }
        assert!(!output.stderr.is_empty(), "{binary}");
    }
}
