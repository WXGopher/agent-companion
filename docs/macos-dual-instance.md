# Optional macOS Codex instance

Agent Companion uses the menu bar as its only macOS entry. The notch and
separate Dock entry have been removed; legacy visibility preferences cannot
hide the menu bar. Click the readout for Tasks / Usage and open Settings from
the popup footer. Reopening Companion reveals the popup.

Codex appears above Dodex, with each row showing its own task-status dot and
weekly quota. A previous reading stays visible while idle; `*` and a tooltip
identify last-known readings awaiting an update. Missing readings show `—`.
Second-instance support remains disabled until explicit opt-in. Opening
Settings does not discover, deploy, adopt, or launch Dodex before that opt-in.

## Deployment and compatibility

The **Codex 双开** section runs deployment on a worker and reports progress.
Fresh deployment requires a valid OpenAI-signed Codex.app in `/Applications`
or `~/Applications`. Its bundle is copied without changing its signature.
Companion creates `~/Applications/Dodex.app` and stores its runtime, Codex
home, SQLite directory, desktop data, cache and logs under
`~/Library/Application Support/AgentCompanion/Dodex`.

The launcher binds each directory explicitly, starts with a clean environment,
and never inherits authentication or instance overrides. A fresh profile sets
`cli_auth_credentials_store = "file"`; no credentials, history or personal
settings are copied. The user opens Dodex and signs in separately after
deployment. See [the official authentication documentation](https://developers.openai.com/zh-Hans/docs/auth).

Existing Companion deployments are checked against their manifest, completion
marker, launcher, configuration, runtime signature and isolation paths.
The original Codex-B layout can also be adopted when its launcher, plist and
manager match the audited code fingerprints in `macos_deployment.rs`.
Its `Dodex.app` entry may be a symbolic link to `Codex B.app`; the real bundle
is verified before adoption. This exception applies only to the launcher alias,
not to profile, credential, database or runtime paths. Signed runtime archives
are checked in fixed-size chunks so larger official releases remain supported.
Unknown legacy versions, ambiguous environments, redirected/shared files and
overlapping primary/secondary data directories fail without being overwritten.
Companion does not execute a legacy manager while validating it.

Temporary directories and exclusive renames prevent replacement of existing
environments. The completion marker is published atomically after validation.
Retry can finish an interrupted publication only when the exact expected
Companion manifest and remaining files validate. An OS file lock serializes
deployment and preference writes across settings processes.

Disabling support stops Companion's secondary monitoring and discards its
caches. It does not quit Dodex or delete files. Re-enabling validates the saved
deployment before monitoring resumes. Runtime updates and uninstall are not
provided.

## Repairing the original desktop launcher

Version `0.3.19` adds `agent-companion dodex-app` (read-only check)
and `agent-companion dodex-app --repair` (explicit desktop repair). Exit the
Dodex desktop first; running TUI sessions can remain open. The repair validates
the known original installation, copies its signed runtime unchanged into
`/Applications/.Dodex/Dodex.app`, and replaces the original desktop launcher
with an exact, isolated launcher. `/Applications/Dodex.app` continues to be the
public entry. Its existing alias to `Codex B.app` is supported.

A saved environment originally deployed by Companion also supports the check;
`--repair` validates it and reports that no legacy repair is needed. Legacy
repair accepts only the audited original code fingerprints. Unknown variants
are preserved and rejected. After the publication journal exists, retry can
finish or clean up the recognized interrupted publication. A forced exit before
that journal is written leaves the incomplete directory for manual inspection.

The new launcher always supplies the second profile and desktop data directory.
It does not ask the old manager whether a CLI process is running. Electron's
instance handling receives repeated opens with the same explicit desktop data
path. No login, credentials, history or profile configuration is migrated.
The old runtime, manager and shell commands remain in place for TUI use.
The original launcher is backed up as
`/Applications/.Dodex/Original Launcher.app`; Companion's existing enabled or
disabled preference is retained while its runtime reference is updated.

After repair, run `python3 scripts/repair-dodex-dock.py --check` to inspect
existing Dock entries, then `--apply` to remove known Dodex launcher/runtime pins.
The launcher and signed runtime have different bundle identities: pinning the
launcher creates an extra icon while the desktop runs. The running Dodex app
supplies its own Dock icon, alongside the primary Codex app. After quitting Dodex,
start it from Applications/Dodex.app or `dodex app`. Do not pin or directly open
the hidden runtime: it would bypass the launcher's independent account environment.
Applying saves a private Dock preference backup, updates only matching application
tiles and restarts Dock. Other entries and their order remain unchanged. It neither
quits an app nor changes the global recent-apps setting.

Before consolidation, the desktop copy and retained TUI runtime are separate
snapshots. The old manager's maintenance commands do not discover the new
desktop path: **exit Dodex desktop before using those legacy commands**.
The optional `scripts/prepare-dodex-manager.py` prepares the narrowly audited
old manager launch-detection fix into an explicitly supplied output file; it
never installs it. Companion accepts both audited manager fingerprints, as well
as its own exact repaired desktop launcher and manifest. Unknown variants still
fail validation.

## Consolidating the repaired App and TUI entries

The following maintenance scripts run from a **source checkout** and are not
installed with the packaged App. They support only the audited original legacy
installation; they are not a general installer for arbitrary copies of Codex.
They never run automatically when upgrading or opening Companion.

For the audited original installation after desktop repair,
`python3 scripts/consolidate-dodex.py` checks the plan without installing it.
`--apply` publishes the unified layout. Quit Dodex desktop first; the existing
Dodex CLI can remain running. The installer checks pending maintenance
transactions, preserves original launch entries and manager code in
`/Applications/.Dodex/Consolidation Backup`, and records an interruption journal.
Rerunning the same installer completes a recognized interrupted publication.
It rejects unknown or concurrently changed files instead of replacing them.

The resulting layout is:

| Entry | Purpose |
| --- | --- |
| Original official Codex App | Primary desktop, unchanged |
| `/Applications/Dodex.app` | Real isolated launcher bundle, replacing the alias and `Codex B.app` |
| `/Applications/.Dodex/Dodex.app` | One unchanged signed runtime used by Dodex App and TUI |
| `codex` | Primary CLI entry, explicitly bound to `~/.codex` |
| `dodex` | Secondary CLI entry, explicitly bound to `~/.codex-second` |

The original account, session and desktop-data directories stay in place.
Both CLI entries preserve working directory, arguments, PATH, proxy settings
and inherited containment metadata, while removing inherited account/session
and runtime overrides. Primary credential-store and SQLite settings still come
from its own configuration. The primary wrapper execs the vendor-managed
standalone `current/bin/codex`; no vendor package files are changed. A future
standalone installer may replace this entry with a symlink, in which case the
same-target wrapper can be restored by rerunning the consolidation installer.
Unknown targets remain untouched.

All Dodex desktop forms, including `dodex app PATH`, use the same runtime.
Desktop launch detection ignores TUI processes. Update, checkpoint and rollback
require all Dodex App, TUI and helper processes to exit, including old runtime
processes. The existing `codex-b-*` aliases remain compatible. New snapshots use
the canonical schema; old snapshots remain intact but cannot be restored over
the new layout, because doing so would resurrect the obsolete manager and paths.
Runtime replacement may require reapplying the custom icon.

While the current CLI still uses `/Applications/Codex B Runtime.app`, the
installer only adds the directory's BSD hidden flag. It never kills or moves a
running runtime. After its last process exits, the next Dodex manager operation
checks the deferred retirement record and signature, then moves the old runtime
exclusively to `/Applications/.Dodex/Retired Runtime.app`. Conflicts and failed
checks preserve the original. This is a backup, not another launch entry.

After consolidation, `python3 scripts/repair-dodex-dock.py --apply` removes known
Dodex launcher/runtime entries from both pinned and recent Dock items, preserving
other applications and global preferences. macOS may add recent items again
later; the helper does not disable recent applications globally.

## Custom Dodex icon

From a source checkout, `scripts/set-macos-app-icon.swift` applies an image you
supply as a Finder custom icon. Apply the same image to the repaired desktop
runtime and public launcher; do not replace signed `Contents/Resources` files.
Personal artwork and image-generation metadata are not distributed.

```sh
swift scripts/set-macos-app-icon.swift /path/to/icon.icns /Applications/.Dodex/Dodex.app
swift scripts/set-macos-app-icon.swift /path/to/icon.icns /Applications/Dodex.app
```

macOS custom icons add an `Icon\r` resource fork and root FinderInfo, which
strict codesign verification rejects as metadata. Companion first attempts
normal strict verification. For this specific metadata shape only, it verifies
the installed app's official identity and sealed resources, makes a private
copy-on-write clone, removes only the clone's root icon file and FinderInfo,
and requires strict verification of that clone. Other modified resources or
nested metadata still fail. The installed bundle is never changed by checking.
This fallback requires a filesystem that supports macOS copy-on-write clones.

## Manual configuration and instruction sync

The **Codex 双开** settings tab provides separate **Codex → Dodex** and
**Dodex → Codex** overwrite actions for `config.toml` and the personal
`AGENTS.md`. Both instances' full file paths are shown for manual editing.
Sync is explicit: editing a file does not automatically change the other copy.
A validated deployed Dodex profile can be synchronized while monitoring is
disabled, without enabling it or launching either client.

Configuration sync copies the source document, including any manually embedded
provider tokens, MCP environment variables or HTTP headers. It preserves the
destination's `cli_auth_credentials_store`, `sqlite_home` and `log_dir` settings
so that account storage, databases and logs remain separate. It never copies
`auth.json`, keychain entries, session history or an entire profile directory.
Login credentials normally live outside `config.toml`, but the file is **not
guaranteed to be secret-free**; see the official [authentication documentation](https://learn.chatgpt.com/docs/auth)
and [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference).

Instruction sync copies only `AGENTS.md` inside each instance's Codex home,
not repository instructions. The page identifies an `AGENTS.override.md` when
present because it can take precedence; that file is not overwritten by the
`AGENTS.md` action. See [global instruction discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md).

Each overwrite backs up an existing destination before replacing it atomically.
An absent source does not clear the destination; a missing destination can be
created. Invalid configuration and concurrent changes fail without overwriting
the target. Configuration sync is blocked while a status-bar draft has unsaved
changes. The result reports whether anything changed and where a backup was
saved. Restart the corresponding client to ensure it loads the saved files.

## Instance routing

The primary instance uses `~/.codex`, independently of the environment of the
process that launched Companion. Its configured `sqlite_home` is respected.
Each instance has its own task monitor, database root, quota/error state and
status-bar draft. Task keys combine the instance ID and conversation ID.

Tasks shows a separate weekly-quota card for every enabled instance, including
Dodex when it has no quota reading yet. Each card opens that instance's Usage
page. Cards use their own local snapshot or recent account cache; displaying
Tasks does not add account requests to the menu bar's background refresh. The menu panel scrolls overflowing page
content while keeping Settings and the other footer actions visible.

Desktop navigation sends the conversation event to the matching running app
process. It fails if that target is absent or ambiguous. CLI recovery commands
bind the selected executable, home and database and clear inherited overrides.
Usage goes through the official CLI's account methods; Companion never opens
credential files or logs raw account responses.
Completed Usage results are cached in memory per instance for five minutes
from completion. Reopening Usage or switching instances restores a fresh cache
immediately; a missing or expired result is read again. The refresh button
bypasses the cache, while repeated clicks during a read share that request.
Closing the panel preserves completed results; cancelled reads create no cache.

While Companion runs, account usage refreshes every five minutes per
instance, including with the popup closed. Background refresh and the Usage
page share requests for the same source. The five-minute refresh interval
does not erase the last-known menu reading: stale values carry `*` until a valid
update arrives, including after failures and quota resets. A valid local reading
can replace a stale account reading. A reset never implies a new 100% allowance.
The menu-bar entry stays enabled; quitting Companion stops background reads.
Disabling an instance or replacing its home, database or runtime clears its
cached data.

## Verification

`cargo test -p agent-companion -p agent-companion-core --features agent-companion-core/server`
covers deployment faults, interruptions, concurrent operations, isolated
configuration saves and dashboard routing. Live-data tests remain ignored.
`sh scripts/test-macos-ui.sh` checks the SwiftUI popup and menu-bar lifecycle;
`--menu-bar` checks single/dual readings, task-state colors, missing readings,
animation and cached quota updates. The `macos_editor`
integration executable exercises the actual Slint window with synthetic
configurations and deployment states. None of these tests deploy the local
Dodex environment.
