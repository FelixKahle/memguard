# How memguard works

`memguard run` is a supervisor in userspace. It cannot ask the kernel to cap a process tree (macOS has no facility
for that which an unprivileged user can use), so it starts the command in a way that keeps every descendant
findable, samples the tree's memory, and kills the whole tree when a limit is passed. This page describes each part
and why it is built the way it is.

```text
             memguard (guard)                                    memguard watchdog
  ┌──────────────────────────────────────┐   lifeline pipe   ┌───────────────────────┐
  │ main thread: sample, decide, kill    │ ────────────────▶ │ waits for "Released"  │
  │ exit thread: waitid(WEXITED|NOWAIT)  │   JSON lines      │ or EOF; on EOF kills  │
  │ signal thread: SIGTERM/INT/HUP/      │                   │ what is left          │
  │                QUIT/TSTP → Remote    │                   └───────────────────────┘
  └──────────────────┬───────────────────┘                    own process group
                     │ spawns with setsid
                     ▼
        ┌──── session led by the command ────┐
        │ command ─┬─ child ── grandchild    │
        │          └─ child                  │
        │ orphan (parent gone, still here)   │
        └────────────────────────────────────┘
```

## The tree

The command is spawned with `setsid` in the forked child, before `exec`, so it leads a new session and a new process
group. The tree is then:

1. the command;
2. every process whose session id is the command's process id;
3. every descendant, through parent links, of any process in 1 or 2.

Parent links alone are not enough. When a process exits, its children are handed to another parent (`launchd`,
`init` or a subreaper), and a walk down from the command no longer reaches them. That is exactly what
`sh -c 'hog &'` and `(hog &)` do. The session survives the hand-over: a process stays in its session unless it calls
`setsid` itself. Rule 3 still matters for a descendant that started a session of its own while its parent is in the
tree.

The process table comes from [`sysinfo`](https://docs.rs/sysinfo). Threads are left out, since on Linux they would
otherwise appear as processes and count the same memory twice. The walk is a pure function of the table rows, and a
property test checks it against a deliberately naive fixed-point model.

## Sampling

The main thread loops:

1. read the process table and compute the tree;
2. record the peak, and check the limits in a fixed order: per process, then the whole tree, then time;
3. wait for an event, for at most `--interval`, or less when the time limit runs out sooner.

The events are the command's exit, a stop request and a suspend request. They arrive on one channel, so the guard
acts on each the moment it happens, not at the next sample. Time spent suspended is subtracted before checking the
time limit.

### Resident memory

The guard measures resident memory: the pages each process has in physical memory. That is what grows when a test
runs away, and it is what `ps` and `top` show as RSS. It is not everything a process costs. On macOS, pages that the
system compressed or swapped out are not resident, so under memory pressure a runaway's resident size can look
smaller than it really is. With a limit well below the machine's physical memory, the guard fires long before that
pressure sets in, which is why the limit should always be chosen that way.

## Noticing the exit

A dedicated thread calls `waitid(P_PID, command, WEXITED | WNOWAIT)`. `WNOWAIT` reports the exit without reaping the
command. The command stays a zombie until the guard has killed the rest of the tree, and only then is it reaped.
While it is a zombie, its process id cannot be given to another process, and with it the session id the guard kills
by. Without this, a fast-cycling machine could hand the id to an unrelated process between the exit and the sweep.

macOS's `waitid` also reports a child that was stopped or continued, although only exits were asked for. Those
reports are ignored; the thread waits again until the status is an exit, a kill or a core dump.

## Killing the tree

A tree that is killed in one pass can fork past the kill: between reading the table and sending the signals, a
process can start a child that was in no snapshot. So the guard freezes first:

1. read the table and send `SIGSTOP` to every process of the tree not stopped yet;
2. repeat until a read finds no new process (at most 64 reads);
3. send `SIGKILL` to every stopped process, and to the command through its handle.

A stopped process cannot fork, so every child that exists by the last read was already in the tree, and every one of
them is stopped. The loop ends after a few reads. Only a process the guard may not signal, such as one owned by
another user, could keep adding children. The bound of 64 reads keeps that case from looping forever.

The same freeze-then-kill runs whichever way the run ends: a limit was passed, the command exited and left processes
running, or the guard was told to stop.

## Signals

Signal handlers are installed before anything is spawned, through
[`signal-hook`](https://docs.rs/signal-hook)'s iterator. A dedicated thread turns each signal into a request on the
guard's channel:

| Signal | Request | Effect |
|---|---|---|
| `SIGTERM`, `SIGINT`, `SIGHUP`, `SIGQUIT` | stop | the tree is killed and `memguard` exits 128 + the signal |
| `SIGTSTP` | suspend | the tree is frozen, `memguard` stops itself with `SIGSTOP`; once continued, it continues the tree |

Because the command has a session of its own, signals the terminal sends to the foreground job (Ctrl-C, Ctrl-Z,
`Ctrl-\`, a hangup) reach `memguard` and not the tree, and `memguard` passes them on in the form above. Without the
suspend handling, Ctrl-Z would stop only the guard and leave the tree running unwatched.

## The watchdog

Nothing a process does can survive its own `SIGKILL`, so the guard has a second process. Before it starts the
command, `memguard run` starts `memguard watchdog`, a hidden subcommand:

- in a process group of its own, so that signals sent to the guard's job, a `kill -9 -<job>` included, do not reach
  it;
- with a pipe on its stdin whose only writer is the guard. The write end is close-on-exec, so the command never
  inherits it and cannot keep the pipe open after the guard is gone.

The guard sends one JSON line once the command runs and one when it is done:

```json
{"Started":{"session":48121,"started":1763731200}}
"Released"
```

`started` is the command's start time in seconds since the Unix epoch (`null` if the command was already gone when
the guard looked). A `Released` line ends the watchdog without killing anything. If the pipe closes without one,
because the guard was killed, crashed or returned an error, the watchdog sweeps the announced tree with the same
freeze-then-kill. Before sweeping, it checks that a process now running under the announced id, if any, started when
the command did. If it did not, the id belongs to another process, so the session can no longer be told apart and the
watchdog kills nothing.

If the watchdog cannot be started, `memguard run` refuses to run the command. If it cannot be told about the command,
the command is killed and the run fails. In both cases the guard fails closed.

## Code map

| Module | Responsibility | Depends on |
|---|---|---|
| [`limit`](../src/limit.rs) | `MemoryLimit` and `TimeLimit`: never-zero limits, parsed with a unit | — |
| [`tree`](../src/tree.rs) | the process table, the tree walk, freeze, kill, thaw | — |
| [`watchdog`](../src/watchdog.rs) | the watchdog process and its line protocol | `tree` |
| [`guard`](../src/guard.rs) | spawning in a session, sampling, events, `Remote` and `Control` | `tree`, `watchdog`, `limit` |
| [`policy`](../src/policy.rs) | compiled rules and the verdict on a command | — |
| [`rules_file`](../src/rules_file.rs) | finding, reading and compiling the rules file | `policy` |
| [`error`](../src/error.rs) | one flat `Error` enum for the whole crate | — |
| [`main`](../src/main.rs) | the command line: arguments, signal thread, watchdog spawn, messages, exit codes | the library |

Every module also uses `error`. Dependencies point down the table: no module reaches up, and there are no cycles.
