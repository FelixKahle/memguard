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

//! Running a command under memory and time limits, contained so that none of its processes outlives the guard's
//! verdict.

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use bytesize::ByteSize;
use rustix::io::Errno;
use rustix::process::{WaitId, WaitIdOptions, WaitIdStatus, waitid};
use signal_hook::consts::signal::SIGSTOP;
use sysinfo::Pid;

use crate::tree::{ProcessTable, TreeSnapshot};
use crate::{Error, MemoryLimit, TimeLimit, Watchdog};

/// What a guarded command must stay within.
///
/// A bare [`MemoryLimit`] converts into limits for the whole tree alone. A per-process limit at or above the tree
/// limit never fires before the tree limit does, but is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The resident memory of the whole tree together.
    pub tree: MemoryLimit,
    /// The resident memory of any one process in the tree; `None` means no per-process limit.
    pub process: Option<MemoryLimit>,
    /// The wall-clock time since the command was started, time spent suspended not counted; `None` means no time
    /// limit.
    pub time: Option<TimeLimit>,
}

impl From<MemoryLimit> for Limits {
    fn from(tree: MemoryLimit) -> Self {
        Self {
            tree,
            process: None,
            time: None,
        }
    }
}

/// How a guarded command ended. Whichever way it ended, no process of its tree is left running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The command exited on its own (or was killed by someone other than the guard).
    Exited {
        /// The command's own exit status.
        status: ExitStatus,
        /// The largest resident memory of the whole tree seen at any sample; zero if it exited before the first.
        peak: ByteSize,
        /// How many processes of the tree were still running when the command exited, and were killed.
        leftovers: usize,
    },
    /// The command passed one of its [`Limits`] and the guard killed every process in its tree.
    Killed {
        /// Which limit was passed.
        breach: Breach,
        /// The largest resident memory of the whole tree seen at any sample, the killing one included.
        peak: ByteSize,
    },
    /// [`Remote::stop`] asked the guard to stop, and it killed every process in the tree.
    Stopped {
        /// The signal number the stop was asked with.
        signal: i32,
        /// The largest resident memory of the whole tree seen at any sample.
        peak: ByteSize,
    },
}

/// The limit a killed command passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Breach {
    /// The whole tree held more than [`Limits::tree`]; the size it held is the [`Outcome::Killed`] peak, since every
    /// earlier sample was within the limit.
    Tree,
    /// One process held more than [`Limits::process`].
    Process {
        /// The process's id.
        pid: u32,
        /// The process's name as the operating system reports it; empty if it could not be read.
        name: OsString,
        /// Its resident memory at the sample that crossed the limit; always above the limit.
        used: ByteSize,
    },
    /// The command ran for longer than [`Limits::time`].
    Time {
        /// How long it had run when the guard noticed, time spent suspended not counted; always above the limit.
        elapsed: Duration,
    },
}

/// What a running guard reacts to between two samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// The command has exited; it is not reaped yet.
    Exited,
    /// [`Remote::stop`] was called with this signal number.
    Stop(i32),
    /// [`Remote::suspend`] was called.
    Suspend,
}

/// Asks a running guard to stop or to suspend; cheap to clone and to hand to another thread, such as one that
/// receives signals.
///
/// The guard acts on a request at once, not at its next sample. Requests made after it has returned are ignored.
#[derive(Debug, Clone)]
pub struct Remote(Sender<Event>);

impl Remote {
    /// Asks the guard to kill every process of the tree and return [`Outcome::Stopped`] carrying `signal`.
    pub fn stop(&self, signal: i32) {
        let _ = self.0.send(Event::Stop(signal));
    }

    /// Asks the guard to stop every process of the tree, then to stop this whole process as `SIGTSTP` would.
    ///
    /// Once this process is continued, the guard continues the tree; time spent suspended does not count towards
    /// [`Limits::time`].
    pub fn suspend(&self) {
        let _ = self.0.send(Event::Suspend);
    }
}

/// The guard's end of a [`Remote`], to pass to [`run_with`]; made by [`control`].
#[derive(Debug)]
pub struct Control {
    /// A sender kept by the guard, so that its inbox never disconnects while it runs.
    events: Sender<Event>,
    /// Where the guard receives requests and the command's exit.
    inbox: Receiver<Event>,
    /// The watchdog to announce the command to, if any.
    watchdog: Option<Watchdog>,
}

/// Makes the controls of one guarded run: the [`Remote`] that drives it and the [`Control`] to pass to [`run_with`].
///
/// With a `watchdog`, the command's tree is also killed if this process dies before the guard has killed it, even
/// of `SIGKILL`.
#[must_use]
pub fn control(watchdog: Option<Watchdog>) -> (Remote, Control) {
    let (events, inbox) = mpsc::channel();
    let remote = Remote(events.clone());
    (
        remote,
        Control {
            events,
            inbox,
            watchdog,
        },
    )
}

/// Runs `command` and kills its whole tree once it passes any of `limits`; when the command exits on its own, what it
/// left running is killed too.
///
/// The command leads a session of its own, so its tree is the command, every process of that session and every
/// descendant of any of them: a process stays in the tree when its parent exits, and leaves it only by starting a
/// session of its own while it has no parent in the tree. The command has no controlling terminal; it can still
/// read and write a terminal it inherits as stdin, stdout or stderr. A `command` that is set to join a process
/// group cannot also lead a session, and fails to start.
///
/// Every `interval`, and when a time limit runs out, the guard reads the resident memory of each process of the tree.
/// When one process is above the per-process limit, the tree together is above the tree limit, or the command has
/// run longer than the time limit, the tree is killed; when several are passed at the same sample, the breach
/// reported is the first of that list. A tree is killed by stopping each of its processes with `SIGSTOP`, reading the
/// table again until no process of the tree is left running, and then sending `SIGKILL` to all of them, so no
/// process can fork its way past the kill. The command is reaped only after its tree is gone, so that its process
/// id, and with it its session, cannot pass to another process while the guard still kills by them.
///
/// # Errors
///
/// Returns [`Error::Spawn`] if the command cannot be started, [`Error::ThreadSpawn`] if the guard cannot start the
/// thread that waits for it, and [`Error::Wait`] if its status cannot be collected.
pub fn run(
    command: &mut Command,
    limits: impl Into<Limits>,
    interval: Duration,
) -> Result<Outcome, Error> {
    run_with(command, limits, interval, control(None).1)
}

/// [`run`], driven by the [`Remote`] that belongs to `control` and announced to its watchdog, if it has one.
///
/// # Errors
///
/// As [`run`], and [`Error::WatchdogLost`] if the watchdog cannot be told about the command (which is then killed),
/// or [`Error::Suspend`] if the guard cannot stop itself for a suspension.
pub fn run_with(
    command: &mut Command,
    limits: impl Into<Limits>,
    interval: Duration,
    control: Control,
) -> Result<Outcome, Error> {
    Guard::start(command, limits.into(), interval, control)?.watch()
}

/// A command being guarded.
#[derive(Debug)]
struct Guard {
    /// The command.
    child: Child,
    /// The command's process id, which is also the id of its session.
    root: Pid,
    /// The process table the tree is read from.
    table: ProcessTable,
    /// What the tree must stay within.
    limits: Limits,
    /// The longest wait between two samples.
    interval: Duration,
    /// When the command was started.
    started: Instant,
    /// How long the guard has been suspended in all.
    paused: Duration,
    /// The largest resident memory of the whole tree seen so far.
    peak: ByteSize,
    /// Where requests and the command's exit arrive.
    inbox: Receiver<Event>,
    /// Kept so that the inbox cannot disconnect while the guard runs.
    _events: Sender<Event>,
    /// The watchdog, released once the guard has killed the tree itself.
    watchdog: Option<Watchdog>,
}

/// What is left once a guard has killed its tree and reaped its command.
#[derive(Debug)]
struct Finished {
    /// The command's exit status.
    status: ExitStatus,
    /// How many processes of the tree other than the command were killed.
    leftovers: usize,
    /// The largest resident memory of the whole tree seen at any sample.
    peak: ByteSize,
}

impl Guard {
    /// Starts `command` in a session of its own, announces it to the watchdog and starts waiting for its exit.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Spawn`] if the command cannot be started. If the watchdog cannot be told or the exit cannot
    /// be waited for, the command's tree is killed and [`Error::WatchdogLost`] or [`Error::ThreadSpawn`] returned.
    fn start(
        command: &mut Command,
        limits: Limits,
        interval: Duration,
        control: Control,
    ) -> Result<Self, Error> {
        let Control {
            events,
            inbox,
            watchdog,
        } = control;
        let child = spawn_in_session(command)?;
        let mut guard = Self {
            root: Pid::from_u32(child.id()),
            child,
            table: ProcessTable::new(),
            limits,
            interval,
            started: Instant::now(),
            paused: Duration::ZERO,
            peak: ByteSize::b(0),
            inbox,
            _events: events.clone(),
            watchdog,
        };
        let ready = guard
            .announce()
            .and_then(|()| notify_exit(&guard.child, events));
        if let Err(error) = ready {
            guard.finish()?;
            return Err(error);
        }
        Ok(guard)
    }

    /// Tells the watchdog, if there is one, which command it guards.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WatchdogLost`] if the watchdog cannot be reached.
    fn announce(&mut self) -> Result<(), Error> {
        let Some(watchdog) = &mut self.watchdog else {
            return Ok(());
        };
        watchdog.announce(self.root.as_u32(), self.table.start_time(self.root))
    }

    /// Samples the tree until it passes a limit, the command exits or a request ends the run.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Wait`] if the command's status cannot be collected and [`Error::Suspend`] if a suspension
    /// fails.
    fn watch(mut self) -> Result<Outcome, Error> {
        loop {
            let tree = self.table.snapshot(self.root);
            self.peak = self.peak.max(tree.resident());
            if let Some(breach) = find_breach(&self.table, &tree, self.limits, self.elapsed()) {
                let peak = self.finish()?.peak;
                return Ok(Outcome::Killed { breach, peak });
            }
            // A timeout is the next sample; the guard's own sender keeps the inbox from disconnecting.
            match self.inbox.recv_timeout(self.next_wait()) {
                Ok(Event::Exited) => {
                    let Finished {
                        status,
                        leftovers,
                        peak,
                    } = self.finish()?;
                    return Ok(Outcome::Exited {
                        status,
                        peak,
                        leftovers,
                    });
                }
                Ok(Event::Stop(signal)) => {
                    let peak = self.finish()?.peak;
                    return Ok(Outcome::Stopped { signal, peak });
                }
                Ok(Event::Suspend) => self.suspend()?,
                Err(_) => {}
            }
        }
    }

    /// How long the command has run, time spent suspended not counted.
    fn elapsed(&self) -> Duration {
        self.started.elapsed().saturating_sub(self.paused)
    }

    /// How long to wait for an event before the next sample: the interval, or less if the time limit runs out sooner.
    fn next_wait(&self) -> Duration {
        let left = self
            .limits
            .time
            .map(|limit| limit.duration().saturating_sub(self.elapsed()));
        left.map_or(self.interval, |left| left.min(self.interval))
    }

    /// Stops the tree, then this whole process; once this process is continued, continues the tree.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Suspend`] if this process cannot stop itself; the tree is continued first.
    fn suspend(&mut self) -> Result<(), Error> {
        let frozen = self.table.freeze(self.root);
        let since = Instant::now();
        // Returns once this process has been stopped and continued.
        let stopped = signal_hook::low_level::raise(SIGSTOP);
        self.paused += since.elapsed();
        self.table.thaw(frozen);
        stopped.map_err(Error::Suspend)
    }

    /// Kills every process of the tree, reaps the command and releases the watchdog.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Wait`] if the command's status cannot be collected; the watchdog is then not released, so it
    /// sweeps the tree again.
    fn finish(mut self) -> Result<Finished, Error> {
        let frozen = self.table.freeze(self.root);
        let leftovers = frozen.others();
        self.table.kill(frozen);
        // The command is also killed through its handle in case the table missed it; an error only means it has
        // already exited.
        let _ = self.child.kill();
        let status = self.child.wait().map_err(Error::Wait)?;
        if let Some(watchdog) = self.watchdog {
            watchdog.release();
        }
        Ok(Finished {
            status,
            leftovers,
            peak: self.peak,
        })
    }
}

/// Starts `command` as the leader of a new session.
///
/// # Errors
///
/// Returns [`Error::Spawn`] if the command cannot be started or cannot start a session.
fn spawn_in_session(command: &mut Command) -> Result<Child, Error> {
    // SAFETY: `pre_exec` runs the closure in the forked child before `exec`, where only async-signal-safe calls are
    // allowed. `setsid` is one, and turning its error code into an `io::Error` does not allocate.
    unsafe {
        command.pre_exec(|| rustix::process::setsid().map(drop).map_err(io::Error::from));
    }
    command.spawn().map_err(|source| Error::Spawn {
        program: command.get_program().to_owned(),
        source,
    })
}

/// Sends [`Event::Exited`] to `events` once `child` has exited, without reaping it.
///
/// # Errors
///
/// Returns [`Error::ThreadSpawn`] if the waiting thread cannot be started.
fn notify_exit(child: &Child, events: Sender<Event>) -> Result<(), Error> {
    let pid = rustix::process::Pid::from_child(child);
    let wait = move || {
        while !has_exited(waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
        )) {}
        let _ = events.send(Event::Exited);
    };
    thread::Builder::new()
        .name("memguard-exit".to_owned())
        .spawn(wait)
        .map(drop)
        .map_err(Error::ThreadSpawn)
}

/// Whether a `waitid` for a child's exit says that the child has exited; any error but an interruption counts, since
/// waiting again would fail the same way.
///
/// macOS also reports a child that was stopped or continued, although only exits were asked for, so those reports
/// do not count.
fn has_exited(waited: rustix::io::Result<Option<WaitIdStatus>>) -> bool {
    match waited {
        Ok(Some(status)) => status.exited() || status.killed() || status.dumped(),
        Ok(None) | Err(Errno::INTR) => false,
        Err(_) => true,
    }
}

/// The first limit of `limits` that `tree`, sampled from `table` after `elapsed`, passes: per process, then the tree,
/// then time.
fn find_breach(
    table: &ProcessTable,
    tree: &TreeSnapshot,
    limits: Limits,
    elapsed: Duration,
) -> Option<Breach> {
    let largest = tree.largest();
    if limits
        .process
        .is_some_and(|limit| limit.is_exceeded_by(largest.resident))
    {
        return Some(Breach::Process {
            pid: largest.pid.as_u32(),
            name: table.name(largest.pid).unwrap_or_default(),
            used: largest.resident,
        });
    }
    if limits.tree.is_exceeded_by(tree.resident()) {
        return Some(Breach::Tree);
    }
    limits
        .time
        .is_some_and(|limit| limit.is_exceeded_by(elapsed))
        .then_some(Breach::Time { elapsed })
}
