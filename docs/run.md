# `memguard run`

```text
memguard run [--interval <duration>] [--per-process <size>] [--timeout <duration>] <limit> <command>...
```

Runs `<command>` and kills its whole process tree the moment the tree passes a limit. When the command exits on its
own, whatever it left running is killed too, so nothing the command started outlives it.

```sh
memguard run 10G cargo test
memguard run 10G cargo test -- --nocapture
memguard run --interval 2s 1.5GiB npm test
memguard run 512M ./target/release/my-tool --big-input data.bin
memguard run --per-process 2G --timeout 15m 6G cargo test
```

Everything after `<limit>` is the command, passed through untouched. Options after the command name, `--`,
`--interval` and `--timeout` included, belong to the command, not to `memguard`. Options for `memguard` go before
`<limit>`.

## Limits

| Option | Kills the tree when | Syntax |
|---|---|---|
| `<limit>` (required) | the resident memory of the whole tree together passes it | byte size with a unit |
| `--per-process <size>` | any single process of the tree passes it | byte size with a unit |
| `--timeout <duration>` | the command has run longer than this, time spent suspended not counted | duration with a unit |

**Byte sizes** need a unit: `10G`, `10GB`, `512M`, `1.5GiB`. `G`, `M` and `K` are decimal (`10G` is 10,000,000,000
bytes); `Gi`, `GiB`, `Mi`, `MiB` and so on are binary. A bare number such as `10` is rejected, so it can never mean
ten bytes by accident, and so is zero.

**Durations** use [`humantime`](https://docs.rs/humantime) syntax: `500ms`, `90s`, `15m`, `1h 30m`. A bare number and
zero are rejected.

A per-process limit at or above the tree limit is allowed, but can never fire first. It is useful below the tree
limit, when one runaway test binary should be stopped long before the whole tree gets close to its limit.

## Sampling

Every `--interval` (default `500ms`) the guard reads the process table and sums the resident memory of every process
of the tree. It also wakes when a timeout runs out, when the command exits and when a signal arrives, so none of
those wait for the next sample.

A sampling guard always overshoots by whatever the tree can allocate within one interval. A shorter interval
narrows the overshoot at the cost of reading the process table more often; `100ms` to `200ms` is cheap on a desktop
machine. Keep the limit well below the machine's physical memory either way. See [Resident memory](design.md#resident-memory)
for why that matters.

## What it prints

Everything `memguard` says goes to stderr, prefixed with `memguard:`, and stdout is left entirely to the command.
Every run that `memguard` sees to its end finishes with one of these lines:

```text
memguard: peak 1.2 GB
memguard: peak 1.2 GB; killed 2 processes the command left running
memguard: killed the tree at 12.3 GB (limit 10.0 GB)
memguard: killed the tree: process 41027 (rustc) held 2.4 GB (per-process limit 2.0 GB); tree peak 3.1 GB
memguard: killed the tree after 15m 312ms (timeout 15m); peak 1.2 GB
memguard: received SIGTERM, killed the tree; peak 1.2 GB
```

The peak is the largest total the guard saw at any sample. If several limits are passed at the same sample, the
per-process limit is reported first, then the tree limit, then the timeout.

## What the tree is

The command is started as the leader of a session of its own. Its tree is the command, every process of that
session, and every descendant of any of them.

A process whose parent exits is handed to a new parent, so a walk down parent links loses it, but it stays in its
session. Backgrounded and orphaned processes (`sh -c 'hog &'`, `(hog &)`) are therefore still counted and still
killed. A process leaves the tree only by starting a session of its own (`setsid`, as daemons do) after every
ancestor it had in the tree has exited.

Killing a tree stops every process of it with `SIGSTOP`, reads the process table again until no process of the tree
is left running, and only then sends `SIGKILL` to each. A stopped process cannot fork, so nothing forks its way past
the kill. [How memguard works](design.md) has the details.

## Signals and the watchdog

`memguard` keeps the tree from outliving it, however it ends:

| What happens to `memguard` | What happens to the tree |
|---|---|
| `SIGTERM`, `SIGINT`, `SIGHUP` or `SIGQUIT`: `kill`, a process manager, an agent stopping the command, Ctrl-C, `Ctrl-\`, a closed terminal | killed at once; `memguard` then exits as if it had died of that signal |
| `SIGTSTP`: Ctrl-Z | stopped together with `memguard`, and continued when `memguard` is |
| killed in any other way, `SIGKILL` included | killed by the watchdog |

The watchdog is a second `memguard` process that the guard starts before the command, in a process group of its own
so that signals sent to the guard's job do not reach it. If the guard dies without saying it is done, the watchdog
kills what is left of the tree:

```text
memguard: the guard ended without cleaning up; the watchdog killed 3 processes it left
```

## The terminal

Because the command leads a session of its own, it has no controlling terminal:

- It still reads and writes the terminal it inherits as stdin, stdout and stderr, and colours still work.
- Ctrl-C and Ctrl-Z reach `memguard` rather than the command. Ctrl-C kills the tree outright, with no chance to clean
  up; a command that must handle `SIGINT` itself should be stopped some other way.
- A program that opens `/dev/tty` itself, such as a password prompt, cannot.

## Exit codes

| Exit code | Meaning |
|---|---|
| the command's own code | the command exited on its own |
| 128 + N | the command died of signal N (for example 143 for `SIGTERM`), or `memguard` received signal N and killed the tree |
| 137 | the guard killed the tree for passing a limit; stderr says which |
| 1 | `memguard` itself failed, for example the command or the watchdog could not be started |
| 2 | invalid arguments, for example a limit or timeout without a unit |

A command that is itself killed with `SIGKILL` also exits 137. The stderr line tells the two apart.

## What it cannot contain

- A process that starts a session of its own after every ancestor it had in the tree has exited, all between two
  samples.
- A process that belongs to another user, for example one started through `sudo`, which the guard may neither stop
  nor kill.
- Memory that is not resident memory of the tree's processes: shared memory, memory held by the kernel on the
  tree's behalf, and pages that were compressed or swapped out.

Only a kernel facility, such as a Linux cgroup, closes these, and macOS has none that an unprivileged user can use.
