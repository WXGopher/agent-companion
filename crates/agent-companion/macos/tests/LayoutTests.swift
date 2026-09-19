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
        if CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--write-dock-preference" {
            let store = DockPreferenceStore(domain: CommandLine.arguments[2])
            precondition(DockPreferences(store: store).setVisible(CommandLine.arguments[3] == "true"))
            return
        }
        if CommandLine.arguments.count == 5, CommandLine.arguments[1] == "--write-display-preference" {
            let store = DisplayPreferenceStore(domain: CommandLine.arguments[2])
            let display = CompanionDisplay(id: CommandLine.arguments[3], name: CommandLine.arguments[4])
            precondition(DisplayPreferences(store: store).select(display.id, displays: [display]))
            return
        }
        _ = NSApplication.shared
        let output = URL(fileURLWithPath: CommandLine.arguments[1])
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        if CommandLine.arguments.contains("--entries") {
            EntryPreferenceTests.run()
            return
        }
        if CommandLine.arguments.contains("--menu-bar") {
            try MenuBarUsageTests.run(output: output)
            return
        }
        if CommandLine.arguments.contains("--subscription-usage") {
            try SubscriptionUsageTests.run(output: output)
            return
        }
        if CommandLine.arguments.contains("--instance-quotas") {
            try InstanceQuotaTests.run(output: output)
            return
        }
        if CommandLine.arguments.contains("--task-tabs") {
            try TaskTabTests.run(output: output)
            return
        }
        if CommandLine.arguments.contains("--page-switches") {
            try PageSwitchTests.run(output: output)
            return
        }
        if CommandLine.arguments.contains("--morph") {
            for camera in [false, true] {
                let directory = output.appendingPathComponent(camera ? "camera-morph" : "external-morph")
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                try verifyMorphing(output: directory, camera: camera)
            }
            return
        }
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
        precondition(model.workingCount == 1 && model.needsInput, "Waiting sessions must not inflate the working count")
        precondition(model.summaryDescription.contains("1 working, approval or input needed"))
        // Tasks and Usage reserve one shared page height, including on first
        // open. This budget includes the 330-point (density-scaled) usage list.
        let standardPageHeight: ClosedRange<CGFloat> = 450...500
        try render(model, name: "compact-external", output: output, height: 28...28)
        model.expanded = true
        try render(model, name: "active-external", output: output, height: standardPageHeight)
        model.expanded = false
        model.metrics = cameraMetrics()
        try render(model, name: "compact-notched", output: output, height: 32...32)
        precondition(max(model.cameraLeftWidth, model.cameraRightWidth) <= 37,
                     "A working session and question mark must fit in narrow camera wings")
        let tasks = model.snapshot.tasks
        let waitingWidth = model.compactWidth
        model.snapshot.tasks = [tasks[0], tasks[2]]
        precondition(model.workingCount == 1 && !model.needsInput)
        precondition(!model.summaryDescription.contains("input needed"))
        precondition(model.compactWidth < waitingWidth, "Clearing pending input should reclaim its menu-bar space")
        try render(model, name: "compact-working", output: output, height: 32...32)
        let workingWidth = model.compactWidth
        model.snapshot.completedCount = 1000
        precondition(model.compactWidth == workingWidth, "Finished tasks should not widen the compact strip")
        model.snapshot.completedCount = 1
        model.snapshot.tasks = [tasks[1]]
        precondition(model.workingCount == 0 && model.needsInput)
        try render(model, name: "compact-waiting-only", output: output, height: 32...32)
        model.snapshot.tasks = (0..<120).map { taskFixture($0, state: "waiting") }
        precondition(model.workingCount == 0 && model.needsInput && model.compactWidth == waitingWidth,
                     "Multiple waiting sessions must share one question mark")
        try render(model, name: "compact-many-waiting", output: output, height: 32...32)
        model.snapshot.tasks = []
        precondition(model.workingCount == 0 && !model.needsInput)
        try render(model, name: "compact-idle", output: output, height: 32...32)
        model.snapshot.tasks = tasks
        let weekly = model.snapshot.weekly
        model.snapshot.weekly = WeeklyUsage(usedPercent: 99, resetsAt: weekly?.resetsAt, expired: false)
        try render(model, name: "compact-low-quota", output: output, height: 32...32)
        model.snapshot.weekly = WeeklyUsage(usedPercent: 0, resetsAt: weekly?.resetsAt, expired: false)
        precondition(model.weeklyText == "100%")
        try render(model, name: "compact-full-quota", output: output, height: 32...32)
        model.snapshot.weekly = weekly
        model.snapshot.activeCount = 120
        model.snapshot.completedCount = 101
        model.snapshot.tasks = (0..<120).map { taskFixture($0) } + [tasks[1], tasks[2]]
        precondition(model.countText(model.workingCount) == "99+" && model.needsInput)
        try render(model, name: "large-counts", output: output, height: 32...32)
        precondition(model.cameraRightWidth <= 52, "Large counts exceeded the compact side-indicator budget")
        model.snapshot.tasks = tasks
        model.expanded = true
        let largeCountsSize = try render(model, name: "large-counts-expanded", output: output, height: standardPageHeight)
        model.snapshot.activeCount = 2
        model.snapshot.completedCount = 1
        let activeSize = try render(model, name: "active", output: output, height: standardPageHeight)
        precondition(largeCountsSize.height == activeSize.height, "Large counts wrapped the tabs instead of fitting the compact layout")
        model.metrics = cameraMetrics(width: 156, height: 28)
        try render(model, name: "active-small-camera", output: output, height: 300...460)
        model.metrics = cameraMetrics(width: 220)
        try render(model, name: "active-wide-camera", output: output, height: 350...490)
        model.metrics = NotchMetrics(screen: CGRect(x: 0, y: 0, width: 1280, height: 800))
        try render(model, name: "active-small-external", output: output, height: 300...420)
        model.metrics = cameraMetrics()
        model.showingCompleted = true
        precondition(model.visibleTasks.count == 1)
        let finishedSize = try render(model, name: "finished", output: output, height: standardPageHeight)
        precondition(finishedSize == activeSize, "Task tabs must share the same panel size")
        model.showingCompleted = false
        model.failedTask = model.snapshot.tasks[0]
        model.message = "Could not select the original terminal tab. Allow Agent Companion in System Settings → Privacy & Security → Automation, or copy the resume command."
        try render(model, name: "jump-error", output: output, height: 400...650)
        model.metrics = NotchMetrics()
        try render(model, name: "jump-error-external", output: output, height: 400...750)
        model.metrics = cameraMetrics()
        model.message = nil
        model.failedTask = nil
        model.snapshot = CodexSnapshot(loading: false)
        precondition(model.weeklyText == "—" && model.visibleTasks.isEmpty)
        try render(model, name: "empty", output: output, height: standardPageHeight)
        model.snapshot.weekly = WeeklyUsage(usedPercent: 100, resetsAt: 0, expired: true)
        precondition(model.weeklyText == "—")
        model.snapshot.error = "Could not read Codex sessions: permission denied."
        try render(model, name: "read-error", output: output, height: 330...510)
        model.snapshot = CodexSnapshot(loading: true)
        try render(model, name: "loading", output: output, height: standardPageHeight)
        verifyDisplaySizing()
        verifyResize()
        try TaskTabTests.run(output: output)
        try PageSwitchTests.run(output: output)
        try verifyBoundedContent(output: output)
        verifySnapshotRefresh()
        try verifyMorphing(output: output)
        let cameraOutput = output.appendingPathComponent("camera-morph")
        try FileManager.default.createDirectory(at: cameraOutput, withIntermediateDirectories: true)
        try verifyMorphing(output: cameraOutput, camera: true)
        verifyTerminalTargets()
        verifyNavigationLifecycle()
        verifyHoverLifecycle()
        verifyScreenEdgeHover()
        try verifyDockPreferencePersistence()
        EntryPreferenceTests.run()
        try verifyDisplayPreferences()
        try SubscriptionUsageTests.run(output: output)
        try InstanceQuotaTests.run(output: output)
        try MenuBarUsageTests.run(output: output)
        print("PASS: 21 native SwiftUI layouts; working/waiting summaries, remaining quota, errors and constant-width expansion")
    }

    private static func taskFixture(_ index: Int, state: String = "running") -> CodexTask {
        CodexTask(id: "fixture-\(index)", title: "Synthetic task \(index)", project: "workspace", cwd: nil,
                  client: "cli", state: state, updatedAt: Date().timeIntervalSince1970, transcriptPath: nil)
    }

    private static func cameraMetrics(
        screen: CGRect = CGRect(x: -1470, y: 124, width: 1470, height: 956),
        width: CGFloat = 179, height: CGFloat = 32, offset: CGFloat = 0
    ) -> NotchMetrics {
        let left = CGRect(x: screen.minX, y: screen.maxY - height,
                          width: (screen.width - width) / 2 + offset, height: height)
        let right = CGRect(x: left.maxX + width, y: left.minY,
                           width: screen.maxX - left.maxX - width, height: height)
        return NotchMetrics(screen: screen, safeTop: height, topLeft: left, topRight: right)
    }

    @MainActor private static func verifyBoundedContent(output: URL) throws {
        func scrollViews(in view: NSView) -> [NSScrollView] {
            (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap { scrollViews(in: $0) }
        }
        for camera in [false, true] {
            let screen = CGRect(x: -10000, y: -10000, width: 1280, height: 720)
            let model = CompanionModel()
            model.metrics = camera ? cameraMetrics(screen: screen, width: 156) : NotchMetrics(screen: screen)
            model.snapshot = CodexSnapshot(activeCount: 8, tasks: (0..<8).map { index in
                CodexTask(id: "1b966260-04f3-4281-9179-219ee11f60a\(index)", title: "Task \(index)",
                          project: "fixture", cwd: nil, client: "cli", state: "running",
                          updatedAt: 0, transcriptPath: nil)
            }, error: String(repeating: "The local session could not be read. ", count: 12), loading: false)
            model.message = String(repeating: "The terminal could not be opened. Check Automation permissions. ", count: 12)
            model.failedTask = model.snapshot.tasks[0]
            model.expanded = true
            let panel = NSPanel(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
            panel.isReleasedWhenClosed = false
            let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { true })
            defer { presentation.stop(); panel.close() }
            func update() {
                // Simulate a 60-point bottom Dock without changing the desktop.
                presentation.update(screen: screen, availableHeight: 660, animated: false)
                RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            }
            update()
            let surface = presentation.surface
            precondition(panel.frame.height == 652 && screen.contains(panel.frame),
                         "Long content escaped the screen or covered the bottom Dock: \(panel.frame)")
            precondition(panel.frame.width == model.compactWidth && panel.frame.maxY == screen.maxY)
            guard let scroll = scrollViews(in: surface).first, let document = scroll.documentView else {
                fatalError("Overflowing content has no scroll view")
            }
            let viewport = scroll.convert(scroll.bounds, to: surface)
            precondition(viewport.minY >= model.compactHeight && viewport.maxY < surface.bounds.maxY - 30,
                         "The summary or footer became part of the scrolling content")
            precondition(document.bounds.height > scroll.contentSize.height, "The error text is not scrollable")
            func capture(_ suffix: String) throws -> NSBitmapImageRep {
                let bitmap = surface.bitmapImageRepForCachingDisplay(in: surface.bounds)!
                surface.cacheDisplay(in: surface.bounds, to: bitmap)
                let png = bitmap.representation(using: .png, properties: [:])!
                let name = "bounded-\(camera ? "camera" : "external")-\(suffix).png"
                try png.write(to: output.appendingPathComponent(name))
                return NSBitmapImageRep(data: png)!
            }
            let before = try capture("top")
            scroll.contentView.scroll(to: CGPoint(x: 0, y: document.bounds.height - scroll.contentSize.height))
            scroll.reflectScrolledClipView(scroll.contentView)
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            let offset = scroll.contentView.bounds.minY
            precondition(offset > 100, "Could not reach the bottom of the long error")
            model.snapshot.updatedAt += 1
            update()
            precondition(scrollViews(in: surface).first === scroll && abs(scroll.contentView.bounds.minY - offset) < 1,
                         "A snapshot refresh reset the content's scroll position")
            let after = try capture("bottom")
            let scale = CGFloat(before.pixelsHigh) / surface.bounds.height
            for y in stride(from: Int((viewport.maxY + 5) * scale), to: before.pixelsHigh - 4, by: 4) {
                for x in stride(from: 8, to: before.pixelsWide - 8, by: 4) {
                    precondition(before.colorAt(x: x, y: y) == after.colorAt(x: x, y: y),
                                 "Scrolling displaced or hid the footer actions")
                }
            }
            model.snapshot.error = nil
            model.message = nil
            model.failedTask = nil
            update()
            precondition(panel.frame.height < 652, "Clearing errors left a screen-sized empty panel")
            model.expanded = false
            update()
            precondition(panel.frame.height == model.compactHeight, "Closing overflow left an invisible hit area")
        }
        print("Bounded content: screen/Dock limits, reachable errors, fixed footer, stable scroll on refresh and recovery passed")
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

    @MainActor private static func verifyDisplaySizing() {
        // Logical-point fixtures include the actual 1470-point MacBook geometry,
        // a shifted/asymmetric gap, and a different scaled-resolution setting.
        let external = CGRect(x: -10000, y: -10000, width: 1920, height: 1080)
        let builtin = CGRect(x: -11470, y: -9876, width: 1470, height: 956)
        let camera = cameraMetrics(screen: builtin)
        precondition(camera.width == 228 && camera.cameraWidth == 180 && camera.cameraHeight == 32 && camera.compactHeight == 32)
        precondition(camera.contentScale == 1 && camera.cameraSideWidth == 24)
        let small = CGRect(x: -10000, y: -10000, width: 1280, height: 800)
        precondition(NotchMetrics(screen: small).width == 180)
        let model = CompanionModel()
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { false })
        defer { presentation.stop(); panel.close() }
        let layouts: [(CGRect, NotchMetrics)] = [
            (external, NotchMetrics(screen: external)),
            (builtin, camera),
            (builtin, cameraMetrics(screen: builtin, width: 179, height: 38)),
            (builtin, cameraMetrics(screen: builtin, width: 179, height: 38, offset: 11)),
            (small, cameraMetrics(screen: small, width: 156, height: 28)),
            (external, NotchMetrics(screen: external))
        ]
        for expanded in [false, true] {
            model.expanded = expanded
            presentation.update(screen: external, animated: false)
            var previousScreen = external
            for (screen, metrics) in layouts {
                let geometryChanged = previousScreen != screen || model.metrics != metrics
                model.metrics = metrics
                presentation.update(screen: screen)
                if geometryChanged {
                    precondition(!presentation.isAnimating, "A display change animated with the previous geometry")
                }
                previousScreen = screen
                for count in [0, 9, 10, 99, 120] {
                    model.snapshot.activeCount = count
                    model.snapshot.completedCount = count
                    model.snapshot.tasks = (0..<count).map { taskFixture($0) }
                    if count > 0 { model.snapshot.tasks.append(taskFixture(count, state: "waiting")) }
                    presentation.update(screen: screen)
                    RunLoop.main.run(until: Date().addingTimeInterval(0.02))
                    precondition(panel.frame.width == model.compactWidth, "Display changes retained the old width")
                    precondition(panel.frame.midX == screen.midX + model.centerOffset && panel.frame.maxY == screen.maxY,
                                 "The notch did not follow the new display's camera position: \(panel.frame), screen \(screen), metrics \(metrics)")
                    if metrics.hasCamera {
                        let cameraLeft = screen.midX + metrics.centerOffset - metrics.cameraWidth / 2
                        precondition(panel.frame.minX + model.cameraLeftWidth == cameraLeft &&
                                     panel.frame.maxX - model.cameraRightWidth == cameraLeft + metrics.cameraWidth,
                                     "A display/count change moved the reserved camera gap")
                        precondition(model.compactHeight == metrics.cameraHeight,
                                     "The indicators dropped below the menu-bar band")
                    }
                }
                // Published counter updates can invalidate SwiftUI's measured
                // height on a later layout pass. They are content updates, not
                // display changes, and may finish a normal height animation.
                let deadline = Date().addingTimeInterval(2)
                while presentation.isAnimating && Date() < deadline {
                    RunLoop.main.run(until: Date().addingTimeInterval(0.02))
                }
                precondition(!presentation.isAnimating, "Content layout did not settle after updating counts")
                if !expanded { precondition(panel.frame.height == metrics.compactHeight) }
            }
        }
        // Disconnect while closing; the new screen must snap to its own compact
        // geometry, without leaving the old display's large invisible hit area.
        model.expanded = false
        presentation.update(screen: external)
        model.metrics = camera
        presentation.update(screen: builtin)
        precondition(panel.frame.size == CGSize(width: model.compactWidth, height: 32) && !presentation.isAnimating)
        precondition(NotchMetrics(screen: builtin, safeTop: 32).cameraHeight == 0,
                     "Missing camera metadata must fall back to a compact ordinary strip")
        print("Display sizing: disconnect/reconnect, compact/expanded, camera gap, large counts, scaled resolution and offset origins passed")
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

        let cameraPanel = CGRect(x: 830, y: 1048, width: 260, height: 32)
        for point in [CGPoint(x: 960, y: 1080), CGPoint(x: 830, y: 1080),
                      CGPoint(x: 840, y: 1064), CGPoint(x: 1080, y: 1064),
                      CGPoint(x: 822, y: 1047), CGPoint(x: 1098, y: 1042)] {
            precondition(NotchHoverRegion.contains(point, panel: cameraPanel, screen: screen, cameraHeight: 32),
                         "The camera side indicators lost their top edge or lower hover margin")
        }
        for point in [CGPoint(x: 829, y: 1080), CGPoint(x: 1091, y: 1060), CGPoint(x: 822, y: 1048)] {
            precondition(!NotchHoverRegion.contains(point, panel: cameraPanel, screen: screen, cameraHeight: 32),
                         "Hovering a menu icon beside the camera opened the panel")
        }

        var pointer = CGPoint(x: 40, y: 1080)
        var sampledPointer = pointer
        var frame = compact
        var expansions = 0
        var collapses = 0
        let hover = NotchHover(
            containsPointer: {
                sampledPointer = pointer
                return NotchHoverRegion.contains(pointer, panel: frame, screen: screen)
            },
            expand: { frame = expanded; expansions += 1 },
            collapse: { frame = compact; collapses += 1 }
        )
        func elapse(_ seconds: TimeInterval) {
            RunLoop.main.run(until: Date().addingTimeInterval(seconds))
        }
        hover.start()
        hover.start() // Starting again must not create a second sampler.
        pointer = CGPoint(x: 960, y: 1080) // Deliberately no mouse event.
        waitFor("A stationary pointer at the top edge did not expand") { expansions == 1 }
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
        waitFor("Polling did not observe leaving the dismissed strip") { sampledPointer == pointer }
        pointer = CGPoint(x: 1076, y: 1080)
        waitFor("Leaving and returning to the margin did not re-arm hover") { expansions == 2 }
        precondition(expansions == 2, "Leaving and returning to the margin did not re-arm hover")
        pointer = CGPoint(x: 40, y: 1080)
        waitFor("Polling did not dismiss after leaving the expanded panel") { collapses == 1 }
        precondition(collapses == 1, "Polling did not dismiss after leaving the expanded panel")
        hover.stop()
        pointer = CGPoint(x: 960, y: 1080)
        elapse(0.45)
        precondition(expansions == 2, "Polling continued after shutdown")
        print("Screen-edge hover: inclusive top, margins, display clipping, missing events, panel entry and dismissal passed")
    }

    private static func waitFor(_ message: String, timeout: TimeInterval = 3, until ready: () -> Bool) {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while !ready() && ProcessInfo.processInfo.systemUptime < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        }
        precondition(ready(), message)
    }

    @MainActor private static func verifyDisplayPreferences() throws {
        let domain = "com.wxgopher.agent-companion.tests.\(UUID().uuidString)"
        let store = DisplayPreferenceStore(domain: domain)
        let preferences = DisplayPreferences(store: store)
        defer {
            preferences.stop()
            CFPreferencesSetAppValue(DisplayPreferenceStore.key, nil, domain as CFString)
            CFPreferencesAppSynchronize(domain as CFString)
        }
        let first = CompanionDisplay(id: "00000000-0000-0000-0000-000000000001", name: "Studio Display")
        let second = CompanionDisplay(id: "00000000-0000-0000-0000-000000000002", name: "Studio Display")
        let connected = [first, second]
        precondition(store.read() == nil)
        precondition(preferences.snapshot(displays: connected).selectedId.isEmpty)
        precondition(CompanionDisplay.selectedIndex(preferredID: nil, identifiers: [second.id, first.id]) == 0)
        precondition(CompanionDisplay.selectedIndex(preferredID: first.id, identifiers: []) == nil)
        precondition(store.write(second))
        precondition(DisplayPreferenceStore(domain: domain).read() == second, "Display preference was not persisted")
        for ids in [[first.id, second.id], [second.id, first.id]] {
            let index = CompanionDisplay.selectedIndex(preferredID: store.read()?.id, identifiers: ids)!
            precondition(ids[index] == second.id, "Screen ordering changed the selected display")
        }
        let connectedSnapshot = preferences.snapshot(displays: connected)
        precondition(Set(connectedSnapshot.options.map(\.label)).count == 3, "Identical names are ambiguous")
        let sameSuffix = CompanionDisplay(id: "00000000-0000-0000-0000-000000010002", name: second.name)
        precondition(Set(preferences.snapshot(displays: connected + [sameSuffix]).options.map(\.label)).count == 4,
                     "Identical display names and short ID suffixes are ambiguous")
        let offline = preferences.snapshot(displays: [first])
        precondition(offline.selectedId == second.id && offline.options.last?.label.contains("Disconnected") == true)
        precondition(CompanionDisplay.selectedIndex(preferredID: second.id, identifiers: [first.id]) == 0)
        precondition(store.read() == second, "Fallback forgot the disconnected display")
        precondition(preferences.snapshot(displays: connected) == connectedSnapshot, "Reconnecting did not restore the selection")
        precondition(!preferences.select("unknown", displays: connected) && store.read() == second)

        var changes = 0
        preferences.start { changes += 1 }
        preferences.start { fatalError("Display observation started twice") }
        for (index, selection) in [first, CompanionDisplay(id: "", name: ""), second].enumerated() {
            let child = Process()
            child.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
            child.arguments = ["--write-display-preference", domain, selection.id, selection.name]
            try child.run()
            child.waitUntilExit()
            precondition(child.terminationStatus == 0)
            waitFor("The running notch did not receive the settings change") { changes == index + 1 }
            precondition((store.read()?.id ?? "") == selection.id)
        }

        // Use the connected hardware's real UUIDs, negative origins and camera
        // metadata, but keep all preferences and windows isolated from the app.
        let screens = NSScreen.screens
        let displays = screens.compactMap(CompanionDisplay.init(screen:))
        let model = CompanionModel()
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { true })
        defer { presentation.stop(); panel.close() }
        for screen in screens {
            guard let display = CompanionDisplay(screen: screen) else { continue }
            precondition(preferences.select(display.id, displays: displays))
            precondition(preferences.selectedScreen(in: screens) === screen)
            model.metrics = NotchMetrics(screen: screen)
            presentation.update(screen: screen.frame, availableHeight: screen.frame.maxY - screen.visibleFrame.minY, animated: false)
            precondition(panel.frame.maxY == screen.frame.maxY && panel.frame.midX == screen.frame.midX + model.centerOffset)
            precondition(screen.frame.contains(panel.frame) && panel.frame.height == model.compactHeight)
            if screen.safeAreaInsets.top > 0 { precondition(model.hasCamera, "The secondary screen lost its camera metadata") }
            let remaining = screens.filter { $0 !== screen }
            precondition(preferences.selectedScreen(in: remaining) === remaining.first)
            precondition(preferences.selectedScreen(in: screens) === screen)
            print("Connected display geometry: \(display.name), compact \(panel.frame.size), camera \(model.hasCamera)")
        }
        precondition(preferences.select("", displays: displays))
        precondition(store.read() == nil && preferences.selectedScreen(in: screens) === screens.first)
        let malformed = ["id": "invalid", "name": "Old display"] as CFDictionary
        CFPreferencesSetAppValue(DisplayPreferenceStore.key, malformed, domain as CFString)
        CFPreferencesAppSynchronize(domain as CFString)
        precondition(store.read() == nil, "An invalid saved identifier broke the default mode")
        print("Display preference: persistence, duplicate names, primary changes, disconnect/reconnect, notifications and connected hardware passed")
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
        precondition(!store.read(), "A fresh install must hide the Dock icon")
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

    @MainActor private static func verifyMorphing(output: URL, camera: Bool = false) throws {
        let model = CompanionModel()
        model.metrics = camera ? cameraMetrics() : NotchMetrics()
        model.snapshot = CodexSnapshot(activeCount: 2, completedCount: 1, loading: false)
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        var reducedMotion = false
        let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { reducedMotion })
        defer { presentation.stop(); panel.close() }
        // An offscreen native panel exercises actual resizing/clipping without
        // moving the user's cursor, taking focus, or touching their sessions.
        let screen = CGRect(x: -10000, y: -10000, width: 1920, height: 1080)
        presentation.update(screen: screen, animated: false)
        let compact = panel.frame
        let content = presentation.surface.subviews[0]
        let contentFrame = content.frame
        precondition(compact.height == model.compactHeight && compact.width == model.compactWidth)
        func checkAnchor() {
            precondition(abs(panel.frame.maxY - screen.maxY) < 0.01, "The notch detached from the menu bar during animation")
            precondition(panel.frame.width == compact.width && panel.frame.midX == screen.midX + model.centerOffset,
                         "Expansion occupied more menu-bar space")
            precondition(content === presentation.surface.subviews[0] && content.frame == contentFrame && content.frame.minY == 0,
                         "Expansion relaid out the content instead of revealing its stable layout")
            precondition(panel.frame.height >= compact.height)
        }
        @discardableResult func capture(_ name: String, header expected: [UInt8]? = nil) throws -> [UInt8] {
            let surface = presentation.surface
            guard let image = surface.bitmapImageRepForCachingDisplay(in: surface.bounds) else { fatalError("No animated frame") }
            surface.cacheDisplay(in: surface.bounds, to: image)
            let png = image.representation(using: .png, properties: [:])!
            try png.write(to: output.appendingPathComponent(name + ".png"))
            let pixels = NSBitmapImageRep(data: png)!
            // Compare actual rendered counter pixels, not only view frames:
            // a fade or a vertically centered SwiftUI root would otherwise pass.
            let scale = CGFloat(pixels.pixelsWide) / surface.bounds.width
            var header: [UInt8] = []
            // Include the short counters at both edges. A wide inset can
            // accidentally compare only the empty center of an external strip.
            for y in stride(from: Int(2 * scale), to: Int((model.compactHeight - 2) * scale), by: 2) {
                for x in stride(from: Int(2 * scale), to: pixels.pixelsWide - Int(2 * scale), by: 2) {
                    let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    // Compare visible ink, including its opacity. The black
                    // silhouette's alpha changes intentionally while growing.
                    header += [color.redComponent, color.greenComponent, color.blueComponent]
                        .map { UInt8((min(1, max(0, $0 * color.alphaComponent)) * 255).rounded()) }
                }
            }
            if let expected {
                let differences = zip(header, expected).enumerated().filter { abs(Int($0.element.0) - Int($0.element.1)) > 2 }
                if !differences.isEmpty {
                    let samples = differences.prefix(12).map { "\($0.offset):\($0.element.1)→\($0.element.0)" }.joined(separator: ", ")
                    print("Morph pixel differences: camera \(camera), frame \(panel.frame), content \(content.frame), bitmap \(pixels.pixelsWide)×\(pixels.pixelsHigh), scale \(scale), samples \(samples)")
                }
                precondition(differences.isEmpty, "The compact counters moved or faded in \(name)")
            }
            return header
        }
        // NSHostingView installs its window appearance and first text layers
        // asynchronously. Compare expansion against a visibly painted, idle
        // strip, as seen before a user hovers, rather than its creation frame.
        var header = try capture("morph-cold")
        var stableFrames = 0
        let paintDeadline = Date().addingTimeInterval(2)
        while stableFrames < 2 && Date() < paintDeadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
            let painted = try capture("morph-compact")
            let hasCounters = painted.contains { $0 > 32 }
            if painted == header && hasCounters {
                stableFrames += 1
            } else {
                if painted != header { print("Compact strip finished its initial paint: camera \(camera)") }
                stableFrames = 0
            }
            header = painted
        }
        precondition(stableFrames == 2, "The idle compact strip did not finish painting")
        model.expanded = true
        presentation.update(screen: screen)
        precondition(panel.frame == compact, "Opening jumped straight to the expanded window")
        var previous = compact.height
        var intermediateFrames = 0
        for index in 0..<14 {
            RunLoop.main.run(until: Date().addingTimeInterval(0.04))
            checkAnchor()
            precondition(panel.frame.height >= previous, "Opening shrank or flashed the strip")
            if presentation.isAnimating && panel.frame.height > compact.height { intermediateFrames += 1 }
            previous = panel.frame.height
            if index < 6 { try capture("morph-expand-\(index)", header: header) }
        }
        precondition(!presentation.isAnimating && intermediateFrames >= 3 && panel.frame.height > 300,
                     "Expansion did not complete through intermediate frames: height \(panel.frame.height), frames \(intermediateFrames), moving \(presentation.isAnimating)")
        let fullHeight = panel.frame.height
        model.expanded = false
        presentation.update(screen: screen)
        precondition(panel.frame.height == fullHeight, "Closing discarded the task content before shrinking")
        RunLoop.main.run(until: Date().addingTimeInterval(0.08))
        checkAnchor()
        precondition(panel.frame.height < fullHeight && panel.frame.height > compact.height)
        try capture("morph-closing", header: header)
        let interrupted = panel.frame
        model.expanded = true
        presentation.update(screen: screen)
        precondition(panel.frame == interrupted, "Re-entering during collapse snapped back to an endpoint")
        RunLoop.main.run(until: Date().addingTimeInterval(0.65))
        checkAnchor()
        precondition(!presentation.isAnimating && abs(panel.frame.height - fullHeight) < 0.01)
        model.expanded = false
        presentation.update(screen: screen)
        previous = panel.frame.height
        for _ in 0..<14 {
            RunLoop.main.run(until: Date().addingTimeInterval(0.04))
            checkAnchor()
            precondition(panel.frame.height <= previous, "Closing reversed direction")
            previous = panel.frame.height
        }
        precondition(panel.frame == compact && !presentation.isAnimating,
                     "Closing left an invisible expanded hit area")
        try capture("morph-closed", header: header)
        model.expanded = true
        reducedMotion = true
        presentation.update(screen: screen)
        precondition(panel.frame.height == fullHeight && !presentation.isAnimating, "Reduced-motion sizing did not finish immediately")
        reducedMotion = false
        model.expanded = false
        presentation.update(screen: screen)
        let movedScreen = screen.offsetBy(dx: -1920, dy: 800)
        presentation.update(screen: movedScreen, animated: false)
        precondition(panel.frame.maxY == movedScreen.maxY && panel.frame.height == compact.height && !presentation.isAnimating,
                     "A display change continued an animation on the old screen")
        print("Native morph: stable strip/top/width, continuous frames, reversible close, exact hit area, reduced motion and display changes passed")
    }

    @discardableResult @MainActor private static func render(_ model: CompanionModel, name: String, output: URL, height: ClosedRange<CGFloat>) throws -> CGSize {
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
        let png = image.representation(using: .png, properties: [:])!
        try png.write(to: output.appendingPathComponent("\(name).png"))
        if model.hasCamera {
            let pixels = NSBitmapImageRep(data: png)!
            let scale = CGFloat(pixels.pixelsWide) / size.width
            let cameraStart = Int(model.cameraLeftWidth * scale)
            let cameraEnd = Int((model.cameraLeftWidth + model.metrics.cameraWidth) * scale)
            var leftInk = 0
            var rightInk = 0
            var leftGreen = 0
            var leftOrange = 0
            var rightGreen = 0
            var rightOrange = 0
            var rightInkTop = pixels.pixelsHigh
            var rightInkBottom = 0
            for y in 2..<Int(model.metrics.cameraHeight * scale) - 2 {
                for x in 2..<pixels.pixelsWide - 2 {
                    let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    let hasInk = max(color.redComponent, color.greenComponent, color.blueComponent) > 0.2
                    let green = color.greenComponent > color.redComponent * 1.25 + 0.05
                    let orange = color.redComponent > color.greenComponent * 1.3 + 0.05 && color.greenComponent > 0.1
                    if x >= cameraStart && x < cameraEnd {
                        precondition(!hasInk, "\(name): an indicator is hidden under the physical camera")
                    } else if hasInk {
                        if x < cameraStart {
                            leftInk += 1
                            if green { leftGreen += 1 }
                            if orange { leftOrange += 1 }
                        } else {
                            rightInk += 1
                            if green { rightGreen += 1 }
                            if orange { rightOrange += 1 }
                            rightInkTop = min(rightInkTop, y)
                            rightInkBottom = max(rightInkBottom, y)
                        }
                    }
                }
            }
            // A missing-quota em dash has very little ink on a 1× CI display.
            let minimumInk = Int(3 * scale * scale)
            precondition(leftInk > minimumInk && rightInk > minimumInk,
                         "\(name): counters/quota are missing from the menu-bar row beside the camera")
            if let remaining = model.weeklyRemainingPercent {
                precondition((remaining <= 10 ? leftOrange : leftGreen) > minimumInk,
                             "\(name): the left quota is missing its status color")
            }
            if model.workingCount > 0 {
                precondition(rightGreen > minimumInk, "\(name): the right working count is not green")
            }
            precondition(model.needsInput ? rightOrange > minimumInk : rightOrange == 0,
                         "\(name): the pending question mark is missing or stale")
            precondition(CGFloat(rightInkBottom - rightInkTop) < 14 * model.metrics.cameraContentScale * scale,
                         "\(name): the working summary wrapped onto a second line")
        }
        window.close()
        print("\(name): \(size.width) × \(size.height)")
        return size
    }
}
