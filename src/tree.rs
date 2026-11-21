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

//! A guarded command's processes, read from the operating system's process table, and stopping, continuing or
//! killing all of them at once.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::iter;

use bytesize::ByteSize;
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

/// How many times [`ProcessTable::freeze`] reads the table before it stops chasing a tree that keeps growing.
///
/// A stopped process cannot fork, so a tree whose processes can all be stopped is complete after a few reads. Only a
/// process that refuses `SIGSTOP`, such as one that belongs to another user, can keep adding children.
const FREEZE_READS: usize = 64;

/// One process of a [`TreeSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Member {
    /// The process.
    pub(crate) pid: Pid,
    /// Its resident memory; zero if it was missing from the table.
    pub(crate) resident: ByteSize,
}

/// A guarded command's processes at one moment: the command, every process of the session it leads, and every
/// descendant of any of them.
///
/// The session catches what a walk down parent links misses: a process whose parent exited is handed to another
/// parent, but stays in its session unless it starts one of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeSnapshot {
    /// The command; present even if it was missing from the table.
    pub(crate) root: Member,
    /// Every other process of the tree exactly once.
    pub(crate) others: Vec<Member>,
}

impl TreeSnapshot {
    /// Every process of the tree exactly once, the root first.
    pub(crate) fn members(&self) -> impl Iterator<Item = &Member> {
        iter::once(&self.root).chain(&self.others)
    }

    /// The resident memory of every process of the tree together, saturating at `u64::MAX` bytes.
    pub(crate) fn resident(&self) -> ByteSize {
        let total = self.members().fold(0_u64, |total, member| {
            total.saturating_add(member.resident.as_u64())
        });
        ByteSize::b(total)
    }

    /// The process with the most resident memory; of equals, the one [`TreeSnapshot::members`] lists first.
    pub(crate) fn largest(&self) -> Member {
        self.members().fold(self.root, |largest, member| {
            if member.resident > largest.resident {
                *member
            } else {
                largest
            }
        })
    }
}

/// One row of the process table, as [`walk`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    /// The process.
    pid: Pid,
    /// Its parent; `None` for a process without one.
    parent: Option<Pid>,
    /// The leader of its session; `None` if it could not be read.
    session: Option<Pid>,
    /// Its resident memory in bytes.
    resident: u64,
}

/// Processes of one tree that [`ProcessTable::freeze`] stopped; they stay stopped until they are killed or thawed.
#[derive(Debug)]
#[must_use = "frozen processes stay stopped until they are killed or thawed"]
pub(crate) struct Frozen {
    /// The tree's root.
    root: Pid,
    /// Every process that was sent `SIGSTOP`, the root among them if it was in the table.
    pids: Vec<Pid>,
}

impl Frozen {
    /// How many frozen processes there are.
    pub(crate) fn len(&self) -> usize {
        self.pids.len()
    }

    /// How many frozen processes are not the root.
    pub(crate) fn others(&self) -> usize {
        self.pids.iter().filter(|pid| **pid != self.root).count()
    }
}

/// The operating system's process table, refreshed on demand.
#[derive(Debug)]
pub(crate) struct ProcessTable {
    /// The process list as of the last read.
    system: System,
}

impl ProcessTable {
    /// Creates a table that has not read any process yet.
    pub(crate) fn new() -> Self {
        Self {
            system: System::new(),
        }
    }

    /// Re-reads the process table and returns the tree of the command `root`, which leads a session of its own.
    pub(crate) fn snapshot(&mut self, root: Pid) -> TreeSnapshot {
        // Threads are left out: on Linux they would otherwise appear as processes and count the same memory twice.
        let refresh = ProcessRefreshKind::nothing().with_memory().without_tasks();
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
        let rows = self
            .system
            .processes()
            .values()
            .filter(|process| process.thread_kind().is_none())
            .map(|process| Row {
                pid: process.pid(),
                parent: process.parent(),
                session: process.session_id(),
                resident: process.memory(),
            });
        walk(root, rows)
    }

    /// The name of `pid` as of the last read; `None` if it was not in the table.
    pub(crate) fn name(&self, pid: Pid) -> Option<OsString> {
        self.system
            .process(pid)
            .map(|process| process.name().to_owned())
    }

    /// Re-reads `pid` and returns when it started, in seconds since the Unix epoch; `None` if it is not running.
    pub(crate) fn start_time(&mut self, pid: Pid) -> Option<u64> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        self.system.process(pid).map(Process::start_time)
    }

    /// Stops every process of the tree of `root` with `SIGSTOP`, reading the table again until a read finds no
    /// process of the tree that was not stopped yet.
    ///
    /// Once that read comes back empty, no process of the tree is left running to fork a child the guard has not
    /// seen, so killing the frozen processes kills the whole tree. A process that cannot be signalled is skipped;
    /// a tree that is still growing after [`FREEZE_READS`] reads is frozen as far as it got.
    pub(crate) fn freeze(&mut self, root: Pid) -> Frozen {
        let mut seen = HashSet::new();
        let mut pids = Vec::new();
        for _ in 0..FREEZE_READS {
            let tree = self.snapshot(root);
            let fresh: Vec<Pid> = tree
                .members()
                .map(|member| member.pid)
                .filter(|pid| seen.insert(*pid))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for pid in &fresh {
                self.signal(*pid, Signal::Stop);
            }
            pids.extend(fresh);
        }
        Frozen { root, pids }
    }

    /// Sends `SIGKILL` to every frozen process.
    pub(crate) fn kill(&self, frozen: Frozen) {
        for pid in &frozen.pids {
            self.signal(*pid, Signal::Kill);
        }
    }

    /// Sends `SIGCONT` to every frozen process.
    pub(crate) fn thaw(&self, frozen: Frozen) {
        for pid in &frozen.pids {
            self.signal(*pid, Signal::Continue);
        }
    }

    /// Sends `signal` to `pid` if it was in the table at the last read.
    ///
    /// Failures are ignored: a process that exited since needs no signal, and the guard has nothing better to do
    /// about one it may not signal.
    fn signal(&self, pid: Pid, signal: Signal) {
        if let Some(process) = self.system.process(pid) {
            let _ = process.kill_with(signal);
        }
    }
}

/// The tree of the command `root` in a process table of `rows`.
///
/// The tree is `root`, every process whose session `root` leads, and every process reachable from any of those
/// through parent links, each exactly once, even if the table has a cycle. A root missing from the table counts as
/// zero bytes but is still the root.
fn walk(root: Pid, rows: impl IntoIterator<Item = Row>) -> TreeSnapshot {
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    let mut resident: HashMap<Pid, u64> = HashMap::new();
    let mut stack = Vec::new();
    for row in rows {
        if let Some(parent) = row.parent {
            children.entry(parent).or_default().push(row.pid);
        }
        if row.session == Some(root) {
            stack.push(row.pid);
        }
        resident.insert(row.pid, row.resident);
    }
    stack.extend(children.get(&root).into_iter().flatten());
    let member = |pid: Pid| Member {
        pid,
        resident: ByteSize::b(resident.get(&pid).copied().unwrap_or(0)),
    };
    let mut seen = HashSet::from([root]);
    let mut others = Vec::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        others.push(member(pid));
        stack.extend(children.get(&pid).into_iter().flatten());
    }
    TreeSnapshot {
        root: member(root),
        others,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    /// A row of a process that is not in the root's session.
    fn row(pid: u32, parent: Option<u32>, resident: u64) -> Row {
        Row {
            pid: Pid::from_u32(pid),
            parent: parent.map(Pid::from_u32),
            session: Some(Pid::from_u32(1)),
            resident,
        }
    }

    /// A grandchild counts; a sibling of the root does not.
    #[test]
    fn walks_descendants_only() {
        let table = [
            row(1, None, 1),
            row(10, Some(1), 100),
            row(11, Some(10), 20),
            row(12, Some(11), 3),
            row(20, Some(1), 5000),
        ];
        let tree = walk(Pid::from_u32(10), table);
        assert_eq!(tree.resident(), ByteSize::b(123));
        assert_eq!(tree.root.pid, Pid::from_u32(10));
        assert_eq!(tree.others.len(), 2);
        assert_eq!(tree.largest().pid, Pid::from_u32(10));
    }

    /// A process of the root's session counts after its parent has gone, and so do its children.
    #[test]
    fn keeps_orphans_of_the_session() {
        let orphan = Row {
            session: Some(Pid::from_u32(10)),
            ..row(30, Some(1), 7000)
        };
        let table = [
            row(1, None, 1),
            row(10, Some(1), 100),
            orphan,
            row(31, Some(30), 5),
        ];
        let tree = walk(Pid::from_u32(10), table);
        assert_eq!(tree.resident(), ByteSize::b(7105));
        assert_eq!(tree.largest().pid, Pid::from_u32(30));
    }

    /// A root that has already gone is still the root and counts as nothing.
    #[test]
    fn missing_root_is_listed() {
        let tree = walk(Pid::from_u32(7), []);
        let root = Member {
            pid: Pid::from_u32(7),
            resident: ByteSize::b(0),
        };
        assert_eq!(
            tree,
            TreeSnapshot {
                root,
                others: vec![]
            }
        );
        assert_eq!(tree.largest(), root);
    }

    /// Of processes with equal memory, the root is reported as the largest.
    #[test]
    fn largest_prefers_the_root_among_equals() {
        let tree = walk(Pid::from_u32(1), [row(1, None, 50), row(2, Some(1), 50)]);
        assert_eq!(tree.largest().pid, Pid::from_u32(1));
    }

    proptest! {
        /// The walk agrees with a fixed-point model: start from the root and its session, then keep adding any
        /// process whose parent is already in the set.
        #[test]
        fn matches_fixed_point_model(
            table in prop::collection::btree_map(
                0_u32..40,
                (prop::option::of(0_u32..40), prop::option::of(0_u32..40), 0_u64..1 << 40),
                0..40,
            ),
            root in 0_u32..40,
        ) {
            let mut model = BTreeSet::from([root]);
            model.extend(table.iter().filter(|(_, (_, session, _))| *session == Some(root)).map(|(pid, _)| *pid));
            loop {
                let before = model.len();
                for (pid, (parent, _, _)) in &table {
                    if parent.is_some_and(|parent| model.contains(&parent)) {
                        model.insert(*pid);
                    }
                }
                if model.len() == before {
                    break;
                }
            }
            let bytes_of = |pid: &u32| table.get(pid).map_or(0, |(_, _, bytes)| *bytes);
            let expected: u64 = model.iter().map(bytes_of).sum();
            let expected_largest = model.iter().map(bytes_of).max().unwrap_or(0);

            let rows = table.iter().map(|(pid, (parent, session, bytes))| Row {
                pid: Pid::from_u32(*pid),
                parent: parent.map(Pid::from_u32),
                session: session.map(Pid::from_u32),
                resident: *bytes,
            });
            let tree = walk(Pid::from_u32(root), rows);
            let walked: BTreeSet<u32> = tree.members().map(|member| member.pid.as_u32()).collect();
            prop_assert_eq!(walked.len(), tree.members().count());
            prop_assert_eq!(tree.root.pid, Pid::from_u32(root));
            prop_assert_eq!(walked, model.clone());
            prop_assert_eq!(tree.resident(), ByteSize::b(expected));
            for member in tree.members() {
                prop_assert_eq!(member.resident, ByteSize::b(bytes_of(&member.pid.as_u32())));
            }
            let largest = tree.largest();
            prop_assert!(model.contains(&largest.pid.as_u32()));
            prop_assert_eq!(largest.resident, ByteSize::b(expected_largest));
        }
    }
}
