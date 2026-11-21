# memguard documentation

| Page | What it covers |
|---|---|
| [`memguard run`](run.md) | running a command under memory and time limits: options, units, messages, signals, exit codes, and what cannot be contained |
| [`memguard check`](check.md) | deciding which commands must run through the guard: the rules file, its patterns, and integrations such as a Claude Code hook |
| [How memguard works](design.md) | the session that contains the tree, sampling, freeze-then-kill, noticing the exit, signals, the watchdog, and a map of the code |
| [Using memguard as a library](library.md) | the Rust API: `guard::run`, limits, `Remote` and `Control`, the watchdog, and the rules engine |
| [Development](development.md) | the repository layout, building, testing under a guard, conventions, and the banner |

Start with the [project README](../README.md) for installation and a quick start.
