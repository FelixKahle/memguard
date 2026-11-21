# Using memguard as a library

The `memguard` binary is a thin shell around the `memguard` crate, and everything it does is available to Rust code.
The crate is not published on crates.io; depend on it by path or git:

```toml
[dependencies]
memguard = { git = "https://github.com/FelixKahle/memguard" }
```

Every example on this page is compiled and run as a doctest with `cargo test`, so the examples cannot drift from the
API. The full reference is `cargo doc --open`.

## Running a command under a limit

`guard::run` starts the command in a session of its own and returns how it ended. Whichever way that was, no process
of its tree is left running.

```rust
use std::process::Command;
use std::time::Duration;

use memguard::{ByteSize, MemoryLimit, Outcome, guard};

const LIMIT: MemoryLimit = MemoryLimit::new(ByteSize::gb(1));

fn main() -> Result<(), memguard::Error> {
    let mut command = Command::new("sh");
    command.args(["-c", "exit 3"]);
    match guard::run(&mut command, LIMIT, Duration::from_millis(500))? {
        Outcome::Exited { status, peak, leftovers } => {
            assert_eq!(status.code(), Some(3));
            assert_eq!(leftovers, 0);
            println!("peak {}", peak.display().si());
        }
        other => panic!("the command should have exited on its own: {other:?}"),
    }
    Ok(())
}
```

`Outcome` has three variants:

| Variant | Meaning |
|---|---|
| `Exited { status, peak, leftovers }` | the command exited on its own; `leftovers` processes it left running were killed |
| `Killed { breach, peak }` | the tree passed a limit, named by `breach`, and was killed |
| `Stopped { signal, peak }` | the guard was asked to stop through a `Remote`, and killed the tree |

`peak` is always the largest resident memory of the whole tree seen at any sample.

## Limits

A bare `MemoryLimit` limits the whole tree. `Limits` adds a per-process limit and a time limit:

```rust
use std::process::Command;
use std::time::Duration;

use memguard::{Breach, ByteSize, Limits, MemoryLimit, Outcome, TimeLimit, guard};

const LIMITS: Limits = Limits {
    tree: MemoryLimit::new(ByteSize::gb(6)),
    process: Some(MemoryLimit::new(ByteSize::gb(2))),
    time: Some(TimeLimit::new(Duration::from_millis(200))),
};

fn main() -> Result<(), memguard::Error> {
    let mut command = Command::new("sleep");
    command.arg("5");
    let outcome = guard::run(&mut command, LIMITS, Duration::from_millis(500))?;
    assert!(matches!(outcome, Outcome::Killed { breach: Breach::Time { .. }, .. }));
    Ok(())
}
```

`MemoryLimit::new` and `TimeLimit::new` are `const fn`s that reject zero, so a zero literal fails the build. Limits
read from the outside are parsed, and must name their unit:

```rust
use std::time::Duration;

use memguard::{ByteSize, MemoryLimit, TimeLimit};

fn main() -> Result<(), memguard::Error> {
    let memory: MemoryLimit = "1.5GiB".parse()?;
    let time: TimeLimit = "1h 30m".parse()?;
    assert_eq!(memory.size(), ByteSize::mib(1536));
    assert_eq!(time.duration(), Duration::from_secs(5400));
    assert!("10".parse::<MemoryLimit>().is_err());
    assert!("0s".parse::<TimeLimit>().is_err());
    Ok(())
}
```

## Stopping and suspending from another thread

`guard::control` makes a `Remote` and a `Control`. Pass the `Control` to `guard::run_with`, and hand the `Remote`,
which is cheap to clone, to any thread that needs to stop or suspend the run. The guard acts on a request at once,
not at its next sample.

```rust
use std::process::Command;
use std::thread;
use std::time::Duration;

use memguard::{ByteSize, MemoryLimit, Outcome, guard};

const LIMIT: MemoryLimit = MemoryLimit::new(ByteSize::gb(1));
const SIGTERM: i32 = 15;

fn main() -> Result<(), memguard::Error> {
    let (remote, control) = guard::control(None);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        remote.stop(SIGTERM);
    });
    let mut command = Command::new("sleep");
    command.arg("5");
    let outcome = guard::run_with(&mut command, LIMIT, Duration::from_millis(500), control)?;
    assert!(matches!(outcome, Outcome::Stopped { signal: SIGTERM, .. }));
    Ok(())
}
```

`Remote::stop` takes the number of the signal it stands for, and `Outcome::Stopped` carries it back, so a caller can
exit with 128 + that signal as a shell would. `Remote::suspend` freezes the tree and stops the whole process as
`SIGTSTP` would; once the process is continued, the tree is continued too. The `memguard` binary connects both to
real signals with [`signal-hook`](https://docs.rs/signal-hook).

## The watchdog

A guard killed with `SIGKILL` cannot kill its tree. A `Watchdog` is a second process that does it instead. Start it
before the command and hand it to `guard::control`:

```rust no_run
use std::process::Command;
use std::time::Duration;

use memguard::{ByteSize, MemoryLimit, Watchdog, guard};

const LIMIT: MemoryLimit = MemoryLimit::new(ByteSize::gb(1));

fn main() -> Result<(), memguard::Error> {
    let watchdog = Watchdog::spawn(Command::new("memguard").arg("watchdog"))?;
    let (_remote, control) = guard::control(Some(watchdog));
    let mut command = Command::new("cargo");
    command.arg("test");
    guard::run_with(&mut command, LIMIT, Duration::from_millis(500), control)?;
    Ok(())
}
```

The watchdog command must be a program that passes its stdin to `memguard::watchdog::watch`, as the hidden
`memguard watchdog` subcommand does. A program of your own can be its own watchdog by handling such a subcommand and
starting itself through `std::env::current_exe`. [How memguard works](design.md#the-watchdog) describes the protocol.

## Checking commands against rules

`policy::Policy` is the rules engine of `memguard check`, and `rules_file::load` reads it from a TOML file:

```rust
use memguard::policy::{Policy, Rules};

fn main() -> Result<(), memguard::Error> {
    let policy = Policy::compile(Rules {
        guarded: vec![r"(^|[;&|(]\s*)npm\s+test\b".to_owned()],
        exempt: vec![],
        reason: "Run `{command}` through memguard run.".to_owned(),
    })?;
    let denial = policy.decide("npm test").expect("npm test is guarded");
    assert_eq!(denial.reason, "Run `npm test` through memguard run.");
    assert!(policy.decide("memguard run 4G npm test").is_none());
    assert!(policy.decide("echo npm test").is_none());
    Ok(())
}
```

## Errors

Every fallible function returns `memguard::Error`, one flat enum with a variant per failure and a `Display` that
reads as a sentence. Each function's documentation lists the variants it can return.
