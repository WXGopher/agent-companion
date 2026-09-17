// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import Combine

@MainActor enum TaskTabTests {
    static func run(output: URL) throws {
        for camera in [false, true] {
            for (active, finished) in [(1, 0), (0, 1), (8, 1), (1, 8)] {
                try verify(camera: camera, active: active, finished: finished, output: output)
            }
        }
        print("Task tabs: stable window/footer pixels and scroll view through repeated empty/populated/long-list switches passed")
    }

    private static func verify(camera: Bool, active: Int, finished: Int, output: URL) throws {
        let screen = CGRect(x: -10000, y: -10000, width: 1470, height: 956)
        let model = CompanionModel()
        if camera {
            model.metrics = NotchMetrics(screen: screen, safeTop: 32,
                topLeft: CGRect(x: screen.minX, y: screen.maxY - 32, width: 645, height: 32),
                topRight: CGRect(x: screen.minX + 825, y: screen.maxY - 32, width: 645, height: 32))
        }
        let tasks = (0..<(active + finished)).map { index in
            CodexTask(id: "tab-fixture-\(index)", title: "Synthetic task \(index)", project: "workspace", cwd: nil,
                      client: "cli", state: index < active ? "running" : "completed", updatedAt: 0, transcriptPath: nil)
        }
        model.snapshot = CodexSnapshot(activeCount: active, completedCount: finished, tasks: tasks,
            weekly: WeeklyUsage(usedPercent: 32, resetsAt: 4_000_000_000, expired: false), loading: false)
        model.expanded = true
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { false })
        // Follow the production controller's deferred @Published layout path.
        let subscription = model.objectWillChange.sink {
            DispatchQueue.main.async { presentation.update(screen: screen) }
        }
        defer { subscription.cancel(); presentation.stop(); panel.close() }
        presentation.update(screen: screen, animated: false)
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        let frame = panel.frame
        let surface = presentation.surface
        let host = surface.subviews[0]
        let baseline = capture(surface)
        guard let scroll = scrollViews(in: surface).first else { fatalError("Task viewport is missing") }
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
                precondition(panel.frame == frame && !presentation.isAnimating,
                             "Switching task tabs resized the panel: \(frame) → \(panel.frame)")
                precondition(surface.subviews[0] === host && scrollViews(in: surface).first === scroll,
                             "Switching task tabs recreated the hosting view or native scroll view")
                precondition(scroll.convert(scroll.bounds, to: surface) == viewport,
                             "Switching task tabs moved the list viewport: \(viewport) → \(scroll.convert(scroll.bounds, to: surface)), camera \(camera), counts \(active)/\(finished)")
                precondition(abs(scroll.contentView.bounds.minY) < 1,
                             "Switching task tabs kept the previous list's scroll offset")
                verifyPixels(pixels, match: baseline, height: frame.height, compactHeight: model.compactHeight)
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
            let name = "tabs-\(camera ? "camera" : "external")-\(active)-\(finished)-\(index).png"
            try capture(surface).representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent(name))
        }
    }

    private static func scrollViews(in view: NSView) -> [NSScrollView] {
        (view as? NSScrollView).map { [$0] } ?? view.subviews.flatMap { scrollViews(in: $0) }
    }

    private static func capture(_ view: NSView) -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return NSBitmapImageRep(data: bitmap.representation(using: .png, properties: [:])!)!
    }

    private static func verifyPixels(_ pixels: NSBitmapImageRep, match baseline: NSBitmapImageRep,
                                     height: CGFloat, compactHeight: CGFloat) {
        precondition(pixels.pixelsWide == baseline.pixelsWide && pixels.pixelsHigh == baseline.pixelsHigh)
        let scale = CGFloat(pixels.pixelsHigh) / height
        // Compare the anchored summary and footer, excluding the changing tabs/list.
        for range in [4..<Int((compactHeight - 4) * scale), Int((height - 48) * scale)..<Int((height - 12) * scale)] {
            for y in range where y % 3 == 0 {
                for x in stride(from: 20, to: pixels.pixelsWide - 20, by: 3) {
                    let before = baseline.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    let after = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    for difference in [before.redComponent - after.redComponent,
                                       before.greenComponent - after.greenComponent,
                                       before.blueComponent - after.blueComponent] {
                        precondition(abs(difference) <= 2.0 / 255, "The summary or footer flashed during a tab switch")
                    }
                }
            }
        }
    }
}
