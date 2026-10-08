# Windows validation

## v0.3.29 local verification (2026-10-08)

This record separates automated checks and authorized local deployment from visual desktop acceptance. It covers the Windows dual-instance changes and the subsequent quota-query repair included in v0.3.29.

### Automated checks

| Change | Completed checks |
| --- | --- |
| Dual-instance setup and taskbar readings | 583 tests passed; 10 tests requiring additional local conditions remained ignored. Static checks and the Windows release build passed. |
| SQLite-directory equivalence repair | 26 related tests passed, including equivalent Windows path spellings, rejection of different/relative/unresolved unequal directories, and the native-query fixture. |
| App-bundled TUI version inspection and alignment | 24 software-update tests passed, with one live test ignored by default; 11 maintenance tests passed. Formatting and workspace Clippy checks passed. The separately opted-in read-only Settings snapshot showed matching current/target TUI and App versions with no row errors, labeled the App-bundled source, and left the deployment manifest unchanged. |
| Existing signed-in accounts | The explicitly opted-in `live_windows_instances_publish_allowance` check passed using production source discovery, `UsageService`, and the Windows display snapshot for both Codex and Dodex. Both snapshots contained a successful weekly reading without a query error. |

The counts describe separate validation stages, not a count of unique cases on the release tag. Normal automated tests use fixtures; the ignored live-account check was run separately with authorization. It performs a quota query and does not create a model task. Public records omit credentials, account identifiers and real quota values.

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
