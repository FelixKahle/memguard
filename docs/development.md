# Development

## Repository layout

| Path | Contents |
|---|---|
| [`src/main.rs`](../src/main.rs) | the `memguard` binary: arguments, signal handling, watchdog spawn, messages, exit codes |
| [`src/lib.rs`](../src/lib.rs) | the library root and its re-exports |
| [`src/guard.rs`](../src/guard.rs), [`tree.rs`](../src/tree.rs), [`watchdog.rs`](../src/watchdog.rs), [`limit.rs`](../src/limit.rs) | `memguard run`; see [How memguard works](design.md#code-map) |
| [`src/policy.rs`](../src/policy.rs), [`rules_file.rs`](../src/rules_file.rs) | `memguard check` |
| [`src/error.rs`](../src/error.rs) | the crate's one error enum |
| [`tests/run.rs`](../tests/run.rs) | `memguard run` against the built binary, with real processes |
| [`tests/check.rs`](../tests/check.rs) | `memguard check` against the built binary |
| [`examples/rules.toml`](../examples/rules.toml) | the rules for Rust projects |
| [`docs/`](.) | this documentation; [`docs/img/`](img) holds the README banner |

## Building

```sh
cargo build
cargo install --locked --path .
```

The crate needs the Rust version pinned as `rust-version` in [`Cargo.toml`](../Cargo.toml), on macOS or Linux. Dev
builds optimise dependencies but not the crate itself, so it stays debuggable; release builds use fat LTO and keep
line tables for backtraces.

## Testing

```sh
memguard run 6G cargo test --locked
```

Run the test suite under a guard. Use an installed `memguard` that you know works, never the build under test: a
bug in the code being tested must not also be a bug in the guard around it.

The suite has four parts:

- **Unit tests** next to the code: limit parsing, the error messages, the watchdog's line protocol with its JSON round
  trip, and a property test (`proptest`) of the tree walk against a deliberately naive fixed-point model.
- **[`tests/run.rs`](../tests/run.rs)** starts the real binary on real processes. It checks exit codes, each limit,
  an orphaned memory hog, leftovers, a fork loop that must leave no survivor, prompt handling of the exit and of
  `SIGTERM` with a 10 s interval, `SIGINT`, `SIGTSTP` and `SIGCONT`, and a `SIGKILL` of the guard that the watchdog
  must clean up after. Survivors are found by session id.
- **[`tests/check.rs`](../tests/check.rs)** runs `memguard check` on the example rules.
- **Doctests**, including every example of [the library guide](library.md), which `src/lib.rs` includes as a doctest.

The memory hogs in the tests hold about 300 MB each, and every test bounds its own runtime.

## Conventions

- Errors are one flat enum with a hand-written `Display`; no `anyhow` or `thiserror`.
- Every item has a doc comment, private items included. A function that can panic documents it under `# Panics`,
  and one that returns `Result` documents its errors under `# Errors`. The lints in `Cargo.toml` enforce this.
- A value with an invariant, such as a non-zero limit, has a `const fn` constructor that panics in a `const` context,
  so literals are checked at compile time. Values read from the outside are parsed instead.
- Data that crosses a process boundary derives `serde`: the rules file and the watchdog's messages.
- Dependencies point down between modules; see the [code map](design.md#code-map).
- Process tables come from `sysinfo`; nothing shells out to `ps`.
- The one `unsafe` block, the `pre_exec` that calls `setsid` in `src/guard.rs`, carries a `// SAFETY:` comment.
  std's own `setsid` is not stable yet.

Check a change with:

```sh
cargo fmt --check
cargo clippy --locked --all-targets
cargo doc --locked --no-deps
memguard run 6G cargo test --locked
```

## The banner

[`docs/img/banner.py`](img/banner.py) writes `banner-light.svg` and `banner-dark.svg` from one template, so the two
cannot drift apart. Edit the script, then run:

```sh
python3 docs/img/banner.py
```

The README picks the variant through a `<picture>` element with a `prefers-color-scheme` source.
