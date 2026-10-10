// MIT License
//
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
//! must run under that guard.

use std::ffi::OsString;
use std::io::{self, ErrorKind, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode, ExitStatus};
use std::time::Duration;

use clap::{Parser, Subcommand};
use memguard::policy::Policy;
use memguard::{Error, MemoryLimit, Outcome, guard, rules_file};

/// The exit code when the guard killed the tree, as if the command had died of `SIGKILL`.
const KILLED: u8 = 137;

/// The exit code when `memguard run` itself fails, for example because the command cannot be started.
const FAILED: u8 = 1;

/// The exit code of `memguard check` for a command that must run through the guard.
const DENIED: u8 = 1;

/// The exit code of `memguard check` for unusable rules or input, the same code clap uses for bad arguments.
const USAGE: u8 = 2;

/// Runs commands under a memory limit for their whole process tree.
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
    /// Exits with the command's exit code, 128 + N if it died of signal N, or 137 if the guard killed it.
    Run {
        /// How often the tree's memory is sampled, for example `500ms` or `2s`.
        #[arg(long, default_value = "500ms", value_parser = humantime::parse_duration)]
        interval: Duration,
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
}

/// Runs the chosen subcommand; errors are printed on stderr with the subcommand's error exit code.
fn main() -> ExitCode {
    let result = match Cli::parse().action {
        Action::Run {
            interval,
            limit,
            command,
        } => run(interval, limit, &command).map_err(|error| (error, FAILED)),
        Action::Check { config, command } => check(config, command).map_err(|error| (error, USAGE)),
    };
    result.unwrap_or_else(|(error, code)| {
        eprintln!("memguard: {error}");
        ExitCode::from(code)
    })
}

/// Runs `command` under `limit` and reports the outcome on stderr.
///
/// # Errors
///
/// Returns the guard's error if the command cannot be started or waited for.
fn run(interval: Duration, limit: MemoryLimit, command: &[OsString]) -> Result<ExitCode, Error> {
    // `required = true` on the argument guarantees a program.
    let (program, args) = command
        .split_first()
        .expect("clap requires at least one command word");
    let outcome = guard::run(Command::new(program).args(args), limit, interval)?;
    Ok(match outcome {
        Outcome::Exited { status, peak } => {
            eprintln!("memguard: peak {}", peak.display().si());
            exit_code(status)
        }
        Outcome::Killed { used } => {
            eprintln!(
                "memguard: killed the tree at {} (limit {})",
                used.display().si(),
                limit.size().display().si()
            );
            ExitCode::from(KILLED)
        }
    })
}

/// The shell's exit code for `status`: the code itself, or 128 + the signal that ended the process.
fn exit_code(status: ExitStatus) -> ExitCode {
    let code = status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal));
    // Unix exit codes fit in a byte; anything else is reported as plain failure.
    code.and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from)
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
