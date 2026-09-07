use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("sao2 e2e {label} {} {nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write_source(&self, name: &str, source: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, source).unwrap();
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn sao2(directory: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sao2"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn run_source(directory: &TestDirectory, source: &[u8]) -> Output {
    let source_path = directory.write_source("program with spaces.sao2", source);
    sao2(&directory.0, &[OsStr::new("run"), source_path.as_os_str()])
}

fn compiler_is_missing(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stderr).contains("no supported C compiler found")
}

#[test]
fn malformed_source_has_stable_diagnostic() {
    let directory = TestDirectory::new("malformed");
    let source_path = directory.write_source("bad.sao2", b"print(123);");
    let output = sao2(
        &directory.0,
        &[OsStr::new("build"), source_path.as_os_str()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.sao2:1:7: expected string literal"));
    assert!(!directory.0.join("build/program.c").exists());
}

#[test]
fn missing_configured_compiler_is_a_toolchain_error() {
    let directory = TestDirectory::new("missing compiler");
    let source_path = directory.write_source("hello.sao2", b"print(\"hello\");");
    let output = Command::new(env!("CARGO_BIN_EXE_sao2"))
        .args([OsStr::new("build"), source_path.as_os_str()])
        .current_dir(&directory.0)
        .env("SAO2_CC", directory.0.join("definitely missing compiler"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("configured C compiler"));
}

#[test]
fn failing_compiler_reports_captured_failure() {
    let directory = TestDirectory::new("failing compiler");
    let source_path = directory.write_source("hello.sao2", b"print(\"hello\");");
    let output = Command::new(env!("CARGO_BIN_EXE_sao2"))
        .args([OsStr::new("build"), source_path.as_os_str()])
        .current_dir(&directory.0)
        .env("SAO2_CC", std::env::current_exe().unwrap())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("using C compiler:"));
    assert!(stderr.contains("C compiler"));
    assert!(stderr.contains("failed with status"));
}

#[test]
fn compiles_and_runs_exact_bytes_twice_when_a_compiler_is_available() {
    let directory = TestDirectory::new("exact output");
    let source = br#"/* before */ print // between
        ("\\\"\'\n\r\t\0\x41") /* after */ ;"#;
    let expected = b"\\\"'\n\r\t\0A";

    let first = run_source(&directory, source);
    if compiler_is_missing(&first) {
        eprintln!("skipping native end-to-end assertions: no C compiler available");
        return;
    }
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, expected);

    let second = run_source(&directory, source);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(second.stdout, expected);
}

#[test]
fn runs_an_empty_string_when_a_compiler_is_available() {
    let directory = TestDirectory::new("empty output");
    let output = run_source(&directory, br#"print("");"#);
    if compiler_is_missing(&output) {
        eprintln!("skipping native end-to-end assertions: no C compiler available");
        return;
    }
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}
