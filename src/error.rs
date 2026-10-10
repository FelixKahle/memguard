// Copyright (c) 2026 Felix Kahle
// SPDX-License-Identifier: MIT
//
// This file is part of memguard and is distributed under the MIT License; see the LICENSE file at the root of
// the repository for the full text.

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
    /// The command to guard could not be started.
    Spawn {
        /// The program that was to be run.
        program: OsString,
        /// Why the operating system refused to start it.
        source: io::Error,
    },
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
            Self::Spawn { program, source } => {
                write!(
                    f,
                    "could not start `{}`: {source}",
                    program.to_string_lossy()
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
