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

//! The crate's single error type.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Everything that can go wrong in `memguard`, one variant per failure.
#[derive(Debug)]
pub enum Error {
    /// A memory limit could not be parsed as a byte size.
    LimitUnparsable {
        /// The text that was given as the limit.
        input: String,
        /// Why the byte-size parser rejected it.
        reason: String,
    },
    /// A memory limit was a bare number; the unit is required so that `10` cannot silently mean ten bytes.
    LimitWithoutUnit {
        /// The text that was given as the limit.
        input: String,
    },
    /// A memory limit was zero, which would kill every command before it starts.
    LimitZero {
        /// The text that was given as the limit.
        input: String,
    },
    /// A time limit could not be parsed as a duration with a unit.
    TimeLimitUnparsable {
        /// The text that was given as the limit.
        input: String,
        /// Why the duration parser rejected it.
        source: humantime::DurationError,
    },
    /// A time limit was zero, which would kill every command before it starts.
    TimeLimitZero {
        /// The text that was given as the limit.
        input: String,
    },
    /// The command to guard could not be started.
    Spawn {
        /// The program that was to be run.
        program: OsString,
        /// Why the operating system refused to start it.
        source: io::Error,
    },
    /// The handlers for the signals that stop or suspend the guard could not be installed.
    SignalHandler(io::Error),
    /// A thread the guard needs could not be started.
    ThreadSpawn(io::Error),
    /// The guard could not stop itself to suspend.
    Suspend(io::Error),
    /// The watchdog process could not be started.
    WatchdogSpawn(io::Error),
    /// The watchdog process could not be told about the guarded command.
    WatchdogLost(io::Error),
    /// The watchdog could not read the pipe from its guard.
    LifelineUnreadable(io::Error),
    /// The watchdog read a line from its guard that is not a message it understands.
    MessageInvalid(serde_json::Error),
    /// The status of the guarded command could not be collected.
    Wait(io::Error),
    /// The rules file could not be read.
    RulesUnreadable {
        /// The file that was to be read.
        path: PathBuf,
        /// Why reading it failed.
        source: io::Error,
    },
    /// The rules file is not valid TOML or does not have the expected keys.
    RulesInvalid {
        /// The file that was read.
        path: PathBuf,
        /// What the TOML parser rejected.
        source: toml::de::Error,
    },
    /// A pattern in the rules is not a valid regular expression.
    PatternInvalid {
        /// The pattern as written in the rules.
        pattern: String,
        /// Why the regex compiler rejected it.
        source: regex::Error,
    },
    /// The command to check could not be read from stdin.
    StdinUnreadable(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitUnparsable { input, reason } => {
                write!(f, "invalid MemoryLimit `{input}`: {reason}")
            }
            Self::LimitWithoutUnit { input } => write!(
                f,
                "invalid MemoryLimit `{input}`: a unit is required, for example `{input}G` or `{input}M`"
            ),
            Self::LimitZero { input } => {
                write!(
                    f,
                    "invalid MemoryLimit `{input}`: the limit must be greater than zero"
                )
            }
            Self::TimeLimitUnparsable { input, source } => {
                write!(f, "invalid TimeLimit `{input}`: {source}")
            }
            Self::TimeLimitZero { input } => {
                write!(
                    f,
                    "invalid TimeLimit `{input}`: the limit must be greater than zero"
                )
            }
            Self::Spawn { program, source } => {
                write!(
                    f,
                    "could not start `{}`: {source}",
                    program.to_string_lossy()
                )
            }
            Self::SignalHandler(source) => {
                write!(f, "could not install the signal handlers: {source}")
            }
            Self::ThreadSpawn(source) => write!(f, "could not start a thread: {source}"),
            Self::Suspend(source) => write!(f, "could not suspend the guard: {source}"),
            Self::WatchdogSpawn(source) => {
                write!(f, "could not start the watchdog process: {source}")
            }
            Self::WatchdogLost(source) => {
                write!(f, "could not reach the watchdog process: {source}")
            }
            Self::LifelineUnreadable(source) => {
                write!(f, "the watchdog could not read from its guard: {source}")
            }
            Self::MessageInvalid(source) => {
                write!(
                    f,
                    "the watchdog got an invalid message from its guard: {source}"
                )
            }
            Self::Wait(source) => write!(f, "could not wait for the guarded command: {source}"),
            Self::RulesUnreadable { path, source } => {
                write!(
                    f,
                    "could not read the rules file `{}`: {source}",
                    path.display()
                )
            }
            Self::RulesInvalid { path, source } => {
                write!(
                    f,
                    "the rules file `{}` is invalid: {source}",
                    path.display()
                )
            }
            Self::PatternInvalid { pattern, source } => {
                write!(
                    f,
                    "the rules pattern `{pattern}` is not a valid regex: {source}"
                )
            }
            Self::StdinUnreadable(source) => {
                write!(
                    f,
                    "could not read the command to check from stdin: {source}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}
