# Windows validation

## v0.3.32 PowerShell module compatibility (2026-10-09)

Local migration with the v0.3.31 package stopped at native package signature
verification: Windows PowerShell inherited PowerShell 7's `PSModulePath` and
could not load `Microsoft.PowerShell.Security`. A diagnostic using the explicit
Windows PowerShell built-in module directory reported a valid OpenAI signature
for the same native executable.

The v0.3.32 change gives Companion's Windows PowerShell maintenance processes
an explicit matching module root for installation, junction creation and
signature verification. Terminal launch retains the caller's module path.
Package verification continues to require `Valid` signatures and an OpenAI
signer for both native executables.

A targeted Windows `acomp` regression passed against the saved fix. It uses
real Windows PowerShell with inherited PowerShell 7 and synthetic conflicting
module paths, checks a signed system executable through the matching security
module, and exercises a temporary installer script, instance environment,
junction and hidden native child. The child retains the inherited module paths,
including a Unicode fixture path. The test modifies no installed TUI or personal
profile. Formatting, diff whitespace and locked offline package metadata checks
also passed; all three workspace packages report version 0.3.32.

The v0.3.31 GUI was installed and started locally, and the recorded personal
baseline checks passed. The subsequent TUI migration did not complete; these
results do not count as a successful local repair. The pushed v0.3.31 tag is
retained, and its release remains a draft while v0.3.32 is prepared.

v0.3.32 full workspace checks, native CI, release-archive verification and
authorized local repair are pending at this documentation stage. Results below belong to
the named v0.3.31 source or earlier candidates and are not v0.3.32 acceptance.

## v0.3.31 independent TUI verification (2026-10-09)

This revision removes the Dodex desktop mirror. Codex and Dodex have separate
native console entries, complete packages, accounts, histories, databases, logs,
daemon state and update prefixes. Existing secondary profiles stay in place;
the primary retains its installation channel. The earlier records below describe
historical desktop implementations and are not current installation guidance.

### Final Windows source checks

The completion changes pass workspace formatting, Clippy for all targets with
warnings denied, and the core build without default features. The final local
workspace run passes 580 test executions with 10 explicitly ignored; the separate
`server` core run passes 258 with 2 ignored. These suites overlap and are not
added together as unique cases. The Python contract run passes its two portable
retirement cases; 15 native opt-in cases remain skipped on this host.

The completion regressions cover npm command precedence and native resume,
owned PATH aliases across repeated repairs, the native Programs known folder,
and unchanged configuration bytes for equivalent Windows storage paths. Named
profile files and trusted project config layers cannot redirect Dodex storage
or credentials; safe native model and MCP settings remain available. Raw parent
path segments, redirected directories and different storage remain rejected.

Synthetic Settings renders were inspected at the end of this run: the TUI setup
page has separate Codex and Dodex version/update rows and terminal actions, with
no duplicated desktop App control. These source checks do not establish a
successful local TUI migration; the later Windows PowerShell failure is recorded
above. The candidate runs below are evidence from earlier branch revisions.

### Earlier candidate checks

Native Windows MSVC tests cover command/environment isolation, dynamically
selected versions, stdin and working directories, child exit codes, Ctrl+C and
Ctrl+Break, concurrent maintenance, interrupted publication, repeated repair,
legacy desktop retirement and preservation of unrelated commands and shortcuts.
The workspace suite passes 555 test executions with 10 explicitly ignored;
the separate `server` core run passes 245 with 2 ignored. These groups overlap.
The [native Release build](https://github.com/WXGopher/agent-companion/actions/runs/37838141464)
at `88cb306` passes formatting, build, Clippy, these test suites and optimized
packaging. Publishing was skipped. The downloaded x64 archive's checksum and
GitHub provenance were verified; it contains five executables and the migration
guide, with no Dodex desktop launcher.

Test-owned native Windows settings and task windows receive pointer events at
1×, 1.5× and 2× scale. The checks exercise TUI installation, opening a terminal,
both independent update actions, busy-state guards and terminal task navigation.
CI preserves the resulting render artifacts; `windows-dual.png` is a native
Windows render with synthetic values.

The real-package fixture installs different official releases, compares native
command help and login status, installs Companion's terminal entries, repairs
again without copying authentication, updates only Dodex and verifies the
primary package tree is unchanged. A failed native update leaves the previous
version runnable. Two synthetic loopback accounts exercise separate daemons,
multiple clients, resuming an active turn, queue, restart and recovery.
The [candidate acceptance run](https://github.com/WXGopher/agent-companion/actions/runs/37842931266)
at `d39f71c` passes on both Windows and macOS using the optimized Release
artifacts above. It verifies provenance and unchanged product source before
running the fixture. One Windows attempt returned `Access is denied` during
the native 0.159.3 daemon's first installation; the same source and artifact
passed in a fresh environment on rerun. No automatic product retry or
daemon-disable fallback was added to conceal that result.

On a developer's Windows computer, run `scripts/test-tui-instances.py` from a
non-elevated terminal. The GitHub-hosted runner instead creates a disposable
standard account on its disposable VM. A temporary test service loads that account's
profile and launches the fixture with its standard-user token, outside runner
and credential-launcher process jobs. An early preflight verifies the ordinary
account can create detached children before Rust builds or package downloads.
The service, account, profile and owned processes are removed afterward.
The installed product's console entry uses the vendor's ordinary launch behavior.
The fixture captures command output in files and waits for the command process
itself, so a native daemon retaining an output handle does not keep an anonymous
pipe open indefinitely. Native daemon sockets are reached through Winsock when
Windows CPython does not expose AF_UNIX. These changes apply to the acceptance
harness, not the installed console launch path.

These automated native checks do not claim manual acceptance on a user's
physical Windows desktop. Real account billing, existing terminal focus,
notifications, Explorer restart and multiple-monitor placement remain separate
manual scenarios. Ignored machine-dependent cases are not counted as passed.

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
