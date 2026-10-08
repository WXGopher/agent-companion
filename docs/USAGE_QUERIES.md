# GUI account usage queries

macOS and Windows use `agent-companion-core::usage_service::Scheduler` for scheduling and in-memory state, and the application's `usage_service::UsageService` for background processes. Swift accesses that service through `agent_companion_usage_event` and `agent_companion_usage_snapshot_json`; Windows calls it directly. Snapshot reads and rendering do not schedule requests.

| Event | Quota | Historical tokens |
| --- | --- | --- |
| Startup or newly enabled/source-changed instance | Query each new instance | No query |
| Timer due | Query each due instance | No query |
| User opens a closed panel, including Windows preview promotion | Query all enabled instances | Check cache if entering Usage |
| Manual refresh | Query selected instance | No query |
| Enter Usage or switch its instance | No query | Check that instance's cache |
| Task activity, rendering, reset time, Windows hover preview | No query | No query |
| Close panel or leave Usage | No cancellation | No new query |

If Usage opens before its source is registered, its history-entry intent is fulfilled once when the source becomes available. Replacing the visible source or falling back after instance removal is another source-entry boundary; unchanged-source snapshot polling never rechecks the history cache.

The default quota interval is five minutes. The application configuration directory's `usage.json` stores `refreshIntervalMinutes`, an integer from 1 to 60 shared by all instances. Missing or damaged configuration falls back to five minutes. Applying a setting restarts the wait without immediately querying. Every quota completion, successful or failed, starts a new interval. A request already in flight absorbs additional triggers without a queued follow-up. After sleep, an overdue timer starts one query. History has its own five-minute cache measured from completion, including failures; it is checked only at page entry or instance selection.

Each authentication home has a Companion-owned background worker. Concurrent quota and history queries share its temporary native app-server, with isolated runtime, configuration home, SQLite home and authentication settings. The server uses strict configuration loading. After `initialize` and `initialized`, a read-only `config/read` verifies the effective authentication store, service URL and SQLite directory. Quota then sends exactly one `account/rateLimits/read`; history sends `account/usage/read` with a separate request ID and deadline. Neither sends `account/read`, creates a thread/turn, or requests model inference. The 20-second deadline includes the protocol exchange. Quota publishes independently of a slow or failed history read. Only Companion-owned children are cleaned up; existing user daemons are not connected to or stopped. The CLI may make multiple internal HTTP requests; the guarantee concerns the business RPC call.

Each instance has independent quota and history snapshots: the last successful response and timestamp, completion timestamp, loading flag, latest error, next query time and elapsed query milliseconds. Failures retain the last successful response only while its account ownership remains verified. Only quota failure marks retained percentages with `*`; age, reset time and loading alone do not. A successful response lacking a window clears that window and displays `—`. A passed reset adds an explanatory message without estimating a replacement percentage.

Ownership includes user, workspace, authentication storage and service source, and exists only in memory. File credentials and supported OS credential stores are read without interactive authorization; credential contents and identity fingerprints are not logged or serialized into snapshots. A normal token rotation leaves ownership and the refresh schedule unchanged. Changing accounts, signing out, changing authentication sources, or failing to verify ownership clears both caches and invalidates old request generations. Late results cannot restore an earlier account's readings. Responses that identify another workspace are rejected. Unsupported effective authentication configurations display an explicit unavailable state rather than guessing which account is active. Account results are never persisted.

System `requirements.toml` may contain supported non-authentication requirements for hooks, approval policies/reviewers, sandbox and permission profiles, web search, network restrictions, rules, apps and MCP servers. These requirements remain loaded and enforced by the native server; they are never merged as user configuration or removed. Feature requirements currently support `hooks` and `remote_control` booleans. Authentication requirements such as `allowed_login_methods` and `allowed_chatgpt_workspaces`, unknown requirement keys, malformed files, legacy `managed_config.toml` and macOS managed preferences still produce an explicit unavailable state. The native effective authentication store, service and SQLite directory must match before any usage RPC is sent.

SQLite-directory verification accepts identical absolute paths or absolute paths that canonicalize to the same directory. This includes ordinary Windows drive paths and their `\\?\` forms returned by filesystem canonicalization. Different directories, relative paths and unresolved unequal paths remain rejected; account and authentication-store checks are unchanged.

On macOS an existing App runtime supplied by instance discovery is used directly. If no runtime was supplied, usage discovery accepts installed native TUI binaries and resolves official npm package layouts to the native binary without executing their JavaScript entry point, then falls back to a Codex-identified `Codex.app` or `ChatGPT.app` in the standard Applications directories. Unknown wrappers and known secondary-instance runtimes are excluded. Reading usage does not require the complete package manifest used for explicit TUI maintenance.

GUI task scans continue independently and no longer read local quota logs. Claude usage and headless monitoring retain their previous behavior.

## Verification

- Core scheduler tests use supplied timestamps and synthetic completions to exercise triggers, merging, retry spacing, sleep, interval changes, identity invalidation and independent history caches.
- App-server tests use synthetic streams/processes to assert the handshake and single business request, response validation, timeout and cleanup, and isolated routing. They do not query a real account.
- macOS native UI tests use an injected bridge and a Rust-serialized FFI fixture. They cover page/panel events, failed-reading markers, reset presentation, missing windows and source isolation.
- Windows lifecycle and presentation tests cover formal panel entry versus preview, query-independent rendering, failed-reading markers and persisted display behavior.

Run the workspace tests on macOS and Windows for native platform coverage. A cross-target `cargo check` verifies Windows compilation but does not execute its native lifecycle tests.

### Local verification (2026-10-07)

- All 29 usage tests passed. New regressions were reproduced before the fix, including hooks-only requirements, native TUI discovery without an updater manifest, official npm layouts and rejection of secondary-instance runtimes. Six npm fixture combinations cover nested, hoisted and bundled packages with both native-path generations; they do not constitute a live npm installation test.
- Native CLI 0.160.1 and the App's bundled runtime 0.155.0 passed effective-configuration and quota probes with an existing ChatGPT login and the same isolated environment and explicit overrides used by Companion. No model task was created.
- An installed local macOS fix build restored quota, lifetime tokens and the last seven reported days of usage. The running executable was checked against the installed artifact before UI verification.
- Strict workspace all-target Clippy (`-D warnings`), formatting and diff checks passed. Windows GUI was not manually tested for this change; CI build and automated test results are separate from native GUI validation.

### Local verification (2026-09-27)

For the subsequent account-ownership, isolation and paired-update changes, see the [2026-10-02 validation record](CODEX_DODEX_VALIDATION.md). The results below describe the earlier baseline.

- macOS app unit tests: 62 passed; core: 225 passed, 2 existing ignored; core with `server`: 231 passed, 2 existing ignored.
- The complete Rust-to-Swift FFI/native UI suite passed, as did the real Slint editor's 29 phases, including isolated refresh-setting validation and persistence.
- Portable tests executing Windows production presentation/navigation code: 25 passed. Windows `--all-targets` compilation/lint and the GNU executable build/link passed using mingw-w64.
- Both platforms' Clippy checks used `-D warnings`; formatting and whitespace checks passed.
- A synthetic app-server measured 509 ms from startup to quota completion while the separate history request was still waiting. This is a local fixture measurement, not a live-account latency benchmark. The test also asserts the event entry point returns in under 200 ms and that quota does not wait for history timeout.
- Actual Windows native GUI/process lifecycle execution still requires a Windows host; cross-compilation and portable tests do not replace it. No live account queries or deployment were needed for this validation.
