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

//! A second process that kills a guarded command's tree when the guard dies without having done so itself.
//!
//! The guard can be killed in ways it cannot catch, `SIGKILL` above all. The watchdog is started before the command,
//! in a process group of its own so that signals sent to the guard's process group do not reach it, and reads a pipe
//! whose only writer is the guard. The guard announces the command on that pipe once it has started it, and releases
//! the watchdog once it has killed the command's tree itself. If the pipe closes without a release, the guard is
//! gone, and the watchdog kills what is left of the tree.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};

use serde::{Deserialize, Serialize};
use sysinfo::Pid;

use crate::Error;
use crate::tree::ProcessTable;

/// One line from a guard to its watchdog, as JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Message {
    /// The guard has started its command.
    Started {
        /// The command's process id, which is also the id of the session it leads.
        session: u32,
        /// When the command started, in seconds since the Unix epoch; `None` if it had already exited when the guard
        /// looked. It tells the command apart from a later process that is given the same id.
        started: Option<u64>,
    },
    /// The guard has killed the command's tree itself, so the watchdog has nothing left to do.
    Released,
}

/// A watchdog process, started before the command it will guard; see the [module docs](self).
#[derive(Debug)]
pub struct Watchdog {
    /// The watchdog process.
    child: Child,
    /// The write end of the watchdog's stdin; closing it without a release makes the watchdog kill the tree.
    lifeline: ChildStdin,
}

impl Watchdog {
    /// Starts `command` as a watchdog, in a process group of its own, with a pipe from this process as its stdin and
    /// its stdout discarded.
    ///
    /// The command must pass its stdin to [`watch`], as `memguard watchdog` does.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WatchdogSpawn`] if the command cannot be started.
    pub fn spawn(command: &mut Command) -> Result<Self, Error> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(Error::WatchdogSpawn)?;
        match child.stdin.take() {
            Some(lifeline) => Ok(Self { child, lifeline }),
            None => Err(Error::WatchdogSpawn(io::Error::other(
                "the watchdog has no stdin pipe",
            ))),
        }
    }

    /// Tells the watchdog which command to guard: the one whose process id is `session`, started at `started`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WatchdogLost`] if the watchdog cannot be reached.
    pub(crate) fn announce(&mut self, session: u32, started: Option<u64>) -> Result<(), Error> {
        self.send(Message::Started { session, started })
    }

    /// Tells the watchdog that the guard has killed the tree itself, and waits for the watchdog to exit.
    ///
    /// Failures are ignored: a watchdog that cannot be told only sweeps a tree that is already gone.
    pub(crate) fn release(mut self) {
        let _ = self.send(Message::Released);
        drop(self.lifeline);
        let _ = self.child.wait();
    }

    /// Writes `message` to the watchdog as one line of JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WatchdogLost`] if the watchdog cannot be reached.
    fn send(&mut self, message: Message) -> Result<(), Error> {
        let mut line =
            serde_json::to_vec(&message).map_err(|error| Error::WatchdogLost(error.into()))?;
        line.push(b'\n');
        self.lifeline.write_all(&line).map_err(Error::WatchdogLost)
    }
}

/// Watches the guard at the other end of `lifeline` and kills what is left of its command's tree if the guard goes
/// away without a release; returns how many processes it killed.
///
/// Returns once the guard releases the watchdog or the lifeline closes. Nothing is killed after a release, before
/// any command was announced, or when the command's process id has been given to another process since.
///
/// # Errors
///
/// Returns [`Error::LifelineUnreadable`] if the lifeline cannot be read and [`Error::MessageInvalid`] if a line on
/// it is not a message from a guard; the announced tree is killed first either way.
pub fn watch(lifeline: impl Read) -> Result<usize, Error> {
    let mut command = None;
    let mut lines = BufReader::new(lifeline).lines();
    let ended = loop {
        let Some(line) = lines.next() else {
            break Ok(());
        };
        let message = line
            .map_err(Error::LifelineUnreadable)
            .and_then(|line| serde_json::from_str(&line).map_err(Error::MessageInvalid));
        match message {
            Ok(Message::Started { session, started }) => command = Some((session, started)),
            Ok(Message::Released) => return Ok(0),
            Err(error) => break Err(error),
        }
    };
    let killed = command.map_or(0, |(session, started)| sweep(session, started));
    ended.map(|()| killed)
}

/// Kills what is left of the tree of the command whose process id is `session`, started at `started`; returns how
/// many processes were killed.
fn sweep(session: u32, started: Option<u64>) -> usize {
    let root = Pid::from_u32(session);
    let mut table = ProcessTable::new();
    if table
        .start_time(root)
        .is_some_and(|running| Some(running) != started)
    {
        // The command is gone and its id belongs to another process, so its session can no longer be told apart.
        return 0;
    }
    let frozen = table.freeze(root);
    let killed = frozen.len();
    table.kill(frozen);
    killed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message survives a JSON round trip.
    #[test]
    fn messages_round_trip() {
        for message in [
            Message::Started {
                session: 42,
                started: Some(1_700_000_000),
            },
            Message::Started {
                session: 7,
                started: None,
            },
            Message::Released,
        ] {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), message);
        }
    }

    /// A release ends the watch without killing anything, even with an announced command.
    #[test]
    fn a_release_kills_nothing() {
        let lifeline = b"{\"Started\":{\"session\":1,\"started\":null}}\n\"Released\"\n";
        assert_eq!(watch(&lifeline[..]).unwrap(), 0);
    }

    /// A lifeline that closes before any announcement leaves nothing to kill.
    #[test]
    fn nothing_announced_kills_nothing() {
        assert_eq!(watch(&b""[..]).unwrap(), 0);
    }

    /// A line that is not a message is an error.
    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            watch(&b"hello\n"[..]),
            Err(Error::MessageInvalid(_))
        ));
    }
}
