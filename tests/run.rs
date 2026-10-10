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

//! `memguard run` against the built binary.

use std::process::{Command, Output};

/// A process that holds about 300 MB of resident memory for ten seconds, unless it is killed first.
const HOG: &str = r#"$x = "a" x 300_000_000; sleep 10"#;

/// Runs `memguard run` with `args` and returns what it did.
fn memguard_run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_memguard"))
        .arg("run")
        .args(args)
        .output()
        .expect("the memguard binary starts")
}

/// The guard's stderr as text.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A command that stays under the limit keeps its exit code, and the peak is reported.
#[test]
fn passes_the_exit_code_through() {
    let output = memguard_run(&["1G", "sh", "-c", "exit 3"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("memguard: peak"),
        "{}",
        stderr(&output)
    );
}

/// A command that dies of a signal exits with 128 + the signal number.
#[test]
fn reports_a_signal_as_128_plus_it() {
    let output = memguard_run(&["1G", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(output.status.code(), Some(128 + 15), "{}", stderr(&output));
}

/// A command over the limit is killed and the guard exits with 137.
#[test]
fn kills_a_hog() {
    let output = memguard_run(&["--interval", "50ms", "100M", "perl", "-e", HOG]);
    assert_eq!(output.status.code(), Some(137), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("memguard: killed the tree at"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("(limit 100.0 MB)"),
        "{}",
        stderr(&output)
    );
}

/// A hog that is a grandchild of the guard is found by the tree walk and killed.
#[test]
fn kills_a_grandchild_hog() {
    // The trailing `exit 0` keeps the shell alive as the hog's parent instead of letting it exec into perl.
    let script = format!("perl -e '{HOG}'; exit 0");
    let output = memguard_run(&["--interval", "50ms", "100M", "sh", "-c", &script]);
    assert_eq!(output.status.code(), Some(137), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("killed the tree"),
        "{}",
        stderr(&output)
    );
}

/// A limit without a unit is rejected before anything runs.
#[test]
fn rejects_a_limit_without_a_unit() {
    let output = memguard_run(&["10", "sh", "-c", "exit 0"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("a unit is required"),
        "{}",
        stderr(&output)
    );
}
