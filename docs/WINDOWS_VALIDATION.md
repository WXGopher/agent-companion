# Windows validation

## Local v100.0 verification (2026-10-08)

Windows now uses the taskbar readout without a separate notification-area icon. Its status animation runs independently; the existing panel, quota tooltip and context-menu actions remain available. Local builds default to `100.0`, including optimized builds, and skip automatic/manual Companion release checks and cached update notices. Explicit release builds retain the workspace version and release checks.

- Workspace formatting, Clippy with warnings denied, and the optimized Windows build passed.
- `cargo test --workspace --locked --offline -- --test-threads=1` passed: 608 executions passed and 10 remained ignored. This count includes shared modules exercised by multiple test binaries. The local-build regression checks repeated panel/manual requests, restart behavior and a cached release above `100.0`, without contacting the release service or rewriting the cache.
- Both app and `acomp` reported `100.0` in default debug and optimized builds. An explicitly opted-in release build reported the workspace version for both entries; default debug binaries were restored afterward.
- The existing installed binaries were backed up, all four Companion executables were replaced, and their SHA-256 hashes matched the optimized build. The old app exited through its normal pipe shutdown request and the installed app restarted successfully. Installed app and CLI version outputs were `100.0`.
- Native window inspection confirmed that the restarted process owned a visible readout embedded in `Shell_TrayWnd`. This verifies window presence and attachment; the final appearance and interactive menu behavior were not visually inspected on the user's desktop.

## v0.3.30 local verification (2026-10-08)

This revision adds a manual release check to Settings and repairs clipped or stale taskbar quota tooltips. The results below describe separate local validation stages before the release version bump; overlapping test groups are not added together or presented as a unique total for the release tag.

### Automated checks

| Change | Completed checks |
| --- | --- |
| Manual release check in Settings | 617 tests passed, with 10 ignored. Coverage includes manual request state, stable-release selection, retry after failure, and the explicit release-link action. |
| Taskbar tooltip repair | App tests: 129 passed, 7 ignored. The targeted native tooltip group passed 3 tests, and the Windows indicator group passed 9 tests. One normally ignored hidden Slint test was explicitly run and passed. These groups overlap with the app tests. |
| Static and build checks | Formatting, workspace Clippy with warnings denied, and the Windows release build passed for the local changes. |

The new tests use synthetic release responses, text and quota values, test-owned windows, and synthetic desktop rectangles. They do not depend on a developer's accounts, credentials, installation paths or real quota readings. Rendered screenshots use synthetic fixtures and are not evidence of acceptance on a user's desktop.

Regression coverage includes the following behavior:

- Settings provide a shared **检查更新** button. Its persistent feedback below the button distinguishes checking, a newer stable release, an up-to-date version and failure. Failed checks can be retried.
- Checking performs no download or installation and does not open a browser. Only the explicit **查看 GitHub Release** action opens the validated release URL; a stale action cannot open a release from an earlier request during a newer check.
- Windows taskbar quota details use a separate, nonactivating native popup. The text is no longer clipped to the embedded readout's small drawing surface, and tooltip text and fonts remain valid through updates and destruction.
- Clicking the readout to open the panel immediately hides its tooltip. The tooltip remains suppressed until the pointer leaves the readout; an open panel also suppresses it.
- Native wrapping and placement were tested at 100%, 125%, 150% and 200% scale, at all four edges and in a narrow synthetic work area. Replacing or destroying the readout invalidates the old tooltip.

### Deployment and manual acceptance

The local development build was installed and Companion restarted after each change. These checks preceded the release version bump and do not validate downloaded release archives. Final user confirmation of the repaired interaction has not been received.

The real Settings update flow, taskbar hover/click behavior, and mixed-DPI or multiple-monitor desktop placement remain manual acceptance scenarios. Synthetic render checks and off-screen native tests do not substitute for those interactions. macOS native CI and release-archive checks are pending at this documentation stage and are recorded separately in the final PR, Release workflow and published release notes.

## v0.3.29 local verification (2026-10-08)

This record separates automated checks and authorized local deployment from visual desktop acceptance. It covers the Windows dual-instance changes and the subsequent quota-query repair included in v0.3.29.

### Automated checks

| Change | Completed checks |
| --- | --- |
| Dual-instance setup and taskbar readings | 583 tests passed; 10 tests requiring additional local conditions remained ignored. Static checks and the Windows release build passed. |
| SQLite-directory equivalence repair | 26 related tests passed, including equivalent Windows path spellings, rejection of different/relative/unresolved unequal directories, and the native-query fixture. |
| App-bundled TUI version inspection and alignment | 24 software-update tests passed, with one live test ignored by default; 11 maintenance tests passed. Formatting and workspace Clippy checks passed. The separately opted-in read-only Settings snapshot showed matching current/target TUI and App versions with no row errors, labeled the App-bundled source, and left the deployment manifest unchanged. |
| Existing signed-in accounts | A separately authorized local acceptance check passed using production source discovery, `UsageService`, and the Windows display snapshot for Codex and Dodex. Both snapshots contained a successful weekly reading without a query error. |
| Short-path shortcut validation and portable test fixtures | The main executable's tests passed: 275 passed, 7 ignored. All four shortcut tests also passed with a dynamically obtained 8.3 temporary-directory alias; the two original shortcut assertions failed under that same condition before the fix. Machine-dependent acceptance probes were removed, and new version fixtures use synthetic values. |

The counts describe separate validation stages, not a count of unique cases on the release tag. Product tests use synthetic fixtures and temporary directories; machine-dependent acceptance probes were removed from the repository after their separately authorized runs. The quota acceptance check queried usage without creating a model task. Public records omit credentials, account identifiers, machine paths and real quota values.

Regression coverage includes the following behavior:

- The Start menu shortcut targets a stable GUI launcher; the PATH command remains a separate console launcher. Both use the independent Dodex profile.
- Existing deployments retain supported schema-2 registration and can migrate known legacy launchers. Unrelated or modified command entries are not overwritten.
- When a standalone TUI installation is absent, a verified official desktop package can supply its bundled native CLI. Invalid or incomplete standalone installations do not silently select this fallback. Existing npm commands remain unchanged.
- Settings use the same source precedence for local TUI version inspection and alignment. App-bundled CLI references are labeled, local prerelease versions are supported, and alignment protects newer Dodex TUI versions from an indirect downgrade during App replacement. Independent stable-channel TUI updates still require a complete standalone installation.
- Codex (`C`) and Dodex (`D`) have separate weekly readings, loading/error states, and task status. Switching or signing out of one account does not reuse the other account's readings.
- Native `config/read` may report an ordinary Windows path while Companion holds its canonical `\\?\` form. Equivalent absolute database directories now pass verification; other identity and effective-configuration checks remain enforced.

### Installed local build

- Existing local changes and installed Companion binaries were backed up before replacement.
- Updated Companion binaries and the Dodex GUI/CLI launchers were installed. Start menu registration, CLI entry points and process startup were checked.
- Existing secondary-account data, sessions and personal settings were retained. Both account queries succeeded after the quota repair without signing in again.
- Companion was restarted with the repaired local build. These deployment checks used a locally built revision before the release version bump; they do not claim verification of the subsequently downloaded release archives.

### Unverified manual scenarios

Desktop automation was unavailable. The final taskbar `C` / `D` layout, popup appearance, and interactive GUI launch/focus behavior have not been visually accepted for this revision. A successful process start or display snapshot does not substitute for that check.

The following remain manual regression scenarios: launching both apps from the final installed shortcuts, opening the final taskbar panel, clicking each instance's task, signed-out/login UI, Explorer or system restart, and mixed-DPI taskbar placement. Fresh account onboarding and the complete paired-update matrix were not repeated during this repair.

macOS GUI was not manually revalidated from this Windows host. Final PR CI and the tag's Release workflow record native automated coverage for Windows MSVC and macOS arm64 separately. Release-archive validation and provenance are recorded in the published release notes after those checks complete.
