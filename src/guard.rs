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

//! Running a command under a memory limit.

use std::process::{Command, ExitStatus};
use std::thread;
use std::time::Duration;

use bytesize::ByteSize;
use sysinfo::Pid;

use crate::tree::ProcessTable;
use crate::{Error, MemoryLimit};

/// How a guarded command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The command exited on its own (or was killed by someone other than the guard).
    Exited {
        /// The command's own exit status.
        status: ExitStatus,
        /// The largest resident memory of the whole tree seen at any sample; zero if it exited before the first.
        peak: ByteSize,
    },
    /// The tree's resident memory passed the limit and the guard killed every process in it.
    ///
    /// No peak is carried: the sample that crossed the limit is the largest one the guard saw.
    Killed {
        /// The tree's resident memory at the sample that crossed the limit; always above the limit.
        used: ByteSize,
    },
}

/// Runs `command` and kills its whole process tree once the tree's resident memory passes `limit`.
///
/// Every `interval` the guard checks whether the command has exited, then sums the resident memory of the command
/// and all of its descendants. A tree over the limit is killed with `SIGKILL`, every process of that sample at once,
/// and the command is reaped before this returns. Descendants still running when the command itself exits are left
/// alone. The command's stdin, stdout and stderr are whatever `command` was configured with.
///
/// # Errors
///
/// Returns [`Error::Spawn`] if the command cannot be started and [`Error::Wait`] if its status cannot be collected.
pub fn run(
    command: &mut Command,
    limit: MemoryLimit,
    interval: Duration,
) -> Result<Outcome, Error> {
    let mut child = command.spawn().map_err(|source| Error::Spawn {
        program: command.get_program().to_owned(),
        source,
    })?;
    let root = Pid::from_u32(child.id());
    let mut table = ProcessTable::new();
    let mut peak = ByteSize::b(0);
    loop {
        if let Some(status) = child.try_wait().map_err(Error::Wait)? {
            return Ok(Outcome::Exited { status, peak });
        }
        let tree = table.snapshot(root);
        peak = peak.max(tree.resident);
        if limit.is_exceeded_by(tree.resident) {
            table.kill(&tree);
            // The root is also killed through its handle in case the table missed it; an error only means it is
            // already dead.
            let _ = child.kill();
            child.wait().map_err(Error::Wait)?;
            return Ok(Outcome::Killed {
                used: tree.resident,
            });
        }
        thread::sleep(interval);
    }
}
