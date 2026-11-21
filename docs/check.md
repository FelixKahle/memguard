# `memguard check`

```text
memguard check [--config <path>] [<command>]
```

Says whether a shell command must run through [`memguard run`](run.md), by rules you write in a TOML file. It is
meant for tools that run commands on your behalf, such as coding agents, editors and git hooks: they ask
`memguard check` before running a command, and refuse it if the answer is no. `memguard` itself knows nothing about
any particular tool. The rules are data, and an integration is a few lines of shell ([Integrations](#integrations)).

```sh
memguard check 'cargo test --workspace'
echo 'cargo test --workspace' | memguard check
memguard check --config ./rules.toml 'npm test'
```

The command to judge is the argument, or the whole of stdin, with one trailing newline removed, when the argument is
left out. A command is denied when it matches one of the rules file's `guarded` patterns, none of its `exempt`
patterns, and does not invoke `memguard run`.

| Exit code | Meaning |
|---|---|
| 0 | the command may run as it is; nothing is printed |
| 1 | the command must run through `memguard run`; the rules file's reason is printed on stderr |
| 2 | bad arguments or unusable rules: a `--config` file that is missing, invalid TOML, an invalid regex |

## The rules file

The rules live in `~/.config/memguard/rules.toml`, or in the file named with `--config <path>`. If the default file
is missing, every command is allowed and one line on stderr says so; a missing `--config` file is an error.

```toml
# Commands matched by any of these regexes must run through the guard.
guarded = [
    '(^|[;&|(]\s*)(\S+=\S+\s+)*cargo\s+(\+\S+\s+)?(test|bench|run|nextest)\b',
    'target/(debug|release)/(deps/)?\S+-[0-9a-f]{16}\b',
]

# A command matching any of these is allowed anyway.
exempt = [
    '--no-run',
]

# Printed on stderr for a denied command; {command} is replaced by the command.
reason = "`{command}` runs tests without the memory guard. Run it as: memguard run 10G {command}"
```

| Key | Required | Meaning |
|---|---|---|
| `guarded` | yes | a command matching any of these patterns must run through the guard; empty means nothing is ever denied |
| `exempt` | no | a command matching any of these is allowed anyway |
| `reason` | yes | printed on stderr for a denied command, with every `{command}` replaced by that command |

A command that invokes `memguard run` is always allowed and needs no `exempt` entry, also when `memguard` is called
through a path such as `~/.cargo/bin/memguard run`.

Patterns are [`regex`](https://docs.rs/regex) syntax in multi-line mode, so `^` matches at the start of every line
of a multi-line command. Unknown keys and invalid regexes are errors that name the offending key or pattern.

Patterns should start with `(^|[;&|(]\s*)`, so that a tool name counts only at the start of a command, after a `;`,
`&`, `|` or `(`, and not inside an argument such as `echo pytest`. The `(\S+=\S+\s+)*` that follows lets environment
assignments such as `RUST_LOG=debug cargo test` through to the match.

### Rules for Rust projects

The repository ships rules for Rust projects as [`examples/rules.toml`](../examples/rules.toml): `cargo test`,
`bench`, `run` and `nextest`, and test binaries under `target/`, must run through the guard, while builds that run
nothing (`--no-run`) are allowed. Install them with:

```sh
mkdir -p ~/.config/memguard
cp examples/rules.toml ~/.config/memguard/rules.toml
```

### Rules for other tools

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

## Integrations

`memguard check` needs only the command as text and answers with its exit code, so a few lines of shell connect it
to any tool that runs commands.

### Claude Code

Add a `PreToolUse` hook on the Bash tool to `~/.claude/settings.json`, or to a project's `.claude/settings.json`:

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

Claude Code treats a hook's exit code 2 as "block the tool call and show stderr to the model", so the model sees the
reason from the rules file and can run the command again through `memguard run`. `jq` ships with current macOS; on
Linux, install it from your package manager.

### A shell function

A zsh or bash function that runs a command only if the rules allow it as written:

```sh
checked() { memguard check "$*" && "$@"; }
```

`checked cargo test` then refuses with the reason, and `checked memguard run 10G cargo test` runs.

### Git hooks and CI

Anything that can run a shell command can ask first. For example, a script that runs commands from a list can call
`memguard check "$line" || exit 1` before each of them.
