// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

// Render the actual Tasks quota cards and exercise source routing with synthetic
// instances. This suite never launches a CLI or reads an account/session file.
@MainActor enum InstanceQuotaTests {
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
        try verifyCards(instances: [primary, secondary], values: ["75%", "20%"],
                        name: "dual-quota-popup", navigation: true, output: output)
        var missing = primary
        missing.weekly = nil
        var expired = secondary
        expired.weekly = WeeklyUsage(usedPercent: 90, resetsAt: 0, expired: true)
        try verifyCards(instances: [missing, expired], values: ["—", "10%"],
                        name: "dual-quota-unavailable", output: output)
        try verifyCards(instances: [primary], values: ["75%"],
                        name: "single-quota", output: output)
        try verifyMenuPopover(output: output)
        print("Instance quota cards: simultaneous Codex/Dodex, missing/expired readings, exact navigation, isolated caches and removal passed")
    }

    private static func official(_ used: Int, readAt: Date = Date(), resetsAt: TimeInterval = 4_000_000_000) -> SubscriptionUsage {
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil,
            primary: nil, secondary: SubscriptionLimits.Window(usedPercent: Double(used), windowDurationMins: 10080, resetsAt: resetsAt))
        return SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: ["codex": bucket]), readAt: readAt)
    }

    private static func verifyReadIsolation() {
        let bridge = FixtureUsageBridge()
        let model = CompanionModel(usageBridge: bridge)
        defer { model.stop() }
        model.snapshot = CodexSnapshot(loading: false, instances: [primary, secondary])
        model.isPresented = true
        precondition(model.weeklyText(for: primary) == "—" && model.weeklyText(for: secondary) == "—",
                     "Session logs populated GUI quota")
        bridge.set(official(39), for: primary.usageSource)
        bridge.set(official(58), for: secondary.usageSource)
        model.refreshUsageSnapshot()
        model.showUsage(for: "codex")
        model.showTasks()
        precondition(model.weeklyText(for: primary) == "61%" && model.weeklyText(for: secondary) == "42%")
        model.showUsage(for: "dodex")
        precondition(model.weeklyText == "42%" && bridge.events == [.history("codex"), .history("dodex")])
        model.showUsage(for: "codex")
        precondition(model.weeklyText == "61%")
        bridge.set(official(95, readAt: Date(timeIntervalSince1970: 1), resetsAt: 0), for: primary.usageSource)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "5%", "Reset or old readings lost their successful value")
        for field in ["home", "runtime", "database"] {
            var replacement = secondary
            if field == "home" { replacement.codexHome = "/replaced" }
            if field == "runtime" { replacement.executablePath = "/replaced/codex" }
            if field == "database" { replacement.databasePath = "/replaced/db" }
            model.snapshot.instances = [primary, replacement]
            precondition(model.weeklyText(for: replacement) == "—", "Replacement source inherited old quota")
        }
        model.snapshot.instances = [primary]
        model.showTasks()
        let before = bridge.events.count
        model.showUsage(for: "dodex")
        precondition(!model.showingUsage && bridge.events.count == before,
                     "A removed card silently selected another instance")
        precondition(bridge.count("refresh") == 0 && bridge.count("panelOpen") == 0)
    }

    private static func verifyCards(instances: [CodexInstance], values: [String],
                                    name: String, navigation: Bool = false, output: URL) throws {
        let bridge = FixtureUsageBridge()
        let model = CompanionModel(usageBridge: bridge)
        model.snapshot = CodexSnapshot(loading: false, instances: instances)
        for instance in instances {
            if let quota = instance.weekly {
                bridge.set(official(quota.usedPercent, resetsAt: quota.resetsAt ?? 4_000_000_000), for: instance.usageSource)
            }
        }
        model.refreshUsageSnapshot()
        model.isPresented = true
        let fixture = PopupTestWindow(model: model)
        let panel = fixture.panel
        defer { model.stop(); panel.close() }
        func update() { RunLoop.main.run(until: Date().addingTimeInterval(0.08)) }
        update()
        let surface = fixture.host
        for (instance, value) in zip(instances, values) {
            precondition(model.weeklyText(for: instance) == value)
        }
        precondition(panel.frame.size == CGSize(width: 356, height: 560), "Quota cards resized the popup")
        precondition(bridge.events.isEmpty, "Rendering Tasks initiated an account request")
        let bitmap = surface.bitmapImageRepForCachingDisplay(in: surface.bounds)!
        surface.cacheDisplay(in: surface.bounds, to: bitmap)
        try bitmap.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("\(name).png"))

        guard navigation else { return }
        var expansions = 0
        model.present = { expansions += 1 }
        model.showUsage(for: "dodex")
        update()
        precondition(model.showingUsage && model.selectedInstanceID == "dodex" && expansions == 1)
        precondition(bridge.events == [.history("dodex")], "The Dodex card opened or requested the wrong account")
        bridge.set(official(58), for: secondary.usageSource)
        model.refreshUsageSnapshot()
        model.showTasks()
        update()
        model.showUsage(for: "codex")
        update()
        precondition(model.showingUsage && model.selectedInstanceID == "codex" && expansions == 2)
        precondition(bridge.events == [.history("dodex"), .history("codex")])
        bridge.set(official(39), for: primary.usageSource)
        model.refreshUsageSnapshot()
        model.showTasks()
        model.snapshot.instances = [primary]
        update()
        precondition(model.instances.count == 1 && model.weeklyText == "61%")
        let removed = surface.bitmapImageRepForCachingDisplay(in: surface.bounds)!
        surface.cacheDisplay(in: surface.bounds, to: removed)
        try removed.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("\(name)-removed.png"))
    }

    private static func verifyMenuPopover(output: URL) throws {
        let bridge = FixtureUsageBridge()
        bridge.set(official(25), for: primary.usageSource)
        let model = CompanionModel(usageBridge: bridge)
        defer { model.stop() }
        var missing = secondary
        missing.weekly = nil
        model.snapshot = CodexSnapshot(activeCount: 8, tasks: (0..<8).map { index in
            CodexTask(id: "synthetic-menu-\(index)", title: "Synthetic task \(index)", project: "fixture", cwd: nil,
                      client: "cli", state: "running", updatedAt: 0, transcriptPath: nil,
                      instanceId: index.isMultiple(of: 2) ? "codex" : "dodex",
                      instanceLabel: index.isMultiple(of: 2) ? "Codex" : "Dodex")
        }, loading: false, instances: [primary, missing])
        model.refreshUsageSnapshot()
        model.isPresented = true
        let panel = NSPanel(contentRect: CGRect(x: -10000, y: -10000, width: 356, height: 560),
                            styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        defer { panel.close() }
        let host = NSHostingView(rootView:
            CompanionPopupView(model: model, themePreferences: ThemeTests.darkPreferences))
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
