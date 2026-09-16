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
        _ = NSApplication.shared
        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let model = CompanionModel()
        if ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_SNAPSHOT"] != nil {
            model.refresh()
            precondition(model.snapshot.error == nil && model.snapshot.activeCount == 7)
            precondition(model.weeklyText == "45%" && model.snapshot.codexHome == "/synthetic/.codex")
            print("Rust → Swift snapshot serialization and allocation/release: passed")
        }
        model.snapshot = CodexSnapshot(activeCount: 2, completedCount: 1, tasks: [
            CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a0", title: "Build the compact Codex notch", project: "agent-companion", cwd: "/synthetic/agent-companion", client: "desktop", state: "running", updatedAt: Date().timeIntervalSince1970 - 30, transcriptPath: nil),
            CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a1", title: "Review navigation and keyboard access", project: "workspace", cwd: "/synthetic/workspace", client: "cli", state: "waiting", updatedAt: Date().timeIntervalSince1970 - 65, transcriptPath: nil),
            CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a2", title: "Run shared configuration checks", project: "agent-companion", cwd: nil, client: "cli", state: "completed", updatedAt: Date().timeIntervalSince1970 - 120, transcriptPath: nil)
        ], weekly: WeeklyUsage(usedPercent: 32, resetsAt: Date().timeIntervalSince1970 + 340000, expired: false), loading: false)
        precondition(model.visibleTasks.count == 2 && model.weeklyText == "32%")
        try render(model, name: "compact-external", output: output, height: 28...28)
        model.expanded = true
        try render(model, name: "active-external", output: output, height: 300...480)
        model.expanded = false
        model.cameraWidth = 184
        model.compactHeight = 32
        try render(model, name: "compact-notched", output: output, height: 32...32)
        precondition(model.compactWidth == 292, "Compact wings grew beyond the two counters and usage label")
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
        print("PASS: 10 native SwiftUI layouts; filters, usage states, errors and constant-width expand/collapse sizing")
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
