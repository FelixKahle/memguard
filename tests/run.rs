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

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

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

/// One process over the per-process limit is killed even when the tree limit is far away, and is named.
#[test]
fn kills_a_process_over_the_per_process_limit() {
    let output = memguard_run(&[
        "--interval",
        "50ms",
        "--per-process",
        "100M",
        "10G",
        "perl",
        "-e",
        HOG,
    ]);
    assert_eq!(output.status.code(), Some(137), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("(perl) held"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("(per-process limit 100.0 MB)"),
        "{}",
        stderr(&output)
    );
}

/// Two processes that are each under the per-process limit are killed by the tree limit instead.
#[test]
fn per_process_limit_leaves_the_tree_limit_in_force() {
    let script = format!("perl -e '{HOG}' & perl -e '{HOG}'; wait");
    let output = memguard_run(&[
        "--interval",
        "50ms",
        "--per-process",
        "1G",
        "500M",
        "sh",
        "-c",
        &script,
    ]);
    assert_eq!(output.status.code(), Some(137), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("killed the tree at"),
        "{}",
        stderr(&output)
    );
}

/// A command that runs past the timeout is killed soon after it, not at the next full interval.
#[test]
fn kills_a_command_over_the_timeout() {
    let started = Instant::now();
    let output = memguard_run(&[
        "--interval",
        "5s",
        "--timeout",
        "300ms",
        "1G",
        "sleep",
        "10",
    ]);
    assert_eq!(output.status.code(), Some(137), "{}", stderr(&output));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    assert!(
        stderr(&output).contains("(timeout 300ms)"),
        "{}",
        stderr(&output)
    );
}

/// A command that finishes within the timeout keeps its exit code.
#[test]
fn a_command_within_the_timeout_keeps_its_exit_code() {
    let output = memguard_run(&["--timeout", "10s", "1G", "sh", "-c", "exit 3"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
}

/// A zero timeout is rejected before anything runs.
#[test]
fn rejects_a_zero_timeout() {
    let output = memguard_run(&["--timeout", "0s", "1G", "sh", "-c", "exit 0"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("TimeLimit"), "{}", stderr(&output));
}

/// A running `memguard run` whose command is `sh -c`, which prints its own process id first: the id of the
/// session the guard puts it in.
struct Guarded {
    /// The guard.
    guard: Child,
    /// The command's stdout, after the first line.
    stdout: BufReader<ChildStdout>,
    /// The command's process id, which is also its session id.
    session: u32,
}

impl Guarded {
    /// Starts `memguard run` with `args`, then `sh -c` running `script` after printing its process id.
    fn start(args: &[&str], script: &str) -> Self {
        let mut guard = Command::new(env!("CARGO_BIN_EXE_memguard"))
            .arg("run")
            .args(args)
            .args(["sh", "-c", &format!("echo $$; {script}")])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the memguard binary starts");
        let mut stdout = BufReader::new(guard.stdout.take().expect("stdout is piped"));
        let session = read_line(&mut stdout)
            .parse()
            .expect("the command prints its pid");
        Self {
            guard,
            stdout,
            session,
        }
    }

    /// Sends `signal`, such as `-TERM`, to the guard.
    fn signal(&self, signal: &str) {
        send(signal, self.guard.id());
    }

    /// Waits for the guard to exit and returns its exit code.
    fn wait(&mut self) -> Option<i32> {
        self.guard.wait().expect("the guard exits").code()
    }

    /// The guard's stderr, read to its end.
    ///
    /// Call it only after [`Guarded::wait`] and after checking what is left of the session: a process left holding
    /// stderr open blocks this until it exits.
    fn stderr(mut self) -> String {
        let mut stderr = String::new();
        self.guard
            .stderr
            .take()
            .expect("stderr is piped")
            .read_to_string(&mut stderr)
            .expect("stderr is text");
        stderr
    }
}

/// The next line of `reader`, without its line break.
fn read_line(reader: &mut impl BufRead) -> String {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("the command prints a line");
    line.trim().to_owned()
}

/// Sends `signal`, such as `-TERM`, to `pid`.
fn send(signal: &str, pid: u32) {
    let status = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .expect("kill starts");
    assert!(status.success(), "kill {signal} {pid} failed");
}

/// The live (not zombie) processes of the session `session`, and the state of each.
fn session_members(session: u32) -> Vec<(u32, ProcessStatus)> {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    system
        .processes()
        .values()
        .filter(|process| process.thread_kind().is_none())
        .filter(|process| process.session_id() == Some(Pid::from_u32(session)))
        .filter(|process| process.status() != ProcessStatus::Zombie)
        .map(|process| (process.pid().as_u32(), process.status()))
        .collect()
}

/// Waits up to five seconds for `done` to hold, and says whether it did.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// Asserts that no live process of the session `session` is left; a killed one may take a moment to go.
fn assert_session_gone(session: u32) {
    assert!(
        eventually(|| session_members(session).is_empty()),
        "session {session} still has {:?}",
        session_members(session)
    );
}

/// A guard that is sent SIGTERM kills the whole tree at once, even with a long interval, and exits as if it had died
/// of SIGTERM.
#[test]
fn sigterm_to_the_guard_kills_the_tree_at_once() {
    let mut guarded = Guarded::start(&["--interval", "10s", "1G"], "sleep 30 & wait");
    let session = guarded.session;
    let sent = Instant::now();
    guarded.signal("-TERM");
    let code = guarded.wait();
    let waited = sent.elapsed();
    assert_session_gone(session);
    let stderr = guarded.stderr();
    assert!(waited < Duration::from_secs(3), "took {waited:?}");
    assert_eq!(code, Some(128 + 15), "{stderr}");
    assert!(
        stderr.contains("received SIGTERM, killed the tree"),
        "{stderr}"
    );
}

/// The same holds for SIGINT: Ctrl-C reaches only the guard, as the command has a session of its own.
#[test]
fn sigint_to_the_guard_kills_the_tree() {
    let mut guarded = Guarded::start(&["1G"], "sleep 30 & wait");
    let session = guarded.session;
    guarded.signal("-INT");
    let code = guarded.wait();
    assert_session_gone(session);
    let stderr = guarded.stderr();
    assert_eq!(code, Some(128 + 2), "{stderr}");
    assert!(stderr.contains("received SIGINT"), "{stderr}");
}

/// A guard killed with SIGKILL cannot clean up, so its watchdog kills the tree.
#[test]
fn sigkill_to_the_guard_leaves_nothing() {
    let mut guarded = Guarded::start(&["1G"], "sleep 30 & sleep 30");
    let session = guarded.session;
    guarded.signal("-KILL");
    let code = guarded.wait();
    assert_session_gone(session);
    let stderr = guarded.stderr();
    assert_eq!(code, None, "{stderr}");
    assert!(stderr.contains("the watchdog killed"), "{stderr}");
}

/// A hog whose parent exited is still in the command's session, so it counts and is killed.
#[test]
fn an_orphaned_hog_is_still_guarded() {
    // The subshell exits as soon as it has started perl, so perl loses its parent before the first sample.
    let script = format!("(perl -e '{HOG}' &); sleep 10");
    let output = memguard_run(&["--interval", "50ms", "100M", "sh", "-c", &script]);
    assert_eq!(output.status.code(), Some(137), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("killed the tree at"),
        "{}",
        stderr(&output)
    );
}

/// What the command leaves running when it exits is killed, and the guard says so.
#[test]
fn kills_what_the_command_leaves_running() {
    let mut guarded = Guarded::start(&["1G"], "sleep 30 &");
    let session = guarded.session;
    let code = guarded.wait();
    assert_session_gone(session);
    let stderr = guarded.stderr();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("killed 1 process the command left running"),
        "{stderr}"
    );
}

/// The guard returns as soon as the command exits, not at the next sample.
#[test]
fn notices_an_exit_at_once() {
    let started = Instant::now();
    let output = memguard_run(&["--interval", "10s", "1G", "true"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
}

/// A command that forks as fast as it can is frozen before it is killed, so no child it forks during the kill
/// survives.
#[test]
fn a_fork_loop_leaves_no_survivors() {
    let mut guarded = Guarded::start(
        &["--timeout", "300ms", "10G"],
        "while :; do sleep 30 & done",
    );
    let session = guarded.session;
    let code = guarded.wait();
    assert_session_gone(session);
    let stderr = guarded.stderr();
    assert_eq!(code, Some(137), "{stderr}");
}

/// SIGTSTP stops the tree together with the guard, and continuing the guard continues the tree.
#[test]
fn suspends_and_resumes_the_tree() {
    let mut guarded = Guarded::start(&["1G"], "sleep 30 & echo $!; wait");
    let sleeper = read_line(&mut guarded.stdout)
        .parse::<u32>()
        .expect("the command prints its child's pid");
    let session = guarded.session;
    let state = |pid: u32| {
        session_members(session)
            .into_iter()
            .find_map(|(member, status)| (member == pid).then_some(status))
    };
    guarded.signal("-TSTP");
    assert!(eventually(|| state(session) == Some(ProcessStatus::Stop)
        && state(sleeper) == Some(ProcessStatus::Stop)));
    guarded.signal("-CONT");
    assert!(eventually(
        || state(sleeper).is_some_and(|status| status != ProcessStatus::Stop)
    ));
    guarded.signal("-TERM");
    let code = guarded.wait();
    assert_session_gone(session);
    let stderr = guarded.stderr();
    assert_eq!(code, Some(128 + 15), "{stderr}");
}
