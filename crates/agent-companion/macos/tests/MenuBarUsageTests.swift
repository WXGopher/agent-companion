// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum MenuBarUsageTests {
    private final class Reader: SubscriptionReading {
        var reads: Int { completions.count }
        var completions: [(SubscriptionUsage) -> Void] = []
        var completion: ((SubscriptionUsage) -> Void)? { completions.last }
        var cancelled = 0
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            completions.append(completion)
        }
        func cancel() { cancelled += 1 }
    }
    private static func instance(_ id: String, used: Int? = nil, expired: Bool = false,
                                 resetsAt: TimeInterval = 4_000_000_000) -> CodexInstance {
        CodexInstance(instanceId: id, label: id == "codex" ? "Codex" : "Dodex",
            codexHome: "/synthetic/menu/\(id)",
            weekly: used.map { WeeklyUsage(usedPercent: $0, resetsAt: resetsAt, expired: expired) })
    }
    private static func value(_ instances: [CodexInstance]) -> MenuBarUsage {
        MenuBarUsage(instances: instances, usage: { $0.weekly })
    }
    private static func official(_ used: Int, at date: Date = Date(),
                                 resetsAt: TimeInterval = 4_000_000_000) -> SubscriptionUsage {
        let window = SubscriptionLimits.Window(usedPercent: used, windowDurationMins: 10080, resetsAt: resetsAt)
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: nil, secondary: window)
        return SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: nil), readAt: date)
    }

    static func run(output: URL) throws {
        verifyIdleExpiry()
        let primary = instance("codex", used: 43)
        let secondary = instance("dodex", used: 28)
        let dual = value([secondary, primary])
        precondition(dual.rows.map(\.id) == ["codex", "dodex"])
        precondition(dual.rows.map(\.text) == ["57%", "72%"])
        precondition(value([primary, instance("dodex")]).rows.map(\.text) == ["57%"])
        for stale in [instance("dodex", used: 1, expired: true), instance("dodex", used: 1, resetsAt: 0)] {
            precondition(value([primary, stale]).rows.map(\.text) == ["57%", "99%*"])
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
                              ("dodex-only", onlyDodex), ("bounds", bounds), ("empty", empty),
                              ("stale-single", value([instance("codex", used: 0, expired: true)])),
                              ("stale", value([instance("codex", used: 0, expired: true),
                                               instance("dodex", used: 100, resetsAt: 0)]))] {
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
                precondition(image.size.height == 22 && image.size.width <= 48)
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
        print("Menu bar: idle refresh, stale/failure/reset markers, foreground sharing, source isolation, removal, hide/restore and template rendering passed")
    }

    private static func verifyIdleExpiry() {
        let domain = "com.wxgopher.agent-companion.tests.menu.idle.\(UUID().uuidString)"
        let entries = EntryPreferences(store: EntryPreferenceStore(domain: domain))
        var now = Date(timeIntervalSince1970: 2_000_000_000)
        let reader = Reader(), secondaryReader = Reader()
        let background = CompanionModel(usageReader: Reader(), clock: { now })
        let menu = CompanionModel(usageReader: reader, clock: { now })
        let controller = NotchController(entries: entries, model: background, menuModel: menu,
                                         settingsOverride: {}, clock: { now }, usageReader: {
            $0.instanceID == "codex" ? reader : secondaryReader
        })
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        background.stop()
        defer { controller.quit() }
        let snapshot = CodexSnapshot(loading: false, instances: [instance("codex"), instance("dodex", used: 28)])
        background.snapshot = snapshot
        menu.snapshot = snapshot
        menu.showUsage(for: "codex")
        reader.completion?(official(43, at: now))
        menu.expanded = false
        menu.stop()
        func tick() {
            background.snapshot.updatedAt = now.timeIntervalSince1970
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        }
        now += 299
        tick()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"],
                     "299-second account reading disappeared: \(String(describing: controller.menuBarUsage?.rows))")
        precondition(reader.reads == 1 && secondaryReader.reads == 1, "Polling bypassed the cache or duplicated an active query")
        now += 1
        tick()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%*", "72%"],
                     "Idle menu lost its last known quota at the refresh boundary: \(String(describing: controller.menuBarUsage?.rows))")
        precondition(reader.reads == 2 && !menu.expanded, "A closed popup prevented background refresh")
        reader.completion?(.failure("Synthetic offline result"))
        for _ in 0..<299 { now += 1; background.snapshot.updatedAt = now.timeIntervalSince1970 }
        tick()
        precondition(reader.reads == 2 && controller.menuBarUsage?.rows.first?.text == "57%*",
                     "A failed read renewed old data or retried before five minutes")
        precondition(controller.menuBarUsage?.accessibilityDescription.contains("awaiting update") == true)
        now += 1
        tick()
        precondition(reader.reads == 3 && secondaryReader.reads == 1, "Retry was missing or duplicated another instance's query")
        reader.completion?(official(19, at: now, resetsAt: now.timeIntervalSince1970 + 50))
        secondaryReader.completion?(official(28, at: now))
        tick()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["81%", "72%"])
        now += 50
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "81%*", "Reset quota became current or disappeared")
        background.snapshot.instances?[0].weekly = WeeklyUsage(usedPercent: 10, resetsAt: 4_000_000_000, expired: false)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "90%", "Expired account data hid a valid local reading")
        background.snapshot.instances?[0].weekly = WeeklyUsage(usedPercent: 10, resetsAt: 0, expired: true)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "90%*", "Local expiry restored an older account value")
        background.snapshot.instances?[0].weekly = WeeklyUsage(usedPercent: 10, resetsAt: 4_000_000_000, expired: false)
        menu.refreshUsage(force: true)
        reader.completion?(.failure("Synthetic second failure"))
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "90%*", "Local fallback concealed the failed refresh")
    }

    private static func verifyUpdates() {
        let domain = "com.wxgopher.agent-companion.tests.menu.\(UUID().uuidString)"
        let entries = EntryPreferences(store: EntryPreferenceStore(domain: domain))
        let backgroundReader = Reader(), menuReader = Reader()
        var readers: [SubscriptionSource: Reader] = [:]
        func reader(for source: SubscriptionSource) -> Reader {
            if let reader = readers[source] { return reader }
            let reader = Reader()
            readers[source] = reader
            return reader
        }
        let background = CompanionModel(usageReader: backgroundReader)
        let menu = CompanionModel(usageReader: menuReader)
        let controller = NotchController(entries: entries, model: background, menuModel: menu, settingsOverride: {},
                                         usageReader: { reader(for: $0) })
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
        let primaryReader = reader(for: primary.usageSource), secondaryReader = reader(for: secondary.usageSource)
        precondition(primaryReader.reads == 1 && secondaryReader.reads == 1, "The menu did not refresh each source once")
        precondition(backgroundReader.reads == 0 && menuReader.reads == 0, "A surface bypassed the shared coordinator")

        menu.showUsage(for: "dodex")
        precondition(secondaryReader.reads == 1 && menu.usageLoading, "Opening Usage duplicated its background request")
        background.expanded = true
        background.showUsage(for: "codex")
        precondition(primaryReader.reads == 1, "Opening the other surface duplicated its background request")
        secondaryReader.completion?(official(99, resetsAt: 0))
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"],
                     "A recently fetched but reset quota hid the valid new-period local snapshot")
        menu.refreshUsage(force: true)
        secondaryReader.completion?(official(31))
        menu.showTasks()
        menu.stop()
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "69%"], "Completed popup cache did not reach the menu bar")
        for field in ["home", "runtime", "database"] {
            var replaced = secondary
            if field == "home" { replaced.codexHome = "/synthetic/menu/replaced" }
            if field == "runtime" { replaced.executablePath = "/synthetic/menu/replaced/codex" }
            if field == "database" { replaced.databasePath = "/synthetic/menu/replaced/sqlite" }
            background.snapshot.instances = [primary, replaced]
            settle()
            precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"], "Replacement \(field) inherited another source's cache")
            precondition(reader(for: replaced.usageSource).reads == 1)
        }
        background.snapshot.instances = [primary]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.id) == ["codex"], "Disabled Dodex remained in the menu bar")
        background.snapshot.instances = [instance("codex")]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%*"], "A temporarily missing local reading disappeared")
        background.snapshot = snapshot
        settle()
        precondition(secondaryReader.reads == 3, "Re-enabling a removed source reused its old cache")
        secondaryReader.completions[1](official(99))
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"], "A late removed-source result was resurrected")
        let secondaryCancels = secondaryReader.cancelled, primaryCancels = primaryReader.cancelled
        precondition(entries.set(.menuBar, visible: false))
        precondition(!controller.menuBarIsVisible)
        precondition(secondaryReader.cancelled == secondaryCancels + 1, "Hiding the menu did not cancel unowned work")
        precondition(primaryReader.cancelled == primaryCancels, "Hiding the menu cancelled a foreground read")
        primaryReader.completion?(official(35))
        precondition(background.subscriptionUsage.limits != nil, "Hidden menu lost the foreground result")
        background.showTasks()
        background.showUsage(for: "codex")
        precondition(primaryReader.reads == 1 && background.subscriptionUsage.limits != nil,
                     "Reopening foreground Usage with the menu hidden discarded its fresh cache")
        background.showTasks()
        background.snapshot = snapshot
        settle()
        precondition(!controller.menuBarIsVisible, "Quota updates restored a hidden menu entry")
        precondition(secondaryReader.reads == 3 && primaryReader.reads == 1, "A hidden menu started background work")
        precondition(entries.set(.menuBar, visible: true))
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["65%", "72%"], "Restoring the menu lost a valid foreground cache")
        precondition(secondaryReader.reads == 4 && primaryReader.reads == 1)
    }
}
