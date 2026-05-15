#![allow(dead_code)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::{NamedTempFile, TempPath};

pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

pub fn tmpfile(content: &str) -> TempPath {
    let file = NamedTempFile::new().expect("create temp file");
    fs::write(file.path(), content).expect("write temp file contents");
    file.into_temp_path()
}

#[cfg(unix)]
pub fn chmod(path: &Path, mode: u32) {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions).expect("set permissions");
}

#[cfg(unix)]
pub fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .expect("read metadata")
        .permissions()
        .mode()
        & 0o777
}

pub fn run_linehash(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("linehash"))
        .args(args)
        .output()
        .expect("run linehash");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    let code = output.status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

pub fn run_linehash_in(cwd: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("linehash"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run linehash");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    let code = output.status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

pub fn assert_ok_contains(args: &[&str], expected: &str) {
    let (stdout, stderr, code) = run_linehash(args);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(
        stdout.contains(expected),
        "expected stdout to contain {expected:?}, got: {stdout:?}"
    );
}

pub fn assert_err_contains(args: &[&str], expected: &str) {
    let (_stdout, stderr, code) = run_linehash(args);
    assert_ne!(code, 0, "expected failure");
    assert!(
        stderr.contains(expected),
        "expected stderr to contain {expected:?}, got: {stderr:?}"
    );
}

pub fn parse_json(args: &[&str]) -> serde_json::Value {
    let (stdout, stderr, code) = run_linehash(args);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    serde_json::from_str(&stdout).expect("stdout should be valid json")
}

pub fn do_edit(content: &str, anchor: &str, new_content: &str) -> String {
    let file = tmpfile(content);
    let file_arg = file.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_linehash(&["edit", &file_arg, anchor, new_content]);
    assert_eq!(code, 0, "expected edit success, stderr: {stderr}");
    assert!(
        stdout == "Edited line 2.\n"
            || stdout == "Edited lines 2-3.\n"
            || stdout.starts_with("Edited line ")
            || stdout.starts_with("Edited lines "),
        "expected success message on stdout, got: {stdout:?}"
    );
    fs::read_to_string(&file).expect("read edited file")
}
