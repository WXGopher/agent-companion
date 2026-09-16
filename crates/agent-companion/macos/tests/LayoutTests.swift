// SPDX-License-Identifier: GPL-3.0-only
// Render the actual SwiftUI component with synthetic local data. No desktop
// interaction, Codex processes, network requests or production data involved.
import AppKit
import SwiftUI

@_cdecl("agent_companion_snapshot_json")
func fixtureSnapshot() -> UnsafeMutablePointer<CChar>? {
    guard let path = ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_SNAPSHOT"],
          let text = try? String(contentsOfFile: path, encoding: .utf8) else { return strdup("{}") }
    return strdup(text)
}
@_cdecl("agent_companion_release_json")
func freeFixture(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }

@main
struct LayoutTests {
    @MainActor static func main() throws {
        if CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--write-dock-preference" {
            let store = DockPreferenceStore(domain: CommandLine.arguments[2])
            precondition(DockPreferences(store: store).setVisible(CommandLine.arguments[3] == "true"))
            return
        }
        _ = NSApplication.shared
        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let model = CompanionModel()
        if ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_SNAPSHOT"] != nil {
            model.refresh()
            precondition(model.snapshot.error == nil && model.snapshot.activeCount == 7)
            precondition(model.weeklyText == "55%" && model.snapshot.codexHome == "/synthetic/.codex")
            print("Rust → Swift snapshot serialization and allocation/release: passed")
        }
        model.snapshot = CodexSnapshot(activeCount: 2, completedCount: 1, tasks: [
            CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a0", title: "Build the compact Codex notch", project: "agent-companion", cwd: "/synthetic/agent-companion", client: "desktop", state: "running", updatedAt: Date().timeIntervalSince1970 - 30, transcriptPath: nil),
            CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a1", title: "Review navigation and keyboard access", project: "workspace", cwd: "/synthetic/workspace", client: "cli", state: "waiting", updatedAt: Date().timeIntervalSince1970 - 65, transcriptPath: nil),
            CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a2", title: "Run shared configuration checks", project: "agent-companion", cwd: nil, client: "cli", state: "completed", updatedAt: Date().timeIntervalSince1970 - 120, transcriptPath: nil)
        ], weekly: WeeklyUsage(usedPercent: 32, resetsAt: Date().timeIntervalSince1970 + 340000, expired: false), loading: false)
        precondition(model.visibleTasks.count == 2 && model.weeklyText == "68%")
        try render(model, name: "compact-external", output: output, height: 28...28)
        model.expanded = true
        try render(model, name: "active-external", output: output, height: 300...480)
        model.expanded = false
        model.cameraWidth = 184
        model.compactHeight = 32
        try render(model, name: "compact-notched", output: output, height: 32...32)
        precondition(model.compactWidth == 292, "Compact wings grew beyond the two counters and usage label")
        let weekly = model.snapshot.weekly
        model.snapshot.weekly = WeeklyUsage(usedPercent: 0, resetsAt: weekly?.resetsAt, expired: false)
        precondition(model.weeklyText == "100%")
        try render(model, name: "compact-full-quota", output: output, height: 32...32)
        model.snapshot.weekly = weekly
        model.snapshot.activeCount = 120
        model.snapshot.completedCount = 101
        try render(model, name: "large-counts", output: output, height: 32...32)
        model.snapshot.activeCount = 2
        model.snapshot.completedCount = 1
        model.expanded = true
        try render(model, name: "active", output: output, height: 300...420)
        model.showingCompleted = true
        precondition(model.visibleTasks.count == 1)
        try render(model, name: "finished", output: output, height: 230...370)
        model.showingCompleted = false
        model.failedTask = model.snapshot.tasks[0]
        model.message = "Could not select the original terminal tab. Allow Agent Companion in System Settings → Privacy & Security → Automation, or copy the resume command."
        try render(model, name: "jump-error", output: output, height: 400...650)
        model.cameraWidth = 0
        model.compactHeight = 28
        try render(model, name: "jump-error-external", output: output, height: 400...750)
        model.cameraWidth = 184
        model.compactHeight = 32
        model.message = nil
        model.failedTask = nil
        model.snapshot = CodexSnapshot(loading: false)
        precondition(model.weeklyText == "—" && model.visibleTasks.isEmpty)
        try render(model, name: "empty", output: output, height: 300...430)
        model.snapshot.weekly = WeeklyUsage(usedPercent: 100, resetsAt: 0, expired: true)
        precondition(model.weeklyText == "—")
        model.snapshot.error = "Could not read Codex sessions: permission denied."
        try render(model, name: "read-error", output: output, height: 330...480)
        model.snapshot = CodexSnapshot(loading: true)
        try render(model, name: "loading", output: output, height: 300...430)
        verifyResize()
        verifyTerminalTargets()
        verifyNavigationLifecycle()
        verifyHoverLifecycle()
        verifyScreenEdgeHover()
        try verifyDockPreferencePersistence()
        print("PASS: 12 native SwiftUI layouts; filters, remaining quota, errors and constant-width expand/collapse sizing")
    }

    private static func verifyScreenEdgeHover() {
        let screen = CGRect(x: 0, y: 0, width: 1920, height: 1080)
        let compact = CGRect(x: 852, y: 1052, width: 216, height: 28)
        let expanded = CGRect(x: 852, y: 700, width: 216, height: 380)
        for point in [CGPoint(x: 960, y: 1080), CGPoint(x: 844, y: 1080),
                      CGPoint(x: 1076, y: 1070), CGPoint(x: 960, y: 1044)] {
            precondition(NotchHoverRegion.contains(point, panel: compact, screen: screen),
                         "The top edge or hover margin was excluded: \(point)")
        }
        for point in [CGPoint(x: 960, y: 1081), CGPoint(x: 839, y: 1080),
                      CGPoint(x: 1081, y: 1080), CGPoint(x: 960, y: 1043),
                      CGPoint(x: 40, y: 1080), CGPoint(x: 2000, y: 1080)] {
            precondition(!NotchHoverRegion.contains(point, panel: compact, screen: screen),
                         "The hover region leaked onto another display or unrelated menu icons: \(point)")
        }
        let offset = CGPoint(x: -1920, y: -1080)
        precondition(NotchHoverRegion.contains(CGPoint(x: -960, y: 0),
            panel: compact.offsetBy(dx: offset.x, dy: offset.y),
            screen: screen.offsetBy(dx: offset.x, dy: offset.y)))

        var pointer = CGPoint(x: 40, y: 1080)
        var frame = compact
        var expansions = 0
        var collapses = 0
        let hover = NotchHover(
            containsPointer: { NotchHoverRegion.contains(pointer, panel: frame, screen: screen) },
            expand: { frame = expanded; expansions += 1 },
            collapse: { frame = compact; collapses += 1 }
        )
        func elapse(_ seconds: TimeInterval) {
            RunLoop.main.run(until: Date().addingTimeInterval(seconds))
        }
        hover.start()
        hover.start() // Starting again must not create a second sampler.
        pointer = CGPoint(x: 960, y: 1080) // Deliberately no mouse event.
        elapse(0.45)
        precondition(expansions == 1 && collapses == 0, "A stationary pointer at the top edge did not expand")
        pointer = CGPoint(x: 844, y: 800) // Move down through the expanded panel's side margin.
        elapse(0.45)
        precondition(collapses == 0, "Moving from the strip into the panel collapsed it")
        frame = compact
        pointer = CGPoint(x: 960, y: 1080)
        hover.dismiss()
        elapse(0.45)
        precondition(expansions == 1, "Polling reopened the panel after Esc at the top edge")
        pointer = CGPoint(x: 40, y: 1080)
        elapse(0.2)
        pointer = CGPoint(x: 1076, y: 1080)
        elapse(0.45)
        precondition(expansions == 2, "Leaving and returning to the margin did not re-arm hover")
        pointer = CGPoint(x: 40, y: 1080)
        elapse(0.55)
        precondition(collapses == 1, "Polling did not dismiss after leaving the expanded panel")
        hover.stop()
        pointer = CGPoint(x: 960, y: 1080)
        elapse(0.45)
        precondition(expansions == 2, "Polling continued after shutdown")
        print("Screen-edge hover: inclusive top, margins, display clipping, missing events, panel entry and dismissal passed")
    }

    private static func verifyHoverLifecycle() {
        // Exercise the actual dwell timers without moving the user's pointer
        // or requesting Accessibility access to synthesize input.
        func elapse(_ seconds: TimeInterval) {
            RunLoop.main.run(until: Date().addingTimeInterval(seconds))
        }
        var inside = false
        var expansions = 0
        var collapses = 0
        let hover = NotchHover(containsPointer: { inside }, expand: { expansions += 1 }, collapse: { collapses += 1 })
        hover.update()
        inside = true
        for _ in 0..<6 {
            hover.update()
            elapse(0.05)
        }
        precondition(expansions == 1 && collapses == 0, "Movement within the notch kept cancelling hover expansion")
        inside = false
        hover.update()
        elapse(0.08)
        inside = true
        hover.update()
        elapse(0.40)
        precondition(collapses == 0, "A brief exit collapsed the panel after re-entry")
        inside = false
        hover.update()
        elapse(0.40)
        precondition(collapses == 1, "Leaving an unpinned panel did not collapse it")

        inside = true
        hover.update()
        let beforeDismiss = expansions
        hover.dismiss()
        hover.update()
        elapse(0.25)
        precondition(expansions == beforeDismiss, "Esc reopened the panel while the pointer stayed inside")
        inside = false
        hover.update()
        inside = true
        hover.update()
        elapse(0.25)
        precondition(expansions == beforeDismiss + 1, "Hover did not re-arm after leaving and returning")

        hover.pin()
        inside = false
        hover.update()
        elapse(0.40)
        precondition(collapses == 1, "A click-pinned panel collapsed on mouse exit")
        hover.dismiss()
        inside = true
        hover.update()
        let beforeStop = expansions
        hover.stop()
        elapse(0.25)
        precondition(expansions == beforeStop, "A pending hover fired after shutdown")

        let missedExit = NotchHover(containsPointer: { inside }, expand: { expansions += 1 }, collapse: {})
        missedExit.update()
        inside = false // No tracking event: the deadline must sample again.
        elapse(0.25)
        precondition(expansions == beforeStop, "An old enter event expanded after the pointer left")
        missedExit.stop()
        print("Hover lifecycle: dwell during movement, exit/re-entry, dismissal, click pinning and stale events passed")
    }

    private static func verifyDockPreferencePersistence() throws {
        // Never change the user's real app or Codex preferences. A fresh
        // subprocess exercises the editor → notch notification and persistence.
        let domain = "com.wxgopher.agent-companion.tests.\(UUID().uuidString)"
        let store = DockPreferenceStore(domain: domain)
        let originalPolicy = NSApp.activationPolicy()
        defer {
            NSApp.setActivationPolicy(originalPolicy)
            CFPreferencesSetAppValue("showDockIcon" as CFString, nil, domain as CFString)
            CFPreferencesAppSynchronize(domain as CFString)
        }
        precondition(!store.read(), "A fresh install must not show a Dock icon")
        let preferences = DockPreferences(store: store)
        preferences.start()
        precondition(NSApp.activationPolicy() == .accessory)
        for visible in [true, false] {
            let child = Process()
            child.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
            child.arguments = ["--write-dock-preference", domain, String(visible)]
            try child.run()
            child.waitUntilExit()
            precondition(child.terminationStatus == 0)
            let expected: NSApplication.ActivationPolicy = visible ? .regular : .accessory
            let deadline = Date().addingTimeInterval(2)
            while NSApp.activationPolicy() != expected && Date() < deadline {
                RunLoop.main.run(until: Date().addingTimeInterval(0.02))
            }
            precondition(NSApp.activationPolicy() == expected, "The other process's Dock change was not applied")
            precondition(store.read() == visible, "Dock preference did not persist across processes")
        }
        print("Dock preference: hidden by default; native policy changes and cross-process persistence passed")
    }

    @MainActor private static func verifyNavigationLifecycle() {
        var pending: ((String?) -> Void)?
        let model = CompanionModel { _, _, completion in pending = completion }
        let task = CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a0", title: "Navigation fixture", project: "fixture", cwd: nil, client: "cli", state: "running", updatedAt: 0, transcriptPath: nil)
        var collapses = 0
        model.collapse = { [weak model] in
            collapses += 1
            model?.expanded = false
            model?.message = nil
            model?.failedTask = nil
        }
        model.expanded = true
        model.jump(to: task)
        precondition(model.jumpingID == task.id)
        model.expanded = false
        model.expanded = true
        pending?(nil)
        precondition(model.expanded && collapses == 0 && model.jumpingID == nil,
                     "An old jump dismissed the newly reopened panel")
        model.jump(to: task)
        model.expanded = false
        model.expanded = true
        pending?("An old navigation error")
        precondition(model.message == nil && model.failedTask == nil && model.jumpingID == nil,
                     "A dismissed jump left an error in a new presentation")
        model.jump(to: task)
        pending?("The original terminal could not be located.")
        precondition(model.message != nil && model.failedTask?.id == task.id && model.expanded)
        model.jump(to: task)
        pending?(nil)
        precondition(!model.expanded && collapses == 1 && model.jumpingID == nil)
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

    @MainActor private static func verifyResize() {
        let model = CompanionModel()
        let host = NSHostingView(rootView: CompanionView(model: model))
        host.sizingOptions = [.intrinsicContentSize]
        let window = NSWindow(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        precondition(host.fittingSize.height == 28)
        model.expanded = true
        RunLoop.main.run(until: Date().addingTimeInterval(0.2))
        let expanded = host.fittingSize
        precondition(expanded.width == model.compactWidth && expanded.height > 300, "Expansion changed the top-edge width: \(expanded)")
        window.setContentSize(expanded)
        model.expanded = false
        RunLoop.main.run(until: Date().addingTimeInterval(0.2))
        let collapsed = host.fittingSize
        precondition(collapsed == CGSize(width: 216, height: 28), "Collapsed hosting view kept an invisible hit area: \(collapsed)")
        window.close()
    }

    @MainActor private static func render(_ model: CompanionModel, name: String, output: URL, height: ClosedRange<CGFloat>) throws {
        let view = CompanionView(model: model).fixedSize()
        // An offscreen hosting window includes macOS's native ScrollView, which
        // SwiftUI ImageRenderer intentionally omits from its drawing output.
        let host = NSHostingView(rootView: view)
        host.appearance = NSAppearance(named: .darkAqua)
        let size = host.fittingSize
        let window = NSWindow(contentRect: CGRect(origin: .zero, size: size), styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.backgroundColor = .clear
        window.isOpaque = false
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        precondition(size.width == model.compactWidth, "Unexpected width for \(name): \(size)")
        precondition(height.contains(size.height), "Unexpected height for \(name): \(size)")
        guard let image = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { fatalError("Could not render \(name)") }
        host.cacheDisplay(in: host.bounds, to: image)
        try image.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("\(name).png"))
        window.close()
        print("\(name): \(size.width) × \(size.height)")
    }
}
