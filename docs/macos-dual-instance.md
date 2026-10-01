# Optional macOS Codex instance

Agent Companion uses the menu bar as its only macOS entry. The notch and
separate Dock entry have been removed; legacy visibility preferences cannot
hide the menu bar. Click the readout for Tasks / Usage and open Settings from
the popup footer. Reopening Companion reveals the popup.

Codex appears above Dodex, with each row showing its own task-status dot and
weekly quota. A previous reading stays visible while idle; `*` and a tooltip
identify last-known readings awaiting an update. Missing readings show `—`.
Second-instance support remains disabled until explicit opt-in. Settings reads
installed App metadata and saved directory paths for display; opening the page
does not verify or synchronize the App, deploy, adopt, or launch Dodex, or read
account credentials. App checks and synchronization on this page start only
when **安装 Dodex App** or **检查并同步** is clicked. Enabling monitoring and
explicit configuration writes retain their own validation.

## Create and update Dodex App

The settings page separates **Dodex App**, **Companion 接入**, and the secondary
account's **副账号与 TUI**. App installation is reported independently
of Companion monitoring: an existing TUI profile does not mean the new App is
installed, and disabling monitoring does not uninstall the App. The account
configuration actions below these sections copy files between Codex and Dodex;
they do not update either application's program files.

The **Codex 双开** page creates Dodex App from the official Codex App already
installed on this Mac. Thereafter, opening Dodex from Finder or Dock checks the
official App's version and build. Matching versions launch immediately without
copying or signing. A different version is copied, verified and published before
the Dodex window opens. Updating the official App while Dodex is running takes
effect after Dodex is quit and opened again.

This startup check uses the installed Companion executable; its menu bar app
does not need to be running. It does not download an installer or run a background
update daemon. If the source or updater is unavailable, or synchronization fails,
the existing Dodex App still opens and a later startup can retry. Concurrent
startup and manual synchronization share a deployment lock; an already running
Dodex process is never replaced.

To install, click **安装 Dodex App**. For a manual update, quit Dodex and click
**检查并同步**, or run:

```sh
agent-companion dodex-app --sync
```

`agent-companion dodex-app` only checks the current App and reports whether its
source matches the locally installed official version. Neither public command
launches an App. Existing mirrors need one manual sync after installing this
Companion update to receive the startup check.

The startup check leaves signatures untouched when versions match. An actual
upgrade changes the local signature, so macOS may ask again for Keychain access;
silent synchronization does not bypass system authorization prompts.

The source must have a valid OpenAI signature and the Codex identity. Supported
source names are `Codex.app` and `ChatGPT.app` in `/Applications` or
`~/Applications`; unrelated ChatGPT applications and the hidden TUI runtime are
not sources. Companion copies the official App, sets the Dodex bundle identity,
name, icon and explicit instance environment, then signs the local copy. The
vendor executable's code and `app.asar` are not patched. A small native entry
inside the same bundle uses `exec` to start the original executable with the
selected desktop-data directory. Only a validated `--open-project` directory
is forwarded; launch arguments cannot override the profile. Companion task
navigation uses Apple events.
The native executable and outer bundle receive local ad-hoc signatures. The
modified bundle does not retain the source App's original signature.

Dodex is one public App bundle, normally `/Applications/Dodex.app`. On a new
installation without a system-wide Dodex entry, it uses
`~/Applications/Dodex.app`. Finder, Dock and Companion desktop navigation all
open that public App. There is no separate shell launcher starting another
hidden desktop App. The entry and original process use the same PID and
public bundle, keeping the running App attached to its Dodex Dock entry.

## Accounts, TUI and monitoring

Existing environments keep their Codex home, desktop data and SQLite directory.
For the original installation these are `~/.codex-second`,
`~/Library/Application Support/Codex-B` and `~/.codex-second/sqlite`. Dodex App
and the existing TUI continue to share that profile, including its configuration,
login and session history. App synchronization does not rewrite its files.

App synchronization alone leaves the TUI command, manager and hidden runtime in
place. Explicit TUI maintenance below can replace the compatible terminal entry
after backing it up, while retaining its account and session directories. The
saved legacy monitoring record remains intact; in-memory runtime descriptors,
quota queries and copied resume commands then use the validated managed CLI.
The `acomp resume` console entry uses the same validated managed runtime for
Dodex while retaining the saved history and SQLite paths, even with monitoring
disabled. The original deployment record is preserved.
`dodex app [PATH]` opens the current public App with the original adapter's
workspace argument rules, including when the App is already running.
The App mirror has its own record inside
`Contents/Resources/agent-companion-mirror.json`.

A fresh App uses separate directories under
`~/Library/Application Support/AgentCompanion/DodexApp`; it does not copy the
primary instance's credentials, history or personal configuration and requires
its own login. If that official release does not include a compatible CLI,
the App can still be synchronized, but Companion cannot enable its task/usage
monitoring until a compatible CLI is available.

The support switch controls Companion monitoring. Disabling it does not quit
Dodex or remove files. Updating an existing App preserves this preference.

## Explicit version maintenance

Right-click the menu-bar readout, or use **版本管理** in Settings → **Codex 双开**:

- **对齐 Codex/Dodex 版本** compares installed versions without contacting a server. Codex is the reference for its TUI and App separately. A newer Dodex is retained with a suggestion to check official updates; alignment never downgrades it.
- **全部更新到最新** fetches official stable CLI release metadata and the official App feed. It updates the primary standalone CLI with its own updater, verifies the downloaded App's OpenAI signature and exact version/build, and then aligns Dodex. App and CLI version numbers remain independent.

Opening Settings reads local version metadata only. Four rows show current and
target versions; failures and partial updates remain visible and never imply
that both instances are current. Existing Apps and TUI processes must be closed
by the user before replacement. Maintenance does not quit or restart them.
Concurrent settings windows and App synchronization share filesystem locks.

TUI alignment currently supports the complete official standalone package with
`codex-package.json`, plus the known existing Dodex terminal adapter. Other
package managers and unknown adapters are rejected before updates; they are not
silently converted. The entire versioned package is copied, including
`codex-code-mode-host`, `codex-path` and `codex-resources`. The original adapter is
retained under `~/Library/Application Support/AgentCompanion/Tui/original-dodex`;
complete packages stay under `Tui/packages`. Native terminal arguments, exit
status, PID and existing profile, session, SQLite and log paths are preserved.
The adopted `dodex update` command opens Companion's explicit update operation.
No credentials, personal configuration or session files are copied or rewritten.
The legacy hidden runtime and deployment metadata remain for existing validation
and rollback; removing them manually is not part of this migration.

The primary App retains a sibling backup before replacement, and Dodex uses its
existing mirror backup mechanism. An interrupted or failed step reports its
actual installed versions; completed updates remain installed and can be retried.
The existing startup and manual App synchronization also preserve a newer Dodex
instead of copying an older primary App over it.
The isolated tests cover paths, arguments, account separation and native launcher
forwarding; they do not claim a real-account interactive conversation was sent
or resumed during automated verification.

## Publication, backup and Dock

Synchronization checks the current App, builds and verifies a staged copy, then
publishes it at the public path. It refuses to replace an App while its desktop
process is running and never quits it automatically. The first replaced public
bundle is retained as `.Dodex-mirror-backup.app` beside the public entry. Unknown
or changed installation contents fail validation instead of being overwritten.
The original official App and the hidden TUI runtime are not modified.

The public bundle now provides both the saved Dock entry and the running App
identity. Old shell-launcher or hidden-runtime Dock items may still need cleanup:

```sh
python3 scripts/repair-dodex-dock.py --check
python3 scripts/repair-dodex-dock.py --apply
```

The helper normalizes existing known Dodex pins to one public entry, preserves
its position and GUID, removes duplicates and matching recent shortcuts, and
keeps unrelated entries unchanged. A correct public pin retains its macOS
bookmarks and dates. It does not add a pin when none exists or change the global
recent-apps setting. Applying saves a private preference backup, verifies the
written arrays and restarts Dock.

The earlier `dodex-app --repair` and source-checkout consolidation tools are
legacy maintenance paths. They are not the App update mechanism. Existing
schema-1 and schema-2 shell launchers remain recognized as migration inputs;
new App deployment uses the public mirror. Do not rerun an old consolidation
journal over the mirrored public App, because its stored launcher fingerprint
belongs to the previous layout.

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

Primary application opening, desktop navigation and bundled CLI discovery use
the same fixed application candidates as fresh deployment. `Codex.app` takes
priority over a Codex-identified `ChatGPT.app`; an unrelated ChatGPT app is
never treated as Codex. Desktop navigation still matches the selected path.

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

For an installed standalone runtime, the opt-in command
`python3 -B scripts/test-managed-dodex-session.py --codex /path/to/bin/codex`
runs the generated Dodex wrapper with a synthetic older session. It verifies
native app-server list, read and resume against the original rollout and shared
secondary paths. The fixture denies network access and real account directories;
it neither starts a model turn nor modifies the installed runtime.
