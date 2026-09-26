// SPDX-License-Identifier: GPL-3.0-only
// Render the actual SwiftUI component with synthetic local data. No desktop
// interaction, Codex processes, network requests or production data involved.
import AppKit
import Combine
import SwiftUI

@_cdecl("agent_companion_snapshot_json")
func fixtureSnapshot() -> UnsafeMutablePointer<CChar>? {
    guard let path = ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_SNAPSHOT"],
          let text = try? String(contentsOfFile: path, encoding: .utf8) else { return strdup("{}") }
    return strdup(text)
}
@_cdecl("agent_companion_release_json")
func freeFixture(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }

// A single process-lifetime allocation matches the production bridge's borrowed
// static storage. Cargo supplies its build version; standalone runs use a fixture.
private let fixtureAppVersion = strdup(ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_VERSION"] ?? "0.0.0-test")!
@_cdecl("agent_companion_version")
func fixtureVersion() -> UnsafePointer<CChar> { UnsafePointer(fixtureAppVersion) }

@main
struct LayoutTests {
    @MainActor static func main() throws {
        // Preserve the last completed scenario if a native assertion aborts CI.
        setbuf(stdout, nil)
        if CommandLine.arguments.dropFirst().first == "app-server" {
            try SubscriptionUsageTests.serveFixture()
            return
        }
        if CommandLine.arguments.contains("--live-subscription") {
            SubscriptionUsageTests.liveRead()
            return
        }
        _ = NSApplication.shared
        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        NSApp.setActivationPolicy(.accessory)
        var failure: Error?
        var checksCompleted = false
        // Native popovers need AppKit's completed startup and event loop. Running
        // a plain RunLoop after NSApplication.shared does not finish launching.
        let launched = NotificationCenter.default.addObserver(forName: NSApplication.didFinishLaunchingNotification,
                                                               object: NSApp, queue: .main) { _ in
            // Leave the launch callback before testing any native windows. A
            // RunLoop callback allows nested waits to service dispatch-main work.
            // A main-queue block would prevent deferred UI jobs from running.
            Timer.scheduledTimer(withTimeInterval: 0.001, repeats: false) { _ in
                MainActor.assumeIsolated {
                    do {
                        try runChecks(output: output)
                        checksCompleted = true
                    } catch { failure = error }
                    NSApp.stop(nil)
                    if let wake = NSEvent.otherEvent(with: .applicationDefined, location: .zero,
                                                    modifierFlags: [], timestamp: 0, windowNumber: 0,
                                                    context: nil, subtype: 0, data1: 0, data2: 0) {
                        NSApp.postEvent(wake, atStart: true)
                    }
                }
            }
        }
        defer { NotificationCenter.default.removeObserver(launched) }
        NSApp.run()
        if let failure { throw failure }
        precondition(checksCompleted, "The native event loop exited before all requested checks completed")
    }

    @MainActor private static func runChecks(output: URL) throws {
        let suites: [(String, () throws -> Void)] = [
            ("--menu-lifecycle", { MenuBarPlacementTests.run(); MenuBarLifecycleTests.run() }),
            ("--menu-bar", { try MenuBarUsageTests.run(output: output) }),
            ("--subscription-usage", { try SubscriptionUsageTests.run(output: output) }),
            ("--instance-quotas", { try InstanceQuotaTests.run(output: output) }),
            ("--task-tabs", { try TaskTabTests.run(output: output) }),
            ("--page-switches", { try PageSwitchTests.run(output: output) }),
            ("--themes", { try ThemeTests.run(output: output) }),
            ("--popup-header", { try PopupHeaderTests.run(output: output) })
        ]
        if let selected = suites.first(where: { CommandLine.arguments.contains($0.0) }) {
            try selected.1()
            return
        }
        let model = CompanionModel()
        if ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_SNAPSHOT"] != nil {
            model.refresh()
            precondition(model.snapshot.error == nil && model.snapshot.activeCount == 7)
            precondition(model.weeklyText == "55%" && model.snapshot.codexHome == "/synthetic/.codex")
            print("Rust → Swift snapshot serialization and allocation/release: passed")
        }
        verifySnapshotRefresh()
        verifyNavigationLifecycle()
        verifyTerminalTargets()
        for (_, suite) in suites { try suite() }
    }

    @MainActor private static func verifySnapshotRefresh() {
        let model = CompanionModel()
        var changes = 0
        let subscription = model.objectWillChange.sink { changes += 1 }
        defer { model.stop(); withExtendedLifetime(subscription) {} }
        model.start()
        let initial = changes
        model.start()
        precondition(changes == initial, "Repeated start duplicated the snapshot source")
        let deadline = Date().addingTimeInterval(1.2)
        while Date() < deadline { RunLoop.main.run(mode: .eventTracking, before: deadline) }
        precondition(changes > initial, "Scrolling or an open menu paused the snapshot timer")
        model.stop()
        let stopped = changes
        RunLoop.main.run(until: Date().addingTimeInterval(1.2))
        precondition(changes == stopped, "Snapshots continued after shutdown")
        model.start()
        precondition(changes > stopped, "The snapshot source could not restart")
        print("Snapshot timer: menu/scroll tracking, idempotent start, shutdown and restart passed")
    }

    @MainActor private static func verifyNavigationLifecycle() {
        var pending: ((String?) -> Void)?
        let model = CompanionModel { _, _, completion in pending = completion }
        let task = CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a0", title: "Navigation fixture", project: "fixture", cwd: nil, client: "cli", state: "running", updatedAt: 0, transcriptPath: nil)
        var collapses = 0
        model.dismiss = { [weak model] in
            collapses += 1
            model?.isPresented = false
            model?.message = nil
            model?.failedTask = nil
        }
        model.isPresented = true
        model.jump(to: task)
        precondition(model.jumpingID == task.id)
        model.isPresented = false
        model.isPresented = true
        pending?(nil)
        precondition(model.isPresented && collapses == 0 && model.jumpingID == nil,
                     "An old jump dismissed the newly reopened panel")
        model.jump(to: task)
        model.isPresented = false
        model.isPresented = true
        pending?("An old navigation error")
        precondition(model.message == nil && model.failedTask == nil && model.jumpingID == nil,
                     "A dismissed jump left an error in a new presentation")
        model.jump(to: task)
        pending?("The original terminal could not be located.")
        precondition(model.message != nil && model.failedTask?.id == task.id && model.isPresented)
        model.jump(to: task)
        pending?(nil)
        precondition(!model.isPresented && collapses == 1 && model.jumpingID == nil)
        print("Navigation lifecycle: success/error recovery and stale completion isolation passed")
    }

    private static func verifyTerminalTargets() {
        let rows: [Int32: TerminalJump.ProcessRow] = [
            30: .init(parent: 20, tty: "ttys004", executable: "/usr/local/bin/codex"),
            20: .init(parent: 10, tty: "ttys004", executable: "-zsh"),
            10: .init(parent: 1, tty: "??", executable: "/synthetic/Library/Application Support/iTerm2/iTermServer-3.6.11"),
            40: .init(parent: 20, tty: "ttys004", executable: "/usr/bin/cat")
        ]
        let detached = TerminalJump.processTarget(writers: [30], processes: rows,
            detachedITerm: .init(pid: 99, bundle: "com.googlecode.iterm2"), application: { _ in nil })
        precondition(detached?.tty == "/dev/ttys004" && detached?.appPID == 99)
        let terminal = TerminalJump.processTarget(writers: [30], processes: rows, detachedITerm: nil) { pid in
            pid == 10 ? .init(pid: 10, bundle: "com.apple.Terminal") : nil
        }
        precondition(terminal?.bundle == "com.apple.Terminal" && terminal?.tty == "/dev/ttys004")
        precondition(TerminalJump.processTarget(writers: [40], processes: rows,
            detachedITerm: .init(pid: 99, bundle: "com.googlecode.iterm2"), application: { _ in nil }) == nil)
        precondition(TerminalJump.processTarget(writers: [30], processes: rows,
            detachedITerm: nil, application: { _ in nil }) == nil)
        print("Terminal targets: GUI ancestry and detached iTerm server passed")
    }

}
