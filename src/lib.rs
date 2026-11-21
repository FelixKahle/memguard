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

//! Keeps runaway test suites from swapping a machine to death.
//!
//! The crate has two halves, both used by the `memguard` binary:
//!
//! - [`guard::run`] runs a command in a session of its own and kills its whole process tree as soon as it passes one
//!   of its [`Limits`]: a [`MemoryLimit`] for the tree's resident memory, optionally one for each single process, and
//!   optionally a [`TimeLimit`]. Whatever the command leaves running when it exits is killed too. It reports an
//!   [`Outcome`]: the child's exit status, the [`Breach`] for which the tree was killed, or the stop that
//!   [`guard::run_with`] was asked for through a [`Remote`]. A [`Watchdog`] process kills the tree if the guard dies
//!   before it could.
//! - [`policy::Policy`] decides which shell commands must run through `memguard run`: a command that matches a
//!   "guarded" pattern is denied unless it matches an "exempt" pattern or already goes through `memguard run`. The
//!   rules are data, read from a TOML file by [`rules_file::load`].
//!
//! ```no_run
//! use std::process::Command;
//! use std::time::Duration;
//!
//! use memguard::{Breach, ByteSize, Limits, MemoryLimit, Outcome, TimeLimit, guard};
//!
//! const LIMITS: Limits = Limits {
//!     tree: MemoryLimit::new(ByteSize::gb(6)),
//!     process: Some(MemoryLimit::new(ByteSize::gb(2))),
//!     time: Some(TimeLimit::new(Duration::from_secs(15 * 60))),
//! };
//!
//! let mut command = Command::new("cargo");
//! command.arg("test");
//! match guard::run(&mut command, LIMITS, Duration::from_millis(500))? {
//!     Outcome::Exited { status, peak, .. } => println!("{status}, peak {}", peak.display().si()),
//!     Outcome::Killed { breach: Breach::Tree, peak } => println!("killed at {}", peak.display().si()),
//!     other => println!("{other:?}"),
//! }
//!
//! // A memory limit alone guards the whole tree.
//! guard::run(&mut command, MemoryLimit::new(ByteSize::gb(10)), Duration::from_millis(500))?;
//! # Ok::<(), memguard::Error>(())
//! ```

pub mod error;
pub mod guard;
pub mod limit;
pub mod policy;
pub mod rules_file;
mod tree;
pub mod watchdog;

pub use bytesize::ByteSize;
pub use error::Error;
pub use guard::{Breach, Control, Limits, Outcome, Remote};
pub use limit::{MemoryLimit, TimeLimit};
pub use watchdog::Watchdog;
