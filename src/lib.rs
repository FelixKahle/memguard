// Copyright (c) 2026 Felix Kahle
// SPDX-License-Identifier: MIT
//
// This file is part of memguard and is distributed under the MIT License; see the LICENSE file at the root of
// the repository for the full text.

//! Keeps runaway test suites from swapping a machine to death.
//!
//! The crate has two halves, both used by the `memguard` binary:
//!
//! - [`guard::run`] runs a command and kills its whole process tree as soon as the tree's resident memory passes a
//!   [`MemoryLimit`]. It reports an [`Outcome`]: the child's exit status and the peak seen, or the size at which the
//!   tree was killed.
//! - [`policy::Policy`] decides which shell commands must run through `memguard run`: a command that matches a
//!   "guarded" pattern is denied unless it matches an "exempt" pattern or already goes through `memguard run`. The
//!   rules are data, read from a TOML file by [`rules_file::load`].
//!
//! ```no_run
//! use std::process::Command;
//! use std::time::Duration;
//!
//! use memguard::{ByteSize, MemoryLimit, Outcome, guard};
//!
//! const LIMIT: MemoryLimit = MemoryLimit::new(ByteSize::gb(10));
//!
//! let mut command = Command::new("cargo");
//! command.arg("test");
//! match guard::run(&mut command, LIMIT, Duration::from_millis(500))? {
//!     Outcome::Exited { status, peak } => println!("{status}, peak {}", peak.display().si()),
//!     Outcome::Killed { used } => println!("killed at {}", used.display().si()),
//! }
//! # Ok::<(), memguard::Error>(())
//! ```

pub mod error;
pub mod guard;
pub mod limit;
pub mod policy;
pub mod rules_file;
mod tree;

pub use bytesize::ByteSize;
pub use error::Error;
pub use guard::Outcome;
pub use limit::MemoryLimit;
