// Copyright (c) 2026 Felix Kahle
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! `memguard check` against the built binary.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The example rules file shipped in the repository.
fn example_rules() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("rules.toml")
}

/// Runs `memguard check` with `args`, `HOME` set to `home` if given, and `stdin` on its stdin.
fn memguard_check(args: &[&str], home: Option<&Path>, stdin: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_memguard"));
    command
        .arg("check")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = home {
        command.env("HOME", home);
    }
    let mut child = command.spawn().expect("the memguard binary starts");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(stdin.as_bytes())
        .expect("stdin is written");
    child.wait_with_output().expect("memguard check finishes")
}

/// `memguard check` with the example rules, judging `command` given as an argument.
fn check_argument(command: &str) -> Output {
    let rules = example_rules();
    memguard_check(
        &["--config", rules.to_str().expect("UTF-8 path"), command],
        None,
        "",
    )
}

/// `memguard check` with the example rules, judging `stdin`.
fn check_stdin(stdin: &str) -> Output {
    let rules = example_rules();
    memguard_check(
        &["--config", rules.to_str().expect("UTF-8 path")],
        None,
        stdin,
    )
}

/// The command's stderr as text.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A denied command exits 1 with the substituted reason on stderr and nothing on stdout.
#[test]
fn denies_an_unguarded_argument() {
    let output = check_argument("cargo test");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).starts_with("`cargo test` runs tests"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("memguard run 10G cargo test"),
        "{}",
        stderr(&output)
    );
}

/// A command on stdin is judged the same way, without its trailing newline.
#[test]
fn denies_an_unguarded_command_on_stdin() {
    let output = check_stdin("cargo test\n");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).starts_with("`cargo test` runs tests"),
        "{}",
        stderr(&output)
    );
}

/// An allowed command exits 0 and prints nothing, in both forms.
#[test]
fn allows_guarded_and_build_only_commands() {
    for output in [
        check_argument("memguard run 10G cargo test"),
        check_stdin("cargo test --no-run\n"),
    ] {
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}

/// A named rules file that does not exist is a usage error.
#[test]
fn fails_on_a_missing_named_rules_file() {
    let output = memguard_check(
        &["--config", "/nonexistent/rules.toml", "cargo test"],
        None,
        "",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("/nonexistent/rules.toml"),
        "{}",
        stderr(&output)
    );
}

/// A rules file with an invalid pattern is a usage error that names the pattern.
#[test]
fn fails_on_an_invalid_pattern() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("invalid-rules");
    fs::create_dir_all(&dir).expect("the rules directory is created");
    let rules = dir.join("rules.toml");
    fs::write(&rules, "guarded = ['(unclosed']\nreason = 'no'\n")
        .expect("the rules file is written");
    let output = memguard_check(
        &["--config", rules.to_str().expect("UTF-8 path"), "ls"],
        None,
        "",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("`(unclosed`"),
        "{}",
        stderr(&output)
    );
}

/// Without a default rules file every command is allowed, and one stderr line says why.
#[test]
fn allows_everything_without_a_default_rules_file() {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("empty-home");
    fs::create_dir_all(&home).expect("the empty home is created");
    let output = memguard_check(&["cargo test"], Some(&home), "");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    let expected = home.join(".config/memguard/rules.toml");
    assert_eq!(
        stderr(&output),
        format!(
            "memguard: no rules file found at {}; allowing every command\n",
            expected.display()
        )
    );
}
