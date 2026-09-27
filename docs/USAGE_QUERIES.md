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

Each query starts a temporary instance-specific app-server with isolated runtime, configuration home, SQLite home and authentication settings. After `initialize` and `initialized`, quota sends exactly one `account/rateLimits/read`; history separately sends `account/usage/read`. Neither sends `account/read`, creates a thread/turn, or requests model inference. The 20-second deadline includes the protocol exchange. The quota result does not wait for history. Process cleanup runs on completion, timeout and cancellation. The CLI may make multiple internal HTTP requests; the guarantee concerns the business RPC call.

Each instance has independent quota and history snapshots: the last successful response and timestamp, completion timestamp, loading flag, latest error, next query time and elapsed query milliseconds. Failures retain the last successful response. Only quota failure marks retained percentages with `*`; age, reset time and loading alone do not. A successful response lacking a window clears that window and displays `—`. A passed reset adds an explanatory message without estimating a replacement percentage. Removing or changing a source cancels both requests, discards its state and rejects late completions. Account results are never persisted.

GUI task scans continue independently and no longer read local quota logs. Claude usage and headless monitoring retain their previous behavior.

## Verification

- Core scheduler tests use supplied timestamps and synthetic completions to exercise triggers, merging, retry spacing, sleep, interval changes, identity invalidation and independent history caches.
- App-server tests use synthetic streams/processes to assert the handshake and single business request, response validation, timeout and cleanup, and isolated routing. They do not query a real account.
- macOS native UI tests use an injected bridge and a Rust-serialized FFI fixture. They cover page/panel events, failed-reading markers, reset presentation, missing windows and source isolation.
- Windows lifecycle and presentation tests cover formal panel entry versus preview, query-independent rendering, failed-reading markers and persisted display behavior.

Run the workspace tests on macOS and Windows for native platform coverage. A cross-target `cargo check` verifies Windows compilation but does not execute its native lifecycle tests.

### Local verification (2026-09-27)

- macOS app unit tests: 62 passed; core: 225 passed, 2 existing ignored; core with `server`: 231 passed, 2 existing ignored.
- The complete Rust-to-Swift FFI/native UI suite passed, as did the real Slint editor's 29 phases, including isolated refresh-setting validation and persistence.
- Portable tests executing Windows production presentation/navigation code: 25 passed. Windows `--all-targets` compilation/lint and the GNU executable build/link passed using mingw-w64.
- Both platforms' Clippy checks used `-D warnings`; formatting and whitespace checks passed.
- A synthetic app-server measured 509 ms from startup to quota completion while the separate history request was still waiting. This is a local fixture measurement, not a live-account latency benchmark. The test also asserts the event entry point returns in under 200 ms and that quota does not wait for history timeout.
- Actual Windows native GUI/process lifecycle execution still requires a Windows host; cross-compilation and portable tests do not replace it. No live account queries or deployment were needed for this validation.
