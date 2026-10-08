# Independent Codex and Dodex TUIs / TUI 双开

macOS and Windows support two independent terminals, Codex TUI and Dodex TUI.
Only the official Codex App is supported on the desktop. A desktop installation
is not needed to install, run, monitor, or update either TUI.

设置 → **TUI 双开**提供安装、修复、打开终端和两个独立更新按钮。
“高级设置”显示账号、数据库、日志、完整程序包及命令路径，并保留监控开关和手动同步。

```text
agent-companion dodex-tui --install
agent-companion dodex-tui --repair
dodex login
dodex resume
dodex fork
dodex exec "Inspect this project"
dodex mcp list
dodex plugin list
dodex agents
dodex queue --thread <UUID> --message "Continue this task"
dodex app-server daemon version
dodex update
```

`dodex app` fails with an explicit explanation. The removed `dodex-app`, desktop
deployment flags, version alignment and paired-update commands are not aliases
for the new installer.

## Storage and native behavior

`tui-instances.json` (schema 2) records TUI identities independently of desktop
installations and monitoring preferences. It lives under:

| Platform | Companion records | Default Dodex home | Public entries |
| --- | --- | --- | --- |
| macOS | `~/Library/Application Support/AgentCompanion` | `…/AgentCompanion/Dodex/codex-home` | `~/.local/bin/codex`, `~/.local/bin/dodex` |
| Windows | `%LOCALAPPDATA%\AgentCompanion` | `%USERPROFILE%\.dodex` | `…\AgentCompanion\bin\codex.exe`, `dodex.exe` |

Existing secondary home, SQLite and log paths are retained during migration,
including `DodexApp/codex-home` and logs in `DodexApp/desktop-data/logs` on macOS.
The directory name does not imply a dependency on a desktop App.

Each TUI owns its account, configuration, sessions, SQLite, logs and background
state. The secondary uses file credentials and fixed database/log settings.
The entry clears inherited account and daemon-routing variables, then sets the
selected `CODEX_HOME`, `CODEX_SQLITE_HOME` and `CODEX_INSTALL_DIR`. Containment,
terminal capabilities, proxies, certificates and locale remain inherited.
Overrides that would redirect Dodex credentials, database or logs are rejected.

The complete secondary package lives at
`$CODEX_HOME/packages/standalone/releases/<version>`; the vendor controls
`current`, update locks and daemon-owned packages. `$CODEX_HOME/native-bin` is
its private installer prefix. Public console entries resolve the selected
package on every launch, so separate versions can update independently.

Native arguments, working directory and terminal streams are forwarded. Unix
exec preserves the process ID and signal exit. Windows uses a console entry
that forwards control events and the child's exit code without a job object.
No `--no-daemon` or daemon-disable environment variable is injected. Explicit
`--no-daemon` remains a native option. Daemon start/stop/restart/update targets
only the selected home. A TUI client exiting does not terminate its daemon.

## Explicit version maintenance

Opening Settings probes local `--version` output; it does not check online
versions. The two update buttons select one instance. `dodex update` runs its
own native updater immediately, without opening Companion.

Codex keeps its original channel. Homebrew is updated with Homebrew; npm uses
npm; an existing standalone installation retains its own package selection.
An unknown native installation is not automatically migrated to standalone.
The primary public entry fixes the primary home even when invoked from a
Dodex terminal. A private primary standalone prefix prevents the native updater
from overwriting the public identity entry.

Companion's per-instance maintenance locks and the vendor's per-home update
locks prevent concurrent publication. Installation journals and ownership
hashes preserve both command generations across interruption. Repeating repair
completes publication without copying or deleting account data. Updates preserve
running terminals; explicit native daemon commands retain their native behavior.

## Migration and recovery

Installation first recognizes the old deployment record and audited terminal
entry, adopts an already managed complete standalone package when available,
and verifies native signatures and manifest/executable versions. App-bundled
CLI paths are never adopted. Otherwise it installs an official complete package.

Original metadata is backed up in `TuiMigration`; the pending publication is
`tui-install-pending.json`. Commands are backed up beside their original entries
as `*.before-tui`. Configuration changes add only missing isolation defaults;
conflicts stop without rewriting the original configuration. The first changed
config is backed up as `config.toml.before-tui`. Monitoring opt-outs survive
repair. Old active desktop records are removed only after the TUI registry has
been committed; their recovery copies remain available.

On Windows, repair also retires a hash-owned old desktop launcher and its
matching Start menu shortcut. Copies and a replayable retirement journal are
kept in `TuiMigration`. An unrelated shortcut is preserved. If Windows still
holds the old launcher open, close that desktop and repeat repair; running
TUI processes are never terminated. Account, database and log directories are
not moved.

On macOS, after verifying the new TUI and original history, run the repository's
`scripts/retire-dodex-app.py` to move the specifically recognized
`~/Applications/Dodex.app` to Trash and deregister its desktop entry. The script
records original and Trash paths and the original Dock configuration. It never
deletes `DodexApp`, account databases, session files or logs, and never terminates
TUI processes. Restore the bundle from the recorded Trash path if necessary.

Windows uses a short default home because native daemon attachment is limited
to a canonical AF_UNIX address of 108 bytes including its terminator. The
vendor requires a non-elevated terminal with detached-process support; a long
legacy home can use the native embedded-server fallback. Migration preserves
that original home and does not inject a different background mode.
[Native daemon contract](https://github.com/openai/codex/blob/rust-v0.161.0/codex-rs/app-server-daemon/README.md)

## Manual configuration and instruction sync

Advanced settings expose explicit Codex → Dodex and Dodex → Codex operations
for `config.toml` and global `AGENTS.md`. The destination is backed up. Auth
files are not copied; destination credential, database and log settings remain
fixed. Installation and repair do not copy settings or instructions. Config
files can contain embedded secrets, which an explicit sync copies.

Account handoff is still provided by `acomp resume`; see [RESUME.md](RESUME.md).
History discovery uses installed TUI records even when monitoring is disabled.
Dodex task navigation selects an existing terminal or resumes its session in
a new terminal, including sessions originally created by the removed desktop.

## Verification

```sh
cargo test -p agent-companion-core --features server,resume --locked
cargo test -p agent-companion --locked
cargo clippy -p agent-companion -p agent-companion-core --all-targets --locked -- -D warnings
python3 scripts/test-tui-instances.py --companion target/debug/acomp
sh scripts/test-macos-ui.sh --menu-bar
```

The real-package acceptance runs on native macOS and Windows CI. It installs
two different releases into temporary homes, compares native command help,
repairs repeatedly, updates only Dodex, signs in with two synthetic loopback
accounts, and checks independent daemons, multiple clients, resume and queue.
No real account is read and no fixture credential is sent to a public model
service. Additional console tests cover environment isolation, native version
selection, stdin/cwd, exit codes and Unix signals / Windows Ctrl+C.
