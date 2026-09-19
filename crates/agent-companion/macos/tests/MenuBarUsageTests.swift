// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum MenuBarUsageTests {
    private final class Reader: SubscriptionReading {
        var reads = 0
        var completion: ((SubscriptionUsage) -> Void)?
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            reads += 1
            self.completion = completion
        }
        func cancel() {}
    }
    private static func instance(_ id: String, used: Int? = nil, expired: Bool = false,
                                 resetsAt: TimeInterval = 4_000_000_000) -> CodexInstance {
        CodexInstance(instanceId: id, label: id == "codex" ? "Codex" : "Dodex",
            codexHome: "/synthetic/menu/\(id)",
            weekly: used.map { WeeklyUsage(usedPercent: $0, resetsAt: resetsAt, expired: expired) })
    }
    private static func value(_ instances: [CodexInstance]) -> MenuBarUsage {
        MenuBarUsage(instances: instances) { $0.weekly }
    }

    static func run(output: URL) throws {
        let primary = instance("codex", used: 43)
        let secondary = instance("dodex", used: 28)
        let dual = value([secondary, primary])
        precondition(dual.rows.map(\.id) == ["codex", "dodex"])
        precondition(dual.rows.map(\.text) == ["57%", "72%"])
        for unavailable in [instance("dodex"), instance("dodex", used: 1, expired: true),
                            instance("dodex", used: 1, resetsAt: 0)] {
            precondition(value([primary, unavailable]).rows.map(\.text) == ["57%"])
        }
        let onlyDodex = value([instance("codex"), secondary])
        precondition(onlyDodex.rows.map(\.id) == ["dodex"] && onlyDodex.rows[0].text == "72%")
        let empty = value([instance("codex"), instance("dodex")])
        precondition(empty.rows.isEmpty && empty.image != nil)
        precondition(empty.accessibilityDescription == "Agent Companion tasks and usage")
        let bounds = value([instance("codex", used: 100), instance("dodex", used: 0)])
        precondition(bounds.rows.map(\.text) == ["0%", "100%"], "Zero remaining is valid data")

        // Exercise native button sizing, template rendering and accessibility,
        // using only synthetic quota values, in both menu bar appearances.
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        defer { NSStatusBar.system.removeStatusItem(item) }
        for (name, usage) in [("dual", dual), ("single", value([primary])),
                              ("dodex-only", onlyDodex), ("bounds", bounds), ("empty", empty)] {
            usage.apply(to: item)
            let button = item.button!
            precondition(button.image?.isTemplate == true)
            precondition(button.accessibilityLabel() == usage.accessibilityDescription)
            precondition(button.toolTip == usage.accessibilityDescription)
            let preview = NSImage(size: NSSize(width: 120, height: 44), flipped: false) { rect in
                NSColor.white.setFill()
                rect.fill()
                if let image = usage.image {
                    image.draw(in: NSRect(x: (rect.width - image.size.width * 2) / 2, y: 0,
                                         width: image.size.width * 2, height: image.size.height * 2))
                }
                return true
            }
            let previewBitmap = NSBitmapImageRep(data: preview.tiffRepresentation!)!
            try previewBitmap.representation(using: .png, properties: [:])!.write(
                to: output.appendingPathComponent("menu-bar-\(name)-preview.png"))
            if !usage.rows.isEmpty {
                let image = button.image!
                precondition(image.size.height == 22 && image.size.width <= 40)
                let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
                var ink = [Int](repeating: 0, count: bitmap.pixelsHigh)
                for y in 0..<bitmap.pixelsHigh {
                    for x in 0..<bitmap.pixelsWide where (bitmap.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0.1 {
                        ink[y] += 1
                    }
                }
                precondition(ink.first == 0 && ink.last == 0, "Menu percentages touch the clipping boundary")
                if usage.rows.count == 2 {
                    precondition(ink.prefix(ink.count / 2).contains { $0 > 0 }
                                 && ink.suffix(ink.count / 2).contains { $0 > 0 }, "One quota row disappeared")
                }
            }
            for appearance in [NSAppearance.Name.aqua, .darkAqua] {
                button.appearance = NSAppearance(named: appearance)
                RunLoop.main.run(until: Date().addingTimeInterval(0.05))
                let bitmap = button.bitmapImageRepForCachingDisplay(in: button.bounds)!
                button.cacheDisplay(in: button.bounds, to: bitmap)
                try bitmap.representation(using: .png, properties: [:])!.write(
                    to: output.appendingPathComponent("menu-bar-\(name)-\(appearance.rawValue).png"))
            }
        }
        verifyUpdates()
        print("Menu bar: valid single/dual/fallback, zero allowance, expiry, source-isolated cached updates, disable and template rendering passed")
    }

    private static func verifyUpdates() {
        let domain = "com.wxgopher.agent-companion.tests.menu.\(UUID().uuidString)"
        let entries = EntryPreferences(store: EntryPreferenceStore(domain: domain))
        let backgroundReader = Reader(), menuReader = Reader()
        let background = CompanionModel(usageReader: backgroundReader)
        let menu = CompanionModel(usageReader: menuReader)
        let controller = NotchController(entries: entries, model: background, menuModel: menu, settingsOverride: {})
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        background.stop()
        defer {
            controller.quit()
            CFPreferencesSetAppValue(CompanionEntry.menuBar.rawValue as CFString, nil, domain as CFString)
            CFPreferencesAppSynchronize(domain as CFString)
        }
        func settle() { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
        let primary = instance("codex", used: 43), secondary = instance("dodex", used: 28)
        let snapshot = CodexSnapshot(loading: false, instances: [primary, secondary])
        background.snapshot = snapshot
        menu.snapshot = snapshot
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"],
                     "Observed menu rows: \(String(describing: controller.menuBarUsage?.rows)); snapshot instances: \(background.instances.map(\.id))")
        precondition(backgroundReader.reads == 0 && menuReader.reads == 0, "Menu display launched an account query")

        menu.showUsage(for: "dodex")
        let expiredWindow = SubscriptionLimits.Window(usedPercent: 99, windowDurationMins: 10080, resetsAt: 0)
        let expiredBucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: nil, secondary: expiredWindow)
        menuReader.completion?(SubscriptionUsage(limits: SubscriptionLimits(rateLimits: expiredBucket, rateLimitsByLimitId: nil), readAt: Date()))
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"],
                     "A recently fetched but reset quota hid the valid new-period local snapshot")
        menu.refreshUsage(force: true)
        let window = SubscriptionLimits.Window(usedPercent: 31, windowDurationMins: 10080, resetsAt: 4_000_000_000)
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: nil, secondary: window)
        menuReader.completion?(SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: nil), readAt: Date()))
        menu.showTasks()
        menu.stop()
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "69%"], "Completed popup cache did not reach the menu bar")
        var replaced = secondary
        replaced.executablePath = "/synthetic/menu/replaced/codex"
        background.snapshot.instances = [primary, replaced]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"], "Replacement runtime inherited another source's cache")
        background.snapshot.instances = [primary]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.id) == ["codex"], "Disabled Dodex remained in the menu bar")
        background.snapshot.instances = [instance("codex")]
        settle()
        precondition(controller.menuBarUsage?.rows.isEmpty == true)
        precondition(entries.set(.menuBar, visible: false))
        precondition(!controller.menuBarIsVisible)
        background.snapshot = snapshot
        settle()
        precondition(!controller.menuBarIsVisible, "Quota updates restored a hidden menu entry")
        precondition(entries.set(.menuBar, visible: true))
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "69%"])
    }
}
