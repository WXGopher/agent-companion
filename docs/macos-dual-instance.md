# Optional macOS Codex instance

Agent Companion starts with only the menu bar visible, the Dock and notch hidden,
and second-instance support disabled. Settings controls each entry independently
and preserves saved choices across upgrades. The menu bar shows weekly quota
readings with Codex above Dodex. A previously available reading stays visible
while idle; `*` and a tooltip identify last-known readings awaiting an update.
The default terminal icon appears before any readings are available.
Before explicit opt-in, opening Settings
does not discover, deploy, adopt, or launch Dodex. Reopening Companion always reaches Settings,
including when every entry point is hidden.

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

While the menu bar is visible, account usage refreshes every five minutes per
instance, including with the popup closed. Background refresh and both Usage
surfaces share requests for the same source. The five-minute refresh interval
does not erase the last-known menu reading: stale values carry `*` until a valid
update arrives, including after failures and quota resets. A valid local reading
can replace a stale account reading. A reset never implies a new 100% allowance.
Hiding the menu bar withdraws background demand without interrupting an open
Usage page. Disabling an instance or replacing its home, database or runtime
clears its cached data.

## Verification

`cargo test -p agent-companion -p agent-companion-core --features agent-companion-core/server`
covers deployment faults, interruptions, concurrent operations, isolated
configuration saves and dashboard routing. Live-data tests remain ignored.
`sh scripts/test-macos-ui.sh` checks the SwiftUI layouts and entry lifecycle;
`--entries` runs just the menu/Dock/notch recovery checks, and `--menu-bar`
checks valid single/dual readings, icon fallback and cached quota updates. The `macos_editor`
integration executable exercises the actual Slint window with synthetic
configurations and deployment states. None of these tests deploy the local
Dodex environment.
