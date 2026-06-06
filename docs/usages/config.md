# Configuration

`smol-wf` reserves platform-standard application directories for durable state and future global configuration.

## Application directories

### Linux and other XDG environments

#### State directory

Durable workflow state belongs under the XDG state base directory:

```txt
$XDG_STATE_HOME/smol-workflows
```

When `XDG_STATE_HOME` is unset or empty, the fallback is:

```txt
~/.local/state/smol-workflows
```

The default durable SQLite database should live at:

```txt
$XDG_STATE_HOME/smol-workflows/workflows.db
```

or, with the fallback:

```txt
~/.local/state/smol-workflows/workflows.db
```

Use `--db <path>` on commands such as `run` and `history` to override the database path.

#### Config directory

Global user configuration belongs under the XDG config base directory:

```txt
$XDG_CONFIG_HOME/smol-workflows
```

When `XDG_CONFIG_HOME` is unset or empty, the fallback is:

```txt
~/.config/smol-workflows
```

The reserved global config file path is:

```txt
$XDG_CONFIG_HOME/smol-workflows/config.toml
```

or, with the fallback:

```txt
~/.config/smol-workflows/config.toml
```

### macOS

Use the standard per-user Library locations:

```txt
~/Library/Application Support/smol-workflows/workflows.db
~/Library/Application Support/smol-workflows/config.toml
```

macOS does not have a separate XDG-style state directory by default, so durable state and user config both use the application support directory unless an explicit `--db <path>` is provided.

### Windows

Use the standard per-user application data location. With the usual `%APPDATA%` value, paths are:

```txt
%APPDATA%\smol-workflows\workflows.db
%APPDATA%\smol-workflows\config.toml
```

Typically this expands to:

```txt
C:\Users\<user>\AppData\Roaming\smol-workflows\workflows.db
C:\Users\<user>\AppData\Roaming\smol-workflows\config.toml
```

`--db <path>` should continue to accept normal Windows paths, for example:

```powershell
smol-wf run .\workflow.mjs --db C:\workflows\workflows.db
```

## Database path resolution

The durable database path should be resolved in this order:

1. `--db <path>` passed to the current command.
2. The platform default database path.

The platform default database path is:

| Platform | Default database path |
| --- | --- |
| Linux / XDG | `$XDG_STATE_HOME/smol-workflows/workflows.db` |
| Linux / XDG fallback | `~/.local/state/smol-workflows/workflows.db` |
| macOS | `~/Library/Application Support/smol-workflows/workflows.db` |
| Windows | `%APPDATA%\smol-workflows\workflows.db` |

Notes:

- `--db <path>` is intentionally separate from a future `--config <path>`. `--config` would choose which config file to load; `--db` chooses the durable database for this command.
- `run` may create the default database and its parent directory when needed.
- `history` should not create a missing database. If the resolved database does not exist, `history` should report the resolved path and ask the user to pass `--db` or run a workflow first.
- Relative `--db` paths are resolved relative to the current working directory, matching normal CLI path behavior.

## Planned global config

Global config is reserved for future defaults such as provider selection, logging, budgets, and concurrency. Until global config is implemented, command-line flags remain the source of configuration.

Planned precedence for configuration values in general:

```txt
CLI flags > global config > built-in defaults
```

Database path configuration is intentionally not specified here yet. For now, the database path is selected only by `--db <path>` or by the platform default database path.
