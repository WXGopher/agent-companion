// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import Combine

@MainActor enum PageSwitchTests {
    private final class Reader: SubscriptionReading {
        var requests = 0
        var completion: ((SubscriptionUsage) -> Void)?
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            requests += 1
            self.completion = completion
        }
        func cancel() { completion = nil }
    }

    static func run(output: URL) throws {
        for (count, longError) in [(8, true), (0, false), (8, false)] {
            try verify(count: count, longError: longError, output: output)
        }
        print("Page switching: stable panel, header/footer pixels, persistent scroll views and offsets, loading/error/cache transitions passed")
    }

    private static func verify(count: Int, longError: Bool, output: URL) throws {
        let reader = Reader()
        let model = CompanionModel(usageReader: reader)
        model.snapshot = CodexSnapshot(completedCount: count, tasks: (0..<count).map { index in
            CodexTask(id: "page-fixture-\(index)", title: "Synthetic task \(index)", project: "workspace", cwd: nil,
                      client: "cli", state: "completed", updatedAt: 0, transcriptPath: nil)
        }, weekly: WeeklyUsage(usedPercent: 32, resetsAt: 4_000_000_000, expired: false), loading: false)
        if longError { model.snapshot.error = String(repeating: "The local session could not be read. ", count: 40) }
        model.isPresented = true
        model.showingCompleted = true
        let fixture = PopupTestWindow(model: model)
        let panel = fixture.panel, host = fixture.host
        let surface = host
        defer { model.stop(); panel.close() }
        let frame = panel.frame
        let baseline = capture(surface)
        let scrolls = scrollViews(in: surface)
        if longError, let scroll = scrolls.first, let document = scroll.documentView {
            scroll.contentView.scroll(to: CGPoint(x: 0, y: document.bounds.height - scroll.contentSize.height))
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        // First open while loading, successful response, cached round trips,
        // and an unavailable account must all leave the surrounding UI still.
        for step in 0..<12 {
            if step % 2 == 0 { model.showUsage() } else { model.showTasks() }
            for tick in 0..<12 {
                if step == 0 && tick == 4 { reader.completion?(SubscriptionUsageTests.fixture) }
                if step == 10 && tick == 4 { model.subscriptionUsage = .failure("Sign in to Codex, then refresh.") }
                RunLoop.main.run(until: Date().addingTimeInterval(1.0 / 120))
                precondition(panel.frame == frame,
                             "Switching Tasks / Usage started resizing the panel: \(frame) → \(panel.frame)")
                precondition(panel.contentView === host && scrollViews(in: surface).elementsEqual(scrolls, by: { $0 === $1 }),
                             "Switching Tasks / Usage recreated the host or a scroll view")
                let pixels = capture(surface)
                verifyAnchors(pixels, baseline: baseline, frame: frame, headerHeight: CompanionPopupLayout.headerHeight)
                verifyContent(pixels, frame: frame, headerHeight: CompanionPopupLayout.headerHeight)
                precondition(model.showingCompleted, "Returning to Tasks lost the selected filter")
            }
            if step == 0 || step == 1 {
                try capture(surface).representation(using: .png, properties: [:])!.write(to:
                    output.appendingPathComponent("pages-popup-\(count)-\(longError)-\(step).png"))
            }
        }
        precondition(reader.requests == 1, "Page switches bypassed the subscription cache")

        // Hidden pages keep their scroll state, including an outer overflow
        // viewport on a short display. No frame may be recreated to restore it.
        // Restore content so the Usage scroll view can actually move, then
        // record offsets on both pages rather than testing an empty error card.
        model.subscriptionUsage = SubscriptionUsageTests.fixture
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        for scroll in scrolls {
            if let document = scroll.documentView {
                scroll.contentView.scroll(to: CGPoint(x: 0, y: max(0, document.bounds.height - scroll.contentSize.height)))
                scroll.reflectScrolledClipView(scroll.contentView)
            }
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        let offsets = scrolls.map { $0.contentView.bounds.origin }
        let sizes = scrolls.map { "viewport \($0.contentSize), document \(String(describing: $0.documentView?.bounds.size))" }
        for usage in [true, false, true, false] {
            if usage { model.showUsage() } else { model.showTasks() }
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
            precondition(scrolls.map { $0.contentView.bounds.origin } == offsets,
                         "Page switching lost a saved scroll position: \(offsets) → \(scrolls.map { $0.contentView.bounds.origin }), sizes \(sizes) → \(scrolls.map { "viewport \($0.contentSize), document \(String(describing: $0.documentView?.bounds.size))" }), count \(count), error \(longError), usage \(usage)")
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

    private static func verifyAnchors(_ pixels: NSBitmapImageRep, baseline: NSBitmapImageRep,
                                      frame: NSRect, headerHeight: CGFloat) {
        precondition(pixels.pixelsWide == baseline.pixelsWide && pixels.pixelsHigh == baseline.pixelsHigh)
        let scale = CGFloat(pixels.pixelsHigh) / frame.height
        for range in [4..<Int((headerHeight - 4) * scale),
                      Int((frame.height - 48) * scale)..<Int((frame.height - 12) * scale)] {
            for y in range where y % 3 == 0 {
                for x in stride(from: 20, to: pixels.pixelsWide - 20, by: 3) {
                    let before = baseline.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    let after = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    for delta in [before.redComponent - after.redComponent, before.greenComponent - after.greenComponent,
                                  before.blueComponent - after.blueComponent] {
                        precondition(abs(delta) <= 2.0 / 255, "The header or footer flashed during a page switch")
                    }
                }
            }
        }
    }

    private static func verifyContent(_ pixels: NSBitmapImageRep, frame: NSRect, headerHeight: CGFloat) {
        let scale = CGFloat(pixels.pixelsHigh) / frame.height
        var ink = 0
        for y in stride(from: Int((headerHeight + 65) * scale), to: Int((frame.height - 80) * scale), by: 2) {
            for x in stride(from: 20, to: pixels.pixelsWide - 20, by: 2) {
                let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                if max(color.redComponent, color.greenComponent, color.blueComponent) > 0.3 { ink += 1 }
            }
        }
        precondition(ink > 12, "The selected page flashed blank or inherited another page's scroll offset")
    }
}
