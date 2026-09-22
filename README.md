# portez

Static port numbers, one per project directory and name, backed
by a TOML file. Inspired by [phx-port](https://github.com/chgeuer/phx-port)
and [autoport](https://github.com/chrisgreg/autoport).

```sh
$ cd ~/src/app
$ portez            # first call allocates and remembers
portez: registered main → 4001 for /Users/me/src/app
4001
$ portez            # every later call is instant and identical
4001
$ portez debug      # extra names per directory
4002
```

Stdout carries only the number, so it drops straight into scripts:

```sh
PORT=$(portez) mix phx.server
PORT=$(portez) PORT_DEBUG=$(portez debug) node server.js
```

Or in a `mise.toml`:

```toml
[env]
PORT = "{{ exec(command='portez') }}"
PORT_METRICS = "{{ exec(command='portez metrics') }}"
```

## Commands

| Command | Does |
| --- | --- |
| `portez [NAME]` / `portez get [NAME]` | Print the port for `NAME` (default `main`) in the current directory, registering the lowest free port on first use. |
| `portez get --existing [NAME]` | Same, but exit 1 instead of registering when nothing is assigned yet. |
| `portez list` / `portez ls` | Table of every assignment, sorted by port. `--json` emits an array of `{dir, name, port}`. |
| `portez rm [NAME]` / `portez remove` | Forget one name for the current directory. `--all` forgets every name for it. |
| `portez path` | Print the registry file location. |
| `portez completion <shell>` | Completion script for bash, zsh, fish, elvish, nushell or powershell. |

Global flags: `-C, --dir <DIR>` acts on another directory, `--registry <FILE>`
uses another registry file, `-q, --quiet` silences the first-use notice.

The first registration prints a one-line notice to **stderr**; nothing else
ever goes to stdout except the requested value.

Exit codes: `0` success, `1` application error (unknown name with
`--existing`, unreadable or corrupt registry, bad name), `2` usage error.

## Registry

Location, in order of precedence:

1. `$PORTEZ_CONFIG`
2. `$XDG_CONFIG_HOME/portez/ports.toml`
3. `~/.config/portez/ports.toml`

Format:

```toml
[settings]
start = 4001            # optional; lowest port handed out

[ports."/Users/me/src/app"]
main = 4001
debug = 4002

[ports."/Users/me/src/api"]
main = 4003
```

Directories are stored as canonical absolute paths, so symlinked checkouts of
the same folder share ports. Ports are unique across the whole file. When an
assignment is removed its port is reused by the next new registration.

The file is safe to edit by hand; comments survive rewrites. Writes take an
exclusive lock on `ports.toml.lock` beside the registry and replace the file
atomically, so concurrent first-time registrations never collide.

## Install

```sh
cargo install --path .
```

Or through mise once the repository is pushed:

```toml
[tools]
"cargo:https://github.com/integralthread/portez" = "branch:main"
```

## Development

```sh
mise run check   # cargo check
mise run test    # cargo test
mise run lint    # clippy with warnings denied
mise run fmt     # rustfmt
```

Built with [usage](https://usage.jdx.dev/rust/); `portez __usage_spec__`
emits the CLI spec in KDL.
