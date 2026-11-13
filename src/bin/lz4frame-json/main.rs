//! Converts an LZ4 frame stream to JSON that
//! describes its physical structure (`inspect`), and the JSON back to the
//! identical bytes (`restore`).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

use lz4opt::lz4::framejson;

#[path = "../common/diagnostics.rs"]
mod diagnostics;

#[cfg(test)]
mod tests;

const USAGE: &str = "usage: lz4frame-json <inspect|restore> [args...]\n";
const INSPECT_USAGE: &str = "usage: lz4frame-json inspect [input.lz4|-]\n";
const RESTORE_USAGE: &str = "usage: lz4frame-json restore [input.json|-] [output.lz4|-]\n";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout_is_char_device = stdout_is_char_device();
    let code =
        run(&args, &mut io::stdin().lock(), &mut io::stdout().lock(), &mut io::stderr().lock(), stdout_is_char_device);
    process::exit(code);
}

/// Returns true if standard output is a character device (a terminal, in the
/// common case), which `restore` refuses to write binary data to.
#[cfg(unix)]
fn stdout_is_char_device() -> bool {
    use std::os::fd::AsFd;
    use std::os::unix::fs::FileTypeExt;
    let Ok(fd) = io::stdout().as_fd().try_clone_to_owned() else { return false };
    // Stat failing is not this check's concern; let the eventual write report
    // whatever is actually wrong with stdout.
    File::from(fd).metadata().is_ok_and(|m| m.file_type().is_char_device())
}

#[cfg(not(unix))]
fn stdout_is_char_device() -> bool {
    false
}

/// Runs the command with the given arguments and streams, returning the exit
/// code: 0 on success, 1 on an I/O error, an invalid LZ4 frame, or unrestorable
/// JSON, and 2 on a usage error. `stdout_is_char_device` says whether `stdout`
/// is a character device, so tests can supply any writer.
fn run(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdout_is_char_device: bool,
) -> i32 {
    let Some(command) = args.first() else {
        return diagnostics::report(stderr, USAGE.trim_end(), diagnostics::USAGE_ERROR);
    };
    match command.as_str() {
        "inspect" => run_inspect(&args[1..], stdin, stdout, stderr),
        "restore" => run_restore(&args[1..], stdin, stdout, stderr, stdout_is_char_device),
        _ => diagnostics::report(stderr, USAGE.trim_end(), diagnostics::USAGE_ERROR),
    }
}

fn report(stderr: &mut dyn Write, err: impl std::fmt::Display) -> i32 {
    diagnostics::report(stderr, format!("lz4frame-json: {err}"), diagnostics::RUNTIME_ERROR)
}

/// Runs `lz4frame-json inspect [input.lz4|-]`. It parses the input (`args[0]`,
/// or stdin when omitted or "-") as an LZ4 frame stream and, only once parsing
/// succeeds in full, writes indented JSON to stdout in a single write.
/// On a parse failure it reports to stderr and never attempts to write to
/// stdout; a failure from the stdout write itself is also reported to stderr,
/// though by then bytes may already be in flight to whatever stdout is
/// connected to.
fn run_inspect(args: &[String], stdin: &mut dyn Read, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args.len() > 1 {
        return diagnostics::report(stderr, INSPECT_USAGE.trim_end(), diagnostics::USAGE_ERROR);
    }

    let mut input = match open_input(args, stdin) {
        Ok(input) => input,
        Err(err) => return report(stderr, err),
    };
    let root = match framejson::parse(&mut input) {
        Ok(root) => root,
        Err(err) => return report(stderr, err),
    };

    if let Err(err) = stdout.write_all(root.to_json().as_bytes()) {
        return report(stderr, err);
    }
    0
}

/// Runs `lz4frame-json restore [input.json|-] [output.lz4|-]`. It reads
/// and validates a restore JSON document, and only once that fully succeeds
/// delivers the reconstructed LZ4 bytes to the resolved destination --
/// atomically replacing an output file, or writing to stdout after confirming
/// stdout is not a terminal. Positional args: 0 args is stdin input and stdout
/// output; 1 arg is file input (`args[0]`) and stdout output; 2 args give both
/// input and output explicitly, and either one may be "-" to select stdin or
/// stdout instead of a file. A parse/schema/layout failure or a destination
/// write failure is reported to stderr with exit code 1; more than two
/// positional args is a usage error with exit code 2. On any failure an
/// existing output file is left untouched and no temporary file remains.
fn run_restore(
    args: &[String],
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    stdout_is_char_device: bool,
) -> i32 {
    if args.len() > 2 {
        return diagnostics::report(stderr, RESTORE_USAGE.trim_end(), diagnostics::USAGE_ERROR);
    }

    let output_path = restore_output_target(args);
    if output_path.is_none() && stdout_is_char_device {
        return report(stderr, "refusing to write binary LZ4 data to a terminal; redirect stdout to a file or pipe");
    }

    let mut input = match open_input(args, stdin) {
        Ok(input) => input,
        Err(err) => return report(stderr, err),
    };

    let mut buf = Vec::new();
    if let Err(err) = framejson::restore(&mut buf, &mut input) {
        return report(stderr, err);
    }

    match output_path {
        Some(path) => match write_file_atomic(Path::new(path), &buf) {
            Ok(()) => 0,
            Err(err) => report(stderr, err),
        },
        None => match stdout.write_all(&buf) {
            Ok(()) => 0,
            Err(err) => report(stderr, err),
        },
    }
}

/// Resolves restore's optional output positional argument (`args[1]`, when
/// present) to a destination: `None` (stdout) when `args` has fewer than two
/// elements or `args[1]` is "-", the named file otherwise.
fn restore_output_target(args: &[String]) -> Option<&str> {
    match args.get(1).map(String::as_str) {
        None | Some("-") => None,
        Some(path) => Some(path),
    }
}

/// Resolves a subcommand's optional positional input argument (`args[0]`, when
/// present) to a reader: stdin when `args` is empty or `args[0]` is "-", the
/// named file otherwise. Shared by inspect and restore so both agree on "-"
/// meaning stdin.
fn open_input<'a>(args: &[String], stdin: &'a mut dyn Read) -> Result<Box<dyn Read + 'a>, String> {
    match args.first().map(String::as_str) {
        None | Some("-") => Ok(Box::new(stdin)),
        Some(path) => {
            File::open(path).map(|f| Box::new(f) as Box<dyn Read>).map_err(|err| path_error("open", path, &err))
        }
    }
}

/// Formats an I/O error as `op path: message`, with the message in lower case
/// and without the OS error number.
fn path_error(op: &str, path: &str, err: &io::Error) -> String {
    format!("{op} {path}: {}", error_message(err))
}

fn error_message(err: &io::Error) -> String {
    let mut msg = err.to_string();
    if let Some(i) = msg.rfind(" (os error ") {
        msg.truncate(i);
    }
    let mut chars = msg.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => msg,
    }
}

/// Writes `data` to `path` without ever leaving a partially written or
/// truncated file there: it creates a temporary file in `path`'s directory,
/// writes `data`, and closes it, and renames the temporary file over `path`
/// only once both succeed. Any failure along the way attempts to remove the
/// temporary file before returning, and leaves an existing file at `path`
/// untouched; if the removal itself also fails, that error is reported
/// alongside the original one rather than discarded, since it means a
/// temporary file was left behind.
fn write_file_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let base = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let (mut tmp, tmp_path) = create_temp(dir, &format!(".{base}.tmp-"))
        .map_err(|err| path_error("open", &dir.join(format!(".{base}.tmp-*")).to_string_lossy(), &err))?;

    let result = tmp
        .write_all(data)
        .and_then(|()| tmp.flush())
        .map_err(|err| error_message(&err))
        .and_then(|()| {
            drop(tmp);
            set_mode_0644(&tmp_path).map_err(|err| path_error("chmod", &tmp_path.to_string_lossy(), &err))
        })
        .and_then(|()| {
            fs::rename(&tmp_path, path)
                .map_err(|err| format!("rename {} {}: {}", tmp_path.display(), path.display(), error_message(&err)))
        });
    result.map_err(|cause| cleanup_temp_file(&tmp_path, cause))
}

/// Removes `tmp_path` and returns `cause`, joined with the removal error if
/// that also fails, so a left-behind temporary file is reported rather than
/// silently swallowed.
fn cleanup_temp_file(tmp_path: &Path, cause: String) -> String {
    match fs::remove_file(tmp_path) {
        Ok(()) => cause,
        // The rename may have succeeded in moving it already.
        Err(err) if err.kind() == io::ErrorKind::NotFound && !tmp_path.exists() => cause,
        Err(err) => format!("{cause}\nremoving temporary file {}: {}", tmp_path.display(), error_message(&err)),
    }
}

/// Creates a new file named `prefix` followed by a unique suffix in `dir`.
fn create_temp(dir: &Path, prefix: &str) -> io::Result<(File, PathBuf)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    for _ in 0..10_000 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{prefix}{}{nanos:09}{n}", process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not create a unique temporary file"))
}

#[cfg(unix)]
fn set_mode_0644(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn set_mode_0644(_: &Path) -> io::Result<()> {
    Ok(())
}
