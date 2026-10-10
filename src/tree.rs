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

//! Snapshots of a process tree taken from the operating system's process table.

use std::collections::{HashMap, HashSet};

use bytesize::ByteSize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// One process tree at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeSnapshot {
    /// The resident memory of every process in the tree together.
    pub(crate) resident: ByteSize,
    /// Every process in the tree, the root first; the root is listed even if it was missing from the table.
    pub(crate) pids: Vec<Pid>,
}

/// The operating system's process table, refreshed on demand.
#[derive(Debug)]
pub(crate) struct ProcessTable {
    /// The process list as of the last [`ProcessTable::snapshot`].
    system: System,
}

impl ProcessTable {
    /// Creates a table that has not read any process yet.
    pub(crate) fn new() -> Self {
        Self {
            system: System::new(),
        }
    }

    /// Re-reads the process table and returns the tree rooted at `root`.
    pub(crate) fn snapshot(&mut self, root: Pid) -> TreeSnapshot {
        // Threads are left out: on Linux they would otherwise appear as processes and count the same memory twice.
        let refresh = ProcessRefreshKind::nothing().with_memory().without_tasks();
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
        let entries = self
            .system
            .processes()
            .values()
            .filter(|process| process.thread_kind().is_none())
            .map(|process| (process.pid(), process.parent(), process.memory()));
        walk(root, entries)
    }

    /// Sends `SIGKILL` to every process of `tree` that was in the table when it was last read.
    ///
    /// A process that has exited since is skipped; failures are ignored because the guard has nothing better to do.
    pub(crate) fn kill(&self, tree: &TreeSnapshot) {
        for pid in &tree.pids {
            if let Some(process) = self.system.process(*pid) {
                process.kill();
            }
        }
    }
}

/// The tree rooted at `root` in a table of `(pid, parent, resident bytes)` entries.
///
/// Every process reachable from `root` through parent links is in the tree exactly once, even if the table has a
/// cycle. A root missing from the table counts as zero bytes but is still listed.
fn walk(root: Pid, entries: impl IntoIterator<Item = (Pid, Option<Pid>, u64)>) -> TreeSnapshot {
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    let mut resident: HashMap<Pid, u64> = HashMap::new();
    for (pid, parent, bytes) in entries {
        if let Some(parent) = parent {
            children.entry(parent).or_default().push(pid);
        }
        resident.insert(pid, bytes);
    }
    let mut seen = HashSet::new();
    let mut pids = Vec::new();
    let mut total = 0_u64;
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        total = total.saturating_add(resident.get(&pid).copied().unwrap_or(0));
        pids.push(pid);
        stack.extend(children.get(&pid).into_iter().flatten());
    }
    TreeSnapshot {
        resident: ByteSize::b(total),
        pids,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    /// A grandchild counts; a sibling of the root does not.
    #[test]
    fn walks_descendants_only() {
        let pid = Pid::from;
        let table = [
            (pid(1), None, 1),
            (pid(10), Some(pid(1)), 100),
            (pid(11), Some(pid(10)), 20),
            (pid(12), Some(pid(11)), 3),
            (pid(20), Some(pid(1)), 5000),
        ];
        let tree = walk(pid(10), table);
        assert_eq!(tree.resident, ByteSize::b(123));
        assert_eq!(tree.pids[0], pid(10));
        assert_eq!(tree.pids.len(), 3);
    }

    /// A root that has already gone is still listed and counts as nothing.
    #[test]
    fn missing_root_is_listed() {
        let tree = walk(Pid::from(7), []);
        assert_eq!(
            tree,
            TreeSnapshot {
                resident: ByteSize::b(0),
                pids: vec![Pid::from(7)]
            }
        );
    }

    proptest! {
        /// The walk agrees with a fixed-point model: keep adding any process whose parent is already in the set.
        #[test]
        fn matches_fixed_point_model(
            table in prop::collection::btree_map(0_usize..40, (prop::option::of(0_usize..40), 0_u64..1 << 40), 0..40),
            root in 0_usize..40,
        ) {
            let mut model = BTreeSet::from([root]);
            loop {
                let before = model.len();
                for (pid, (parent, _)) in &table {
                    if parent.is_some_and(|parent| model.contains(&parent)) {
                        model.insert(*pid);
                    }
                }
                if model.len() == before {
                    break;
                }
            }
            let expected: u64 = model.iter().filter_map(|pid| table.get(pid)).map(|(_, bytes)| bytes).sum();

            let entries = table.iter().map(|(pid, (parent, bytes))| (Pid::from(*pid), parent.map(Pid::from), *bytes));
            let tree = walk(Pid::from(root), entries);
            let walked: BTreeSet<usize> = tree.pids.iter().map(|pid| pid.as_u32() as usize).collect();
            prop_assert_eq!(walked.len(), tree.pids.len());
            prop_assert_eq!(walked, model);
            prop_assert_eq!(tree.resident, ByteSize::b(expected));
        }
    }
}
