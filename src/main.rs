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

//! The `memguard` command line: `memguard run` guards a command's memory, `memguard check` says whether a command
//! must run under that guard, and the hidden `memguard watchdog` backs up a running guard.

use std::env;
use std::ffi::OsString;
use std::io::{self, ErrorKind, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode, ExitStatus};
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use memguard::policy::Policy;
use memguard::{
    Breach, ByteSize, Error, Limits, MemoryLimit, Outcome, Remote, TimeLimit, Watchdog, guard,
    rules_file, watchdog,
};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGTSTP};
use signal_hook::iterator::Signals;
use signal_hook::low_level::signal_name;

/// The exit code when the guard killed the tree, as if the command had died of `SIGKILL`.
const KILLED: u8 = 137;

/// The signals that make `memguard run` kill the tree and exit, instead of ending the guard and leaving the tree
/// running without it. `SIGTSTP` suspends the tree with the guard instead.
const STOP_SIGNALS: [i32; 4] = [SIGTERM, SIGINT, SIGHUP, SIGQUIT];

/// The exit code when `memguard run` itself fails, for example because the command cannot be started.
const FAILED: u8 = 1;

/// The exit code of `memguard check` for a command that must run through the guard.
const DENIED: u8 = 1;

/// The exit code of `memguard check` for unusable rules or input, the same code clap uses for bad arguments.
const USAGE: u8 = 2;

/// Runs commands under memory and time limits for their whole process tree.
#[derive(Debug, Parser)]
#[command(name = "memguard", version, about)]
struct Cli {
    /// What to do.
    #[command(subcommand)]
    action: Action,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
enum Action {
    /// Run a command and kill its whole process tree once the tree's resident memory passes the limit.
    ///
    /// The tree is the command, which leads a session of its own, every process of that session, and every
    /// descendant of any of them. With `--per-process`, the tree is also killed once any one process in it passes
    /// that size; with `--timeout`, once the command has run for longer than that. Whatever the command leaves
    /// running when it exits is killed. SIGTERM, SIGINT, SIGHUP or SIGQUIT sent to memguard kill the tree, SIGTSTP
    /// suspends it with memguard, and a watchdog process kills it if memguard dies of anything else.
    ///
    /// Exits with the command's exit code, 128 + N if it died of signal N or memguard received signal N, or 137 if
    /// the guard killed it for passing a limit.
    Run {
        /// How often the tree's memory is sampled, for example `500ms` or `2s`.
        #[arg(long, default_value = "500ms", value_parser = humantime::parse_duration)]
        interval: Duration,
        /// Also kill the tree once any single process in it holds more than this, for example `2G`.
        #[arg(long, value_name = "SIZE")]
        per_process: Option<MemoryLimit>,
        /// Also kill the tree once the command has run longer than this, for example `90s`, `15m` or `1h 30m`.
        #[arg(long, value_name = "DURATION")]
        timeout: Option<TimeLimit>,
        /// The limit for the whole tree, with a unit: `10G`, `512M`, `10GB`, `1.5GiB`.
        limit: MemoryLimit,
        /// The command to run, followed by its arguments.
        #[arg(
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "COMMAND"
        )]
        command: Vec<OsString>,
    },
    /// Check whether a command must run through `memguard run`, by the rules in a rules file.
    ///
    /// Exits 0 if the command may run as it is, 1 (with the reason on stderr) if it must run through the guard, and
    /// 2 if the rules cannot be used.
    Check {
        /// The rules file; defaults to `~/.config/memguard/rules.toml`, and without it every command is allowed.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
        /// The command to check; read from stdin (one trailing newline removed) when omitted.
        command: Option<String>,
    },
    /// Kill what is left of a guarded command if its guard dies; started by `memguard run`, with the guard on stdin.
    #[command(hide = true)]
    Watchdog,
}

/// Runs the chosen subcommand; errors are printed on stderr with the subcommand's error exit code.
fn main() -> ExitCode {
    let result = match Cli::parse().action {
        Action::Run {
            interval,
            per_process,
            timeout,
            limit,
            command,
        } => {
            let limits = Limits {
                tree: limit,
                process: per_process,
                time: timeout,
            };
            run(interval, limits, &command).map_err(|error| (error, FAILED))
        }
        Action::Check { config, command } => check(config, command).map_err(|error| (error, USAGE)),
        Action::Watchdog => watch().map_err(|error| (error, FAILED)),
    };
    result.unwrap_or_else(|(error, code)| {
        eprintln!("memguard: {error}");
        ExitCode::from(code)
    })
}

/// Runs `command` under `limits`, backed by a watchdog and driven by signals, and reports the outcome on stderr.
///
/// # Errors
///
/// Returns an error if the signal handlers, the watchdog or the command cannot be set up, or the command cannot be
/// waited for.
fn run(interval: Duration, limits: Limits, command: &[OsString]) -> Result<ExitCode, Error> {
    // `required = true` on the argument guarantees a program.
    let (program, args) = command
        .split_first()
        .expect("clap requires at least one command word");
    // The handlers and the watchdog come first, so that from the moment the command runs nothing can end the guard
    // without the tree.
    let signals =
        Signals::new(STOP_SIGNALS.into_iter().chain([SIGTSTP])).map_err(Error::SignalHandler)?;
    let handle = signals.handle();
    let watchdog = Watchdog::spawn(
        Command::new(env::current_exe().map_err(Error::WatchdogSpawn)?).arg("watchdog"),
    )?;
    let (remote, control) = guard::control(Some(watchdog));
    thread::Builder::new()
        .name("memguard-signals".to_owned())
        .spawn(move || forward(signals, &remote))
        .map_err(Error::ThreadSpawn)?;
    let outcome = guard::run_with(Command::new(program).args(args), limits, interval, control);
    handle.close();
    Ok(report(outcome?, limits))
}

/// Turns each signal that arrives into a request to the guard, until the signals are closed.
fn forward(mut signals: Signals, remote: &Remote) {
    for signal in signals.forever() {
        if signal == SIGTSTP {
            remote.suspend();
        } else {
            remote.stop(signal);
        }
    }
}

/// Prints the outcome on stderr and returns the exit code it stands for.
fn report(outcome: Outcome, limits: Limits) -> ExitCode {
    match outcome {
        Outcome::Exited {
            status,
            peak,
            leftovers,
        } => {
            let peak = peak.display().si();
            match leftovers {
                0 => eprintln!("memguard: peak {peak}"),
                1 => eprintln!("memguard: peak {peak}; killed 1 process the command left running"),
                n => eprintln!(
                    "memguard: peak {peak}; killed {n} processes the command left running"
                ),
            }
            exit_code(status)
        }
        Outcome::Killed { breach, peak } => {
            eprintln!("memguard: {}", killed_message(&breach, peak, limits));
            ExitCode::from(KILLED)
        }
        Outcome::Stopped { signal, peak } => {
            let name = signal_name(signal).unwrap_or("a signal");
            eprintln!(
                "memguard: received {name}, killed the tree; peak {}",
                peak.display().si()
            );
            byte_code(128 + signal)
        }
    }
}

/// Watches the guard on stdin and kills what is left of its command if it dies; says on stderr what it killed.
///
/// # Errors
///
/// Returns the watchdog's error if stdin cannot be read or holds something other than messages from a guard.
fn watch() -> Result<ExitCode, Error> {
    let killed = watchdog::watch(io::stdin().lock())?;
    if killed > 0 {
        eprintln!(
            "memguard: the guard ended without cleaning up; the watchdog killed {killed} processes it left"
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The line saying why the guard killed the tree, without the `memguard: ` prefix.
fn killed_message(breach: &Breach, peak: ByteSize, limits: Limits) -> String {
    let size = |size: ByteSize| size.display().si().to_string();
    match breach {
        Breach::Tree => format!(
            "killed the tree at {} (limit {})",
            size(peak),
            size(limits.tree.size())
        ),
        Breach::Process { pid, name, used } => format!(
            "killed the tree: process {pid} ({}) held {} (per-process limit {}); tree peak {}",
            name.to_string_lossy(),
            size(*used),
            limits
                .process
                .map_or_else(String::new, |limit| size(limit.size())),
            size(peak)
        ),
        Breach::Time { elapsed } => format!(
            "killed the tree after {} (timeout {}); peak {}",
            humantime::format_duration(whole_milliseconds(*elapsed)),
            humantime::format_duration(limits.time.map_or(Duration::ZERO, TimeLimit::duration)),
            size(peak)
        ),
    }
}

/// `duration` truncated to whole milliseconds, so that it prints as `1s 2ms` rather than to the nanosecond.
fn whole_milliseconds(duration: Duration) -> Duration {
    Duration::from_secs(duration.as_secs())
        + Duration::from_millis(u64::from(duration.subsec_millis()))
}

/// The shell's exit code for `status`: the code itself, or 128 + the signal that ended the process.
fn exit_code(status: ExitStatus) -> ExitCode {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .map_or(ExitCode::FAILURE, byte_code)
}

/// `code` as an exit code; Unix exit codes fit in a byte, and anything else is reported as plain failure.
fn byte_code(code: i32) -> ExitCode {
    u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from)
}

/// Checks `command` (or stdin) against the rules and prints the reason on stderr if it is denied.
///
/// Without `config`, a missing default rules file allows everything and says so on stderr.
///
/// # Errors
///
/// Returns an error if the rules cannot be loaded or stdin cannot be read.
fn check(config: Option<PathBuf>, command: Option<String>) -> Result<ExitCode, Error> {
    let Some(policy) = load_policy(config)? else {
        return Ok(ExitCode::SUCCESS);
    };
    let command = match command {
        Some(command) => command,
        None => read_command()?,
    };
    Ok(match policy.decide(&command) {
        Some(denial) => {
            eprintln!("{}", denial.reason);
            ExitCode::from(DENIED)
        }
        None => ExitCode::SUCCESS,
    })
}

/// The policy from `config`, or from the default rules file; `None` if no rules file was named and none exists.
///
/// # Errors
///
/// Returns an error if a named rules file is missing, or any rules file is unreadable or invalid.
fn load_policy(config: Option<PathBuf>) -> Result<Option<Policy>, Error> {
    if let Some(path) = config {
        return rules_file::load(path).map(Some);
    }
    let Some(path) = rules_file::default_path() else {
        eprintln!("memguard: no home directory, so no rules file; allowing every command");
        return Ok(None);
    };
    match rules_file::load(&path) {
        Ok(policy) => Ok(Some(policy)),
        Err(Error::RulesUnreadable { source, .. }) if source.kind() == ErrorKind::NotFound => {
            eprintln!(
                "memguard: no rules file found at {}; allowing every command",
                path.display()
            );
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The whole of stdin, without one trailing newline.
///
/// # Errors
///
/// Returns [`Error::StdinUnreadable`] if stdin cannot be read as UTF-8 text.
fn read_command() -> Result<String, Error> {
    let mut command = String::new();
    io::stdin()
        .read_to_string(&mut command)
        .map_err(Error::StdinUnreadable)?;
    if command.ends_with('\n') {
        command.pop();
        if command.ends_with('\r') {
            command.pop();
        }
    }
    Ok(command)
}
