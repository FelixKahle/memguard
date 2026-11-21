# memguard

`memguard` keeps a runaway command from swapping your machine to death. It does two things:

- `memguard run <limit> <command>...` runs a command and kills its whole process tree (the command, its session and
  every descendant) the moment the tree's combined resident memory passes the limit. Optionally it also kills the
  tree when any single process passes a size of its own, or when the command runs too long. Nothing the command
  starts outlives it: not what it leaves running, not what it orphans, and not even a guard killed with `SIGKILL`.
- `memguard check <command>` says whether a shell command must run through `memguard run`, by rules you write in a
  TOML file (for example: every `cargo test` must). It is meant for tools that run commands on your behalf, such as
  coding agents, editors or git hooks; see [Integrations](#integrations).

macOS and Linux.

## Install

```sh
cargo install --path .
```

This puts the binary at `~/.cargo/bin/memguard`.

## `memguard run`

```sh
memguard run 10G cargo test
memguard run 10G cargo test -- --nocapture
memguard run --interval 2s 1.5GiB npm test
memguard run 512M ./target/release/my-tool --big-input data.bin
memguard run --per-process 2G --timeout 15m 6G cargo test
```

`<limit>` is a byte size with a unit: `10G`, `10GB`, `512M`, `1.5GiB`. `G`, `M` and `K` are decimal (`10G` is
10,000,000,000 bytes); `Gi`, `GiB` and so on are binary. A bare number such as `10` is rejected, so it can never mean
ten bytes by accident. Zero is rejected too.

Every `--interval` (default `500ms`; `humantime` syntax such as `50ms`, `2s`, `1m`) the guard sums the resident
memory of the command's whole tree (see [What the guard contains](#what-the-guard-contains)). When the sum is above the
limit, the tree is killed and the guard prints:

```text
memguard: killed the tree at 12.3 GB (limit 10.0 GB)
```

Two more limits are optional, and either one kills the tree the same way:

- `--per-process <size>` (same syntax as `<limit>`): any single process of the tree holds more than this. Useful
  when one runaway test binary should be stopped long before the whole tree reaches its limit.
- `--timeout <duration>` (`humantime` syntax: `90s`, `15m`, `1h 30m`): the command has run for longer than this,
  measured from when it started, not counting time it spent suspended. The guard wakes up when the timeout runs out,
  not at the next `--interval`.

```text
memguard: killed the tree: process 41027 (rustc) held 2.4 GB (per-process limit 2.0 GB); tree peak 3.1 GB
memguard: killed the tree after 15m 312ms (timeout 15m); peak 1.2 GB
```

If several limits are passed at the same sample, the per-process limit is reported first, then the tree limit, then
the timeout.

Otherwise the guard returns the moment the command exits, kills whatever the command left running, and prints the
largest total it saw:

```text
memguard: peak 1.2 GB
memguard: peak 1.2 GB; killed 2 processes the command left running
```

Everything after `<limit>` is the command, passed through untouched, so options after the command name (including
`--`, `--interval` and `--timeout`) belong to the command, not to `memguard`.

### What the guard contains

The command is started as the leader of a session of its own. Its tree is the command, every process of that
session, and every descendant of any of them. A process whose parent exits stays in its session, so backgrounded and
orphaned processes (`sh -c 'hog &'`) are still counted and still killed. A process leaves the tree only by starting a
session of its own (`setsid`, as daemons do) while none of its ancestors is in the tree any more.

A tree is killed in two steps: every process of it is stopped with `SIGSTOP`, the process table is read again until
no process of the tree is left running, and only then does each get `SIGKILL`. A stopped process cannot fork, so
nothing can fork its way past the kill.

The guard also keeps the tree from outliving it:

| What happens to `memguard` | What happens to the tree |
|---|---|
| `SIGTERM`, `SIGINT`, `SIGHUP` or `SIGQUIT` (`kill`, a process manager, an agent stopping the command, Ctrl-C, `Ctrl-\`, a closed terminal) | killed at once, then `memguard` exits as if it had died of that signal |
| `SIGTSTP` (Ctrl-Z) | stopped together with `memguard`; continued when `memguard` is |
| killed in any other way, `SIGKILL` included | killed by the watchdog |

```text
memguard: received SIGTERM, killed the tree; peak 1.2 GB
memguard: the guard ended without cleaning up; the watchdog killed 3 processes it left
```

The watchdog is a second `memguard` process that the guard starts before the command, in a process group of its own
so that signals sent to the guard's job do not reach it. It holds a pipe whose only writer is the guard. If the pipe
closes before the guard has said it is done, however the guard died, the watchdog kills what is left of the tree.

Running in a session of its own means the command has no controlling terminal. It still reads and writes the
terminal it inherits as stdin, stdout and stderr, and colours still work, but Ctrl-C and Ctrl-Z reach `memguard`
rather than the command: Ctrl-C kills the tree outright, without giving it a chance to clean up. A program that opens
`/dev/tty` itself, such as a password prompt, cannot.

What the guard cannot contain: a process that starts a session of its own after its whole ancestry in the tree has
exited, all between two samples; a process that belongs to another user (for example through `sudo`), which it may
neither stop nor kill; and memory the tree holds through shared memory or the kernel rather than as resident memory
of its processes. Only a kernel facility such as a Linux cgroup closes these, and none exists on macOS.

### Exit codes

| Exit code | Meaning |
|---|---|
| the command's own code | the command exited on its own |
| 128 + N | the command died of signal N (for example 143 for `SIGTERM`), or `memguard` received signal N and killed the tree |
| 137 | the guard killed the tree for passing a limit (tree, per-process or timeout; stderr says which) |
| 1 | `memguard` itself failed, for example the command could not be started |
| 2 | invalid arguments, for example a limit or timeout without a unit |

## `memguard check`

```sh
memguard check 'cargo test --workspace'
echo 'cargo test --workspace' | memguard check
memguard check --config ./rules.toml 'npm test'
```

The command to judge is the argument, or the whole of stdin (one trailing newline removed) when the argument is left
out. A command is denied when it matches one of the rules file's `guarded` patterns, none of its `exempt` patterns,
and does not invoke `memguard run`.

| Exit code | Meaning |
|---|---|
| 0 | the command may run as it is; nothing is printed |
| 1 | the command must run through `memguard run`; the rules file's reason is printed on stderr |
| 2 | bad arguments or unusable rules: a `--config` file that is missing, invalid TOML, an invalid regex |

### Rules

The rules live in `~/.config/memguard/rules.toml`, or in the file named with `--config <path>`. If the default file
is missing every command is allowed, and one line on stderr says so; a missing `--config` file is an error.

The repository ships rules for Rust projects as [`examples/rules.toml`](examples/rules.toml):

```toml
# memguard rules for Rust projects: cargo test, bench, run and nextest, and test binaries under target/, must run
# through `memguard run`. Copy this file to ~/.config/memguard/rules.toml.

# Commands matched by any of these regexes must run through the guard.
guarded = [
    '(^|[;&|(]\s*)(\S+=\S+\s+)*cargo\s+(\+\S+\s+)?(test|bench|run|nextest)\b',
    'target/(debug|release)/(deps/)?\S+-[0-9a-f]{16}\b',
]

# A command matching any of these is already guarded (or exempt) and is allowed.
# A command that invokes `memguard run` is always allowed and needs no entry here.
exempt = [
    '--no-run',
]

# Shown when a command is denied. The literal token {command} is replaced by the offending command.
reason = """`{command}` runs tests or a binary without the memory guard. The machine swaps and that damages the \
hardware. Wrap it: memguard run 10G {command} (the guard kills the process tree at the limit and prints the peak; \
exit 137 means killed). Builds without running are allowed (--no-run)."""
```

Install it with:

```sh
mkdir -p ~/.config/memguard
cp examples/rules.toml ~/.config/memguard/rules.toml
```

- `guarded`: a command matching any of these patterns must run through the guard. Empty means nothing is denied.
- `exempt`: a command matching any of these is allowed anyway. Optional. A command that invokes `memguard run`
  (also through a path such as `~/.cargo/bin/memguard run`) is always allowed and needs no entry.
- `reason`: printed on stderr for a denied command, with every `{command}` replaced by that command.

Patterns are [`regex`](https://docs.rs/regex) syntax in multi-line mode, so `^` matches at the start of every line
of a multi-line command. Unknown keys and invalid regexes are errors that name the offending key or pattern.

Rules for other tools follow the same shape. For example, to guard Python and JavaScript test runs:

```toml
guarded = [
    '(^|[;&|(]\s*)(\S+=\S+\s+)*(python3?\s+-m\s+)?pytest\b',
    '(^|[;&|(]\s*)npm\s+(run\s+)?test\b',
]
exempt = [
    '--collect-only',
]
reason = "`{command}` runs tests without the memory guard. Run it as: memguard run 8G {command}"
```

Patterns start with `(^|[;&|(]\s*)` so that a tool name only counts at the start of a command, not inside an argument
such as `echo pytest`.

## Integrations

`memguard check` only needs the command as text and reports through its exit code, so a few lines of shell connect it
to any tool that runs commands.

### Claude Code

Add a `PreToolUse` hook on the Bash tool to `~/.claude/settings.json` (or a project's `.claude/settings.json`):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "jq -r '.tool_input.command // empty' | ~/.cargo/bin/memguard check || exit 2",
            "timeout": 10,
            "statusMessage": "Checking the memory guard rules"
          }
        ]
      }
    ]
  }
}
```

Claude Code treats a hook's exit code 2 as "block the tool call and feed stderr back to the model", so Claude sees
the reason from the rules file and can rerun the command through `memguard run`. `jq` ships with current macOS; on
Linux install it from your package manager.

### Anything else

A zsh or bash function that runs a command only if the rules allow it as written:

```sh
checked() { memguard check "$*" && "$@"; }
```

`checked cargo test` then refuses with the reason, and `checked memguard run 10G cargo test` runs.

## License

MIT, see [LICENSE](LICENSE).
