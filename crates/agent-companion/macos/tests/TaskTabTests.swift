// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import Combine
import SwiftUI

@MainActor enum TaskTabTests {
    static func run(output: URL) throws {
        for (active, finished) in [(1, 0), (0, 1), (8, 1), (1, 8)] {
            try verify(active: active, finished: finished, output: output)
        }
        try verifyMenuTaskCounts(output: output)
        print("Task tabs: stable window/footer pixels and scroll view through repeated empty/populated/long-list switches passed")
    }

    private static func verifyMenuTaskCounts(output: URL) throws {
        for dual in [false, true] {
            let model = CompanionModel()
            model.isPresented = true
            let primary = CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/synthetic/main")
            let secondary = CodexInstance(instanceId: "dodex", label: "Dodex", codexHome: "/synthetic/second")
            model.snapshot = CodexSnapshot(loading: false, instances: dual ? [primary, secondary] : [secondary])
            // Equal titles must remain distinct. The dual fixture also uses the
            // same session ID in two instances, with their composite UI IDs.
            let tasks = (0..<3).map { index in
                let source = dual && index == 1 ? primary : secondary
                let session = dual && index < 2 ? "shared-session" : "session-\(index)"
                return CodexTask(id: "\(source.id):\(session)", title: "Same synthetic task title", project: "fixture", cwd: nil,
                    client: "desktop", state: "running", updatedAt: 0, transcriptPath: nil,
                    sessionId: session, instanceId: source.id, instanceLabel: source.label)
            }
            let panel = NSPanel(contentRect: CGRect(x: -10000, y: -10000, width: 356, height: 560),
                                styleMask: [.borderless], backing: .buffered, defer: false)
            panel.isReleasedWhenClosed = false
            let host = NSHostingView(rootView:
                CompanionPopupView(model: model, themePreferences: ThemeTests.darkPreferences))
            panel.contentView = host
            host.frame = CGRect(x: 0, y: 0, width: 356, height: 560)
            defer { model.stop(); panel.close() }
            var taskScroll: NSScrollView?
            for (stage, count) in [1, 3, 1].enumerated() {
                model.snapshot.tasks = Array(tasks.prefix(count))
                model.snapshot.activeCount = count
                RunLoop.main.run(until: Date().addingTimeInterval(0.1))
                let pixels = capture(host)
                let scrolls = scrollViews(in: host)
                precondition(scrolls.count >= 2, "The bounded menu lost its task scroll view")
                let scroll = scrolls[1]
                if let taskScroll {
                    precondition(scroll === taskScroll, "A task count update recreated the task viewport")
                }
                taskScroll = scroll
                precondition(model.workingCount == count && model.visibleTasks.count == count)
                precondition(host.bounds.size == CGSize(width: 356, height: 560))
                let viewport = scroll.convert(scroll.bounds, to: host)
                precondition(renderedTaskSymbols(pixels, viewport: viewport, scale: CGFloat(pixels.pixelsWide) / 356) == count,
                             "The bounded menu rendered a different number of rows than its Active and working counts")
                try pixels.representation(using: .png, properties: [:])!.write(to:
                    output.appendingPathComponent("menu-task-counts-\(dual ? "dual" : "single")-\(stage)-\(count).png"))
            }
        }
        print("Menu task counts: 1 → 3 → 1 visible rows, equal titles and per-instance session identities passed")
    }

    private static func renderedTaskSymbols(_ pixels: NSBitmapImageRep, viewport: CGRect, scale: CGFloat) -> Int {
        // Count blue running symbols in the actual rendered task viewport.
        // Restrict x to the icon column so labels and quota bars cannot pass a
        // model-only assertion while LazyVStack omits or clips a visible row.
        var rows = 0
        var previousRowHadInk = false
        for y in Int(viewport.minY * scale)..<Int(viewport.maxY * scale) {
            let hasInk = (Int((viewport.minX + 8) * scale)..<Int((viewport.minX + 27) * scale)).contains { x in
                let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                return color.blueComponent - color.greenComponent > 0.1
                    && color.greenComponent - color.redComponent > 0.15
            }
            if hasInk && !previousRowHadInk { rows += 1 }
            previousRowHadInk = hasInk
        }
        return rows
    }

    private static func verify(active: Int, finished: Int, output: URL) throws {
        let model = CompanionModel()
        let tasks = (0..<(active + finished)).map { index in
            CodexTask(id: "tab-fixture-\(index)", title: "Synthetic task \(index)", project: "workspace", cwd: nil,
                      client: "cli", state: index < active ? "running" : "completed", updatedAt: 0, transcriptPath: nil)
        }
        model.snapshot = CodexSnapshot(activeCount: active, completedCount: finished, tasks: tasks,
            weekly: WeeklyUsage(usedPercent: 32, resetsAt: 4_000_000_000, expired: false), loading: false)
        model.isPresented = true
        let fixture = PopupTestWindow(model: model)
        let panel = fixture.panel, host = fixture.host
        let surface = host
        defer { model.stop(); panel.close() }
        let frame = panel.frame
        let baseline = capture(surface)
        guard let scroll = scrollViews(in: surface).dropFirst().first else { fatalError("Task viewport is missing") }
        let viewport = scroll.convert(scroll.bounds, to: surface)

        for (index, completed) in [true, false, true, false].enumerated() {
            if let document = scroll.documentView, document.bounds.height > scroll.contentSize.height + 20 {
                scroll.contentView.scroll(to: CGPoint(x: 0, y: document.bounds.height - scroll.contentSize.height))
                scroll.reflectScrolledClipView(scroll.contentView)
                RunLoop.main.run(until: Date().addingTimeInterval(0.03))
                let offset = scroll.contentView.bounds.minY
                precondition(offset > 20, "The long-list fixture did not scroll")
                model.snapshot.updatedAt += 1
                RunLoop.main.run(until: Date().addingTimeInterval(0.03))
                precondition(abs(scroll.contentView.bounds.minY - offset) < 1,
                             "A same-tab refresh reset the task list's scroll position")
            }
            model.showingCompleted = completed
            for _ in 0..<8 {
                RunLoop.main.run(until: Date().addingTimeInterval(1.0 / 120))
                let pixels = capture(surface)
                precondition(panel.frame == frame,
                             "Switching task tabs resized the panel: \(frame) → \(panel.frame)")
                precondition(panel.contentView === host && scrollViews(in: surface).dropFirst().first === scroll,
                             "Switching task tabs recreated the hosting view or native scroll view")
                precondition(scroll.convert(scroll.bounds, to: surface) == viewport,
                             "Switching task tabs moved the list viewport: \(viewport) → \(scroll.convert(scroll.bounds, to: surface)), counts \(active)/\(finished)")
                precondition(abs(scroll.contentView.bounds.minY) < 1,
                             "Switching task tabs kept the previous list's scroll offset")
                verifyPixels(pixels, match: baseline, height: frame.height, headerHeight: CompanionPopupLayout.headerHeight)
                let scale = CGFloat(pixels.pixelsWide) / frame.width
                let content = viewport.insetBy(dx: 12, dy: 8)
                var ink = 0
                for y in stride(from: Int(content.minY * scale), to: Int(content.maxY * scale), by: 2) {
                    for x in stride(from: Int(content.minX * scale), to: Int(content.maxX * scale), by: 2) {
                        let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                        if max(color.redComponent, color.greenComponent, color.blueComponent) > 0.3 { ink += 1 }
                    }
                }
                precondition(ink > 12, "The task viewport flashed blank during a tab switch")
            }
            let name = "tabs-popup-\(active)-\(finished)-\(index).png"
            try capture(surface).representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent(name))
        }
    }

    private static func scrollViews(in view: NSView) -> [NSScrollView] {
        ((view as? NSScrollView).map { [$0] } ?? []) + view.subviews.flatMap { scrollViews(in: $0) }
    }

    private static func capture(_ view: NSView) -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return NSBitmapImageRep(data: bitmap.representation(using: .png, properties: [:])!)!
    }

    private static func verifyPixels(_ pixels: NSBitmapImageRep, match baseline: NSBitmapImageRep,
                                     height: CGFloat, headerHeight: CGFloat) {
        precondition(pixels.pixelsWide == baseline.pixelsWide && pixels.pixelsHigh == baseline.pixelsHigh)
        let scale = CGFloat(pixels.pixelsHigh) / height
        // Compare the anchored header and footer, excluding the changing tabs/list.
        for range in [4..<Int((headerHeight - 4) * scale), Int((height - 48) * scale)..<Int((height - 12) * scale)] {
            for y in range where y % 3 == 0 {
                for x in stride(from: 20, to: pixels.pixelsWide - 20, by: 3) {
                    let before = baseline.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    let after = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    for difference in [before.redComponent - after.redComponent,
                                       before.greenComponent - after.greenComponent,
                                       before.blueComponent - after.blueComponent] {
                        precondition(abs(difference) <= 2.0 / 255, "The header or footer flashed during a tab switch")
                    }
                }
            }
        }
    }
}
