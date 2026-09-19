// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

// Render the actual Tasks quota cards and exercise source routing with synthetic
// instances. This suite never launches a CLI or reads an account/session file.
@MainActor enum InstanceQuotaTests {
    private final class Reader: SubscriptionReading {
        var sources: [SubscriptionSource] = []
        var completions: [(SubscriptionUsage) -> Void] = []
        func read(source: SubscriptionSource, completion: @escaping (SubscriptionUsage) -> Void) {
            sources.append(source)
            completions.append(completion)
        }
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            fatalError("Instance quota navigation lost the explicit source")
        }
        func cancel() {}
    }

    private static let primary = CodexInstance(instanceId: "codex", label: "Codex",
        codexHome: "/synthetic/quota/main", executablePath: "/synthetic/quota/main/codex",
        databasePath: "/synthetic/quota/main/sqlite",
        weekly: WeeklyUsage(usedPercent: 25, resetsAt: 4_000_000_000, expired: false))
    private static let secondary = CodexInstance(instanceId: "dodex", label: "Dodex",
        codexHome: "/synthetic/quota/second", executablePath: "/synthetic/quota/second/codex",
        databasePath: "/synthetic/quota/second/sqlite",
        weekly: WeeklyUsage(usedPercent: 80, resetsAt: 4_000_000_000, expired: false))

    static func run(output: URL) throws {
        verifyReadIsolation()
        for camera in [false, true] {
            try verifyCards(camera: camera, instances: [primary, secondary], values: ["75%", "20%"],
                            name: "dual-quota-\(camera)", navigation: true, output: output)
        }
        var missing = primary
        missing.weekly = nil
        var expired = secondary
        expired.weekly = WeeklyUsage(usedPercent: 90, resetsAt: 0, expired: true)
        try verifyCards(camera: false, instances: [missing, expired], values: ["—", "—"],
                        name: "dual-quota-unavailable", output: output)
        try verifyCards(camera: false, instances: [primary], values: ["75%"],
                        name: "single-quota", output: output)
        try verifyMenuPopover(output: output)
        print("Instance quota cards: simultaneous Codex/Dodex, missing/expired readings, exact navigation, isolated caches and removal passed")
    }

    private static func official(_ used: Int, readAt: Date = Date(), resetsAt: TimeInterval = 4_000_000_000) -> SubscriptionUsage {
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil,
            primary: nil, secondary: SubscriptionLimits.Window(usedPercent: used, windowDurationMins: 10080, resetsAt: resetsAt))
        return SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: ["codex": bucket]), readAt: readAt)
    }

    private static func verifyReadIsolation() {
        let reader = Reader()
        let model = CompanionModel(usageReader: reader)
        defer { model.stop() }
        model.snapshot = CodexSnapshot(loading: false, instances: [primary, secondary])
        model.expanded = true
        var expansions = 0
        model.expand = { expansions += 1 }
        precondition(model.weeklyText(for: primary) == "75%" && model.weeklyText(for: secondary) == "20%")
        precondition(reader.sources.isEmpty, "Showing local quota cards launched an account read")

        model.showUsage(for: "codex")
        precondition(model.showingUsage && model.selectedInstanceID == "codex" && expansions == 1)
        reader.completions[0](official(39))
        model.showTasks()
        precondition(model.weeklyText(for: primary) == "61%" && model.weeklyText(for: secondary) == "20%",
                     "The primary official quota replaced Dodex's local reading")
        model.showUsage(for: "dodex")
        precondition(model.showingUsage && model.selectedInstanceID == "dodex" && expansions == 2)
        precondition(model.subscriptionUsage.limits == nil && model.weeklyText(for: primary) == "61%",
                     "Changing the Usage source lost the primary cache or leaked it into Dodex")
        reader.completions[1](official(58))
        model.showTasks()
        precondition(model.weeklyText(for: primary) == "61%" && model.weeklyText(for: secondary) == "42%")
        precondition(reader.sources == [primary.usageSource, secondary.usageSource])

        model.showUsage(for: "codex")
        precondition(reader.sources.count == 2 && model.weeklyText == "61%", "Returning to Codex bypassed its cache")
        model.subscriptionUsage = official(99, readAt: Date().addingTimeInterval(-301))
        precondition(model.weeklyText(for: primary) == "75%", "An expired official reading hid the local reading")
        precondition(model.weeklyText(for: secondary) == "42%", "Primary expiry affected Dodex's cache")
        model.subscriptionUsage = official(95, resetsAt: 0)
        precondition(model.weeklyText(for: primary) == "—", "A reset official quota remained current")

        model.showUsage(for: "dodex")
        var replacement = secondary
        replacement.executablePath = "/synthetic/quota/replaced-runtime/codex"
        replacement.weekly = WeeklyUsage(usedPercent: 10, resetsAt: 4_000_000_000, expired: false)
        model.snapshot.instances = [primary, replacement]
        precondition(model.weeklyText(for: replacement) == "90%",
                     "A replacement runtime inherited the former runtime's account quota")
        model.snapshot.instances = [primary]
        precondition(model.selectedInstance.id == "codex" && model.weeklyText == "61%",
                     "Removing Dodex left its quota attached to the remaining instance")
        model.showTasks()
        model.showUsage(for: "dodex")
        precondition(!model.showingUsage && reader.sources.count == 2,
                     "A stale Dodex card silently opened another instance's usage")
    }

    private static func verifyCards(camera: Bool, instances: [CodexInstance], values: [String],
                                    name: String, navigation: Bool = false, output: URL) throws {
        let screen = CGRect(x: -10000, y: -10000, width: 1470, height: 956)
        let reader = Reader()
        let model = CompanionModel(usageReader: reader)
        model.snapshot = CodexSnapshot(loading: false, instances: instances)
        model.expanded = true
        if camera {
            model.metrics = NotchMetrics(screen: screen, safeTop: 32,
                topLeft: CGRect(x: screen.minX, y: screen.maxY - 32, width: 645, height: 32),
                topRight: CGRect(x: screen.minX + 825, y: screen.maxY - 32, width: 645, height: 32))
        }
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { true })
        defer { presentation.stop(); model.stop(); panel.close() }
        func update() {
            presentation.update(screen: screen, animated: false)
            RunLoop.main.run(until: Date().addingTimeInterval(0.08))
        }
        update()
        let surface = presentation.surface
        for (instance, value) in zip(instances, values) {
            precondition(model.weeklyText(for: instance) == value)
        }
        precondition(screen.contains(panel.frame), "The dual quota panel escaped the available screen")
        precondition(reader.sources.isEmpty, "Rendering Tasks initiated an account request")
        let bitmap = surface.bitmapImageRepForCachingDisplay(in: surface.bounds)!
        surface.cacheDisplay(in: surface.bounds, to: bitmap)
        try bitmap.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("\(name).png"))

        guard navigation else { return }
        var expansions = 0
        model.expand = { expansions += 1 }
        model.showUsage(for: "dodex")
        update()
        precondition(model.showingUsage && model.selectedInstanceID == "dodex" && expansions == 1)
        precondition(reader.sources == [secondary.usageSource], "The Dodex card opened or requested the wrong account")
        reader.completions[0](official(58))
        model.showTasks()
        update()
        model.showUsage(for: "codex")
        update()
        precondition(model.showingUsage && model.selectedInstanceID == "codex" && expansions == 2)
        precondition(reader.sources == [secondary.usageSource, primary.usageSource])
        reader.completions[1](official(39))
        model.showTasks()
        model.snapshot.instances = [primary]
        update()
        precondition(model.instances.count == 1 && model.weeklyText == "61%")
        let removed = surface.bitmapImageRepForCachingDisplay(in: surface.bounds)!
        surface.cacheDisplay(in: surface.bounds, to: removed)
        try removed.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("\(name)-removed.png"))
    }

    private static func verifyMenuPopover(output: URL) throws {
        let model = CompanionModel(usageReader: Reader())
        defer { model.stop() }
        var missing = secondary
        missing.weekly = nil
        model.metrics = NotchMetrics(panelWidth: 356)
        model.snapshot = CodexSnapshot(activeCount: 8, tasks: (0..<8).map { index in
            CodexTask(id: "synthetic-menu-\(index)", title: "Synthetic task \(index)", project: "fixture", cwd: nil,
                      client: "cli", state: "running", updatedAt: 0, transcriptPath: nil,
                      instanceId: index.isMultiple(of: 2) ? "codex" : "dodex",
                      instanceLabel: index.isMultiple(of: 2) ? "Codex" : "Dodex")
        }, loading: false, instances: [primary, missing])
        model.expanded = true
        let panel = NSPanel(contentRect: CGRect(x: -10000, y: -10000, width: 356, height: 560),
                            styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        defer { panel.close() }
        let host = NSHostingView(rootView:
            CompanionView(model: model, showsDetails: true, drawsBackground: false,
                          detailsHeight: 560 - model.compactHeight)
                .frame(width: 356, height: 560, alignment: .top).background(Color.black))
        panel.contentView = host
        host.frame = CGRect(x: 0, y: 0, width: 356, height: 560)
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        precondition(host.bounds.size == CGSize(width: 356, height: 560))
        func scrolls(in view: NSView) -> [NSScrollView] {
            ((view as? NSScrollView).map { [$0] } ?? []) + view.subviews.flatMap { scrolls(in: $0) }
        }
        let viewports = scrolls(in: host)
        precondition(viewports.contains {
            ($0.documentView?.bounds.height ?? 0) > $0.contentSize.height + 10
        }, "The menu fixture did not exercise bounded overflowing content")
        let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds)!
        host.cacheDisplay(in: host.bounds, to: bitmap)
        try bitmap.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("dual-quota-menu.png"))
        for scroll in viewports {
            if let document = scroll.documentView {
                scroll.contentView.scroll(to: CGPoint(x: 0, y: max(0, document.bounds.height - scroll.contentSize.height)))
                scroll.reflectScrolledClipView(scroll.contentView)
            }
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.08))
        let after = host.bitmapImageRepForCachingDisplay(in: host.bounds)!
        host.cacheDisplay(in: host.bounds, to: after)
        precondition(host.bounds.size == CGSize(width: 356, height: 560), "Scrolling resized the menu popover")
        let scale = CGFloat(bitmap.pixelsHigh) / host.bounds.height
        for y in stride(from: Int(500 * scale), to: bitmap.pixelsHigh - 6, by: 3) {
            for x in stride(from: 12, to: bitmap.pixelsWide - 12, by: 3) {
                precondition(bitmap.colorAt(x: x, y: y) == after.colorAt(x: x, y: y),
                             "Scrolling dual-instance tasks moved or obscured the menu footer")
            }
        }
        try after.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("dual-quota-menu-scrolled.png"))
    }
}
