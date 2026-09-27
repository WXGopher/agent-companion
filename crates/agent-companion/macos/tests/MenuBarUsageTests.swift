// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum MenuBarUsageTests {
    private static func instance(_ id: String, used: Int? = nil, expired: Bool = false,
                                 resetsAt: TimeInterval = 4_000_000_000) -> CodexInstance {
        CodexInstance(instanceId: id, label: id == "codex" ? "Codex" : "Dodex",
            codexHome: "/synthetic/menu/\(id)",
            weekly: used.map { WeeklyUsage(usedPercent: $0, resetsAt: resetsAt, expired: expired) })
    }
    private static func task(_ state: String, source: String? = nil) -> CodexTask {
        CodexTask(id: "\(source ?? "codex")-\(state)", title: "Synthetic task", project: "Fixture", cwd: nil,
                  client: "cli", state: state, updatedAt: 0, transcriptPath: nil, instanceId: source)
    }
    private static func value(_ instances: [CodexInstance], tasks: [CodexTask] = []) -> MenuBarUsage {
        MenuBarUsage(instances: instances, tasks: tasks, usage: { $0.weekly })
    }
    private static func failedValue(_ instances: [CodexInstance]) -> MenuBarUsage {
        MenuBarUsage(instances: instances, reading: {
            .init(usage: $0.weekly, stale: true, readAt: Date(timeIntervalSince1970: 2_000_000_000), error: "Synthetic network failure")
        })
    }
    private static func official(_ used: Int, at date: Date = Date(),
                                 resetsAt: TimeInterval = 4_000_000_000) -> SubscriptionUsage {
        let window = SubscriptionLimits.Window(usedPercent: Double(used), windowDurationMins: 10080, resetsAt: resetsAt)
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: nil, secondary: window)
        return SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: nil), readAt: date)
    }

    static func run(output: URL) throws {
        verifyIdleExpiry()
        verifyActivitySemantics()
        verifyBreathingColors()
        let primary = instance("codex", used: 43)
        let secondary = instance("dodex", used: 28)
        let dual = value([secondary, primary])
        precondition(dual.rows.map(\.id) == ["codex", "dodex"])
        precondition(dual.rows.map(\.text) == ["57%", "72%"])
        precondition(value([primary, instance("dodex")]).rows.map(\.text) == ["57%", "—"])
        for stale in [instance("dodex", used: 1, expired: true), instance("dodex", used: 1, resetsAt: 0)] {
            precondition(value([primary, stale]).rows.map(\.text) == ["57%", "99%"])
        }
        let onlyDodex = value([instance("codex"), secondary])
        precondition(onlyDodex.rows.map(\.id) == ["codex", "dodex"] && onlyDodex.rows.map(\.text) == ["—", "72%"])
        let empty = value([instance("codex"), instance("dodex")])
        precondition(empty.rows.map(\.text) == ["—", "—"] && empty.image != nil)
        precondition(empty.accessibilityDescription.contains("No tasks") && empty.accessibilityDescription.contains("unavailable"))
        precondition(value([]).rows.isEmpty && value([]).image != nil)
        let bounds = value([instance("codex", used: 100), instance("dodex", used: 0)])
        precondition(bounds.rows.map(\.text) == ["0%", "100%"], "Zero remaining is valid data")

        // Exercise native button sizing, template rendering and accessibility,
        // using only synthetic quota values, in both menu bar appearances.
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        let indicator = MenuBarActivityView(reduceMotion: { true })
        defer { indicator.stop(); NSStatusBar.system.removeStatusItem(item) }
        for (name, usage) in [("dual", dual), ("single", value([primary])),
                              ("dodex-only", onlyDodex), ("bounds", bounds), ("empty", empty),
                              ("stale-single", failedValue([instance("codex", used: 0)])),
                              ("stale", failedValue([instance("codex", used: 0), instance("dodex", used: 100)])),
                              ("running-waiting", value([primary, secondary], tasks: [task("running"), task("waiting", source: "dodex")])),
                              ("completed-idle", value([primary, secondary], tasks: [task("completed")])),
                              ("failed-paused", value([primary, secondary], tasks: [task("failed"), task("paused", source: "dodex")])),
                              ("activity-no-quota", value([instance("codex"), instance("dodex")], tasks: [task("running"), task("waiting", source: "dodex")]))] {
            usage.apply(to: item, indicator: indicator)
            let button = item.button!
            precondition(button.image?.isTemplate == true)
            precondition(button.accessibilityLabel() == usage.accessibilityDescription)
            precondition(button.toolTip == usage.accessibilityDescription)
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
            // A native cache includes the current menu-window backdrop. Merely
            // forcing Aqua on its button cannot turn that backdrop light.
            // Export the untouched native appearance, plus explicitly matched
            // light/dark composites of its template mask and real dot drawing.
            button.appearance = nil
            try capture(button).representation(using: .png, properties: [:])!.write(
                to: output.appendingPathComponent("menu-bar-\(name)-native.png"))
            for dark in [false, true] {
                let preview = preview(button, indicator: indicator, dark: dark)
                try preview.representation(using: .png, properties: [:])!.write(
                    to: output.appendingPathComponent("menu-bar-\(name)-preview\(dark ? "-dark" : "").png"))
            }
            let blueBackdrop = NSColor(srgbRed: 0.02, green: 0.46, blue: 0.63, alpha: 1)
            try preview(button, indicator: indicator, dark: true, background: blueBackdrop)
                .representation(using: .png, properties: [:])!.write(
                    to: output.appendingPathComponent("menu-bar-\(name)-preview-blue.png"))
            if !usage.rows.isEmpty {
                let image = button.image!
                precondition(image.size.height == 22 && image.size.width <= 58)
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
                if name == "running-waiting" {
                    verifyDotColors(indicator, top: .running, bottom: .waiting)
                    button.highlight(true)
                    let highlighted = button.bitmapImageRepForCachingDisplay(in: button.bounds)!
                    button.cacheDisplay(in: button.bounds, to: highlighted)
                    try highlighted.representation(using: .png, properties: [:])!.write(
                        to: output.appendingPathComponent("menu-bar-highlighted-\(appearance.rawValue).png"))
                    verifyDotColors(indicator, top: .running, bottom: .waiting)
                    button.highlight(false)
                }
            }
            if name == "completed-idle" { verifyDotColors(indicator, top: .completed, bottom: .idle) }
            if name == "failed-paused" { verifyDotColors(indicator, top: .failed, bottom: .idle) }
            precondition(indicator.hitTest(NSPoint(x: 5, y: 5)) == nil, "The color overlay intercepted the menu button")
        }
        try verifyAnimation(output: output)
        verifyUpdates()
        print("Menu bar: activity colors, missing/failed quota, native rendering, running animation, read-only task updates, reset preservation, source isolation and closed-popup lifecycle passed")
    }

    private static func verifyBreathingColors() {
        func channels(_ color: NSColor) -> [CGFloat] {
            let color = color.usingColorSpace(.sRGB)!
            return [color.redComponent, color.greenComponent, color.blueComponent, color.alphaComponent]
        }
        let gray = channels(TaskActivity.breathingGray)
        for activity in [TaskActivity.running, .completed, .waiting, .failed] {
            let bright = channels(activity.indicatorColor)
            for (time, expected) in [(0.0, bright), (1.0, gray), (2.0, bright)] {
                let actual = channels(TaskActivity.breathingColor(activity.indicatorColor, at: time))
                precondition(zip(actual, expected).allSatisfy { abs($0 - $1) < 0.001 },
                             "Breathing must reach the full bright color and the same opaque gray")
            }
            let middle = channels(TaskActivity.breathingColor(activity.indicatorColor, at: 0.5))
            precondition(abs(middle[3] - 1) < 0.001, "Breathing faded into the wallpaper")
            precondition(zip(bright.prefix(3), gray.prefix(3)).contains { abs($0 - $1) > 0.25 },
                         "Bright and gray phases need a visible color difference")
        }
    }

    private static func verifyActivitySemantics() {
        let cases: [([String], TaskActivity)] = [
            ([], .idle), (["running"], .running), (["running", "completed"], .running),
            (["running", "waiting", "failed"], .waiting), (["running", "failed"], .running),
            (["completed", "completed"], .completed), (["completed", "failed"], .failed),
            (["completed", "stopped"], .idle), (["paused"], .idle), (["future-state"], .idle)
        ]
        for (states, expected) in cases {
            let tasks = states.map { task($0) }
            precondition(TaskActivity.aggregate(tasks) == expected, "Wrong task activity for \(states)")
            let row = value([instance("codex")], tasks: tasks).rows[0]
            precondition(row.activity == expected && row.remaining == nil && row.text == "—")
            let model = CompanionModel()
            model.snapshot = CodexSnapshot(tasks: tasks, loading: false)
            precondition(model.taskActivity == row.activity, "Popup activity disagreed with the menu bar")
        }
        precondition(task("future-state").status == "Unknown" && task("future-state").symbol != "checkmark.circle")
        let tasks = [task("completed"), task("waiting", source: "dodex"), task("running", source: "disabled")]
        let dual = value([instance("dodex"), instance("codex")], tasks: tasks)
        precondition(dual.rows.map(\.activity) == [.completed, .waiting])
        let model = CompanionModel()
        model.snapshot = CodexSnapshot(tasks: tasks, loading: false, instances: [instance("codex")])
        precondition(model.taskActivity == .completed && !model.needsInput && model.workingCount == 0,
                     "Disabled instance tasks leaked into the summary")
        precondition(value([instance("codex")], tasks: tasks).rows[0].activity == .completed)
        precondition(value([instance("codex")], tasks: [task("failed")]).accessibilityDescription.contains("failed"))
    }

    private static func capture(_ view: NSView) -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return bitmap
    }

    private static func preview(_ button: NSStatusBarButton, indicator: MenuBarActivityView,
                                dark: Bool, background: NSColor? = nil) -> NSBitmapImageRep {
        let background = background ?? (dark ? NSColor(white: 0.12, alpha: 1) : NSColor.white)
        let mask = button.image!
        let ink = NSImage(size: mask.size, flipped: false) { rect in
            mask.draw(in: rect)
            (dark ? NSColor.white : NSColor.black).setFill()
            rect.fill(using: .sourceAtop)
            return true
        }
        let nativeSize = button.bounds.size
        let content = NSImage(size: nativeSize, flipped: false) { rect in
            background.setFill()
            rect.fill()
            ink.draw(in: NSRect(x: (rect.width - mask.size.width) / 2,
                                y: (rect.height - mask.size.height) / 2,
                                width: mask.size.width, height: mask.size.height))
            indicator.draw(indicator.bounds)
            return true
        }
        let canvas = NSImage(size: NSSize(width: max(120, nativeSize.width * 2), height: nativeSize.height * 2),
                             flipped: false) { rect in
            background.setFill()
            rect.fill()
            content.draw(in: NSRect(x: (rect.width - nativeSize.width * 2) / 2, y: 0,
                                    width: nativeSize.width * 2, height: rect.height))
            return true
        }
        return NSBitmapImageRep(data: canvas.tiffRepresentation!)!
    }

    private static func verifyDotColors(_ view: MenuBarActivityView, top: TaskActivity, bottom: TaskActivity) {
        let bitmap = capture(view)
        for (half, activity) in [(0, top), (1, bottom)] {
            var matches = 0
            for y in (half * bitmap.pixelsHigh / 2)..<((half + 1) * bitmap.pixelsHigh / 2) {
                for x in 0..<bitmap.pixelsWide {
                    guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB), color.alphaComponent > 0.8 else { continue }
                    // Native bitmap caching converts through the display's
                    // profile. Check the intended hue and row, rather than
                    // requiring raw sRGB bytes after that conversion.
                    let r = color.redComponent, g = color.greenComponent, b = color.blueComponent
                    let match: Bool
                    switch activity {
                    case .running: match = b > g + 0.12 && g > r + 0.12
                    case .completed: match = g > r + 0.15 && g > b + 0.07
                    case .waiting, .failed: match = r > b + 0.2 && g > b + 0.15 && r > g
                    case .idle: match = abs(r - g) < 0.06 && b > g + 0.02 && b < g + 0.16 && b < 0.8
                    }
                    if match { matches += 1 }
                }
            }
            precondition(matches > 3, "The \(half == 0 ? "Codex" : "Dodex") colored dot vanished or was template-tinted")
        }
    }

    private static func verifyAnimation(output: URL) throws {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        var now: TimeInterval = 0
        var reduced = false, visible = true
        let indicator = MenuBarActivityView(reduceMotion: { reduced }, clock: { now }, visible: { _ in visible })
        defer { indicator.stop(); NSStatusBar.system.removeStatusItem(item) }
        let running = value([instance("codex", used: 9), instance("dodex", used: 26)], tasks: [task("running")])
        running.apply(to: item, indicator: indicator)
        func settle(_ duration: TimeInterval = 0.12) { RunLoop.main.run(until: Date().addingTimeInterval(duration)) }
        func eventually(_ message: String, timeout: TimeInterval = 2, _ condition: () -> Bool) {
            let start = ProcessInfo.processInfo.systemUptime
            while !condition() && ProcessInfo.processInfo.systemUptime - start < timeout {
                RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.01))
            }
            let elapsed = ProcessInfo.processInfo.systemUptime - start
            precondition(condition(), "\(message); elapsed=\(elapsed)s, ticks=\(indicator.animationTicks), animating=\(indicator.isAnimating)")
        }
        settle()
        precondition(indicator.isAnimating)
        let image = item.button!.image
        let bright = capture(indicator).representation(using: .png, properties: [:])!
        for dark in [false, true] {
            try preview(item.button!, indicator: indicator, dark: dark)
                .representation(using: .png, properties: [:])!.write(
                    to: output.appendingPathComponent("menu-bar-running-bright-preview\(dark ? "-dark" : "").png"))
        }
        try preview(item.button!, indicator: indicator, dark: true,
                    background: NSColor(srgbRed: 0.02, green: 0.46, blue: 0.63, alpha: 1))
            .representation(using: .png, properties: [:])!.write(
                to: output.appendingPathComponent("menu-bar-running-bright-preview-blue.png"))
        now = 1
        indicator.needsDisplay = true
        let dim = capture(indicator).representation(using: .png, properties: [:])!
        precondition(bright != dim && item.button!.image === image, "Breathing changed the quota image or did not change the dot")
        for dark in [false, true] {
            try preview(item.button!, indicator: indicator, dark: dark)
                .representation(using: .png, properties: [:])!.write(
                    to: output.appendingPathComponent("menu-bar-running-dim-preview\(dark ? "-dark" : "").png"))
        }
        try preview(item.button!, indicator: indicator, dark: true,
                    background: NSColor(srgbRed: 0.02, green: 0.46, blue: 0.63, alpha: 1))
            .representation(using: .png, properties: [:])!.write(
                to: output.appendingPathComponent("menu-bar-running-dim-preview-blue.png"))
        let before = indicator.animationTicks
        let started = ProcessInfo.processInfo.systemUptime
        settle(0.35)
        // Native callbacks may delay or extend this run-loop interval on CI.
        // Require real progress, then measure the rate against actual elapsed
        // time, with one tick of allowance at the observation boundaries.
        eventually("The running dot's timer did not fire") { indicator.animationTicks > before }
        let elapsed = ProcessInfo.processInfo.systemUptime - started
        let ticks = indicator.animationTicks - before
        let maximumTicks = Int(ceil(elapsed / 0.1)) + 1
        precondition(ticks <= maximumTicks,
                     "The running dot exceeded its 10 Hz repaint budget; ticks=\(ticks), elapsed=\(elapsed)s, maximum=\(maximumTicks)")
        reduced = true
        indicator.refreshAnimation()
        precondition(!indicator.isAnimating)
        let stopped = indicator.animationTicks
        settle(0.25)
        precondition(indicator.animationTicks == stopped)
        reduced = false
        indicator.refreshAnimation()
        precondition(indicator.isAnimating)
        visible = false
        let hiddenTicks = indicator.animationTicks
        eventually("A hidden native menu window kept its animation timer") { !indicator.isAnimating }
        precondition(indicator.animationTicks == hiddenTicks,
                     "A hidden native menu window repainted; ticks before=\(hiddenTicks), after=\(indicator.animationTicks)")
        visible = true
        indicator.refreshAnimation()
        precondition(indicator.isAnimating, "A visible unchanged row did not resume breathing")
        for state in ["waiting", "completed", "failed", "paused"] {
            value([instance("codex", used: 43)], tasks: [task(state)]).apply(to: item, indicator: indicator)
            precondition(!indicator.isAnimating, "A resting \(state) indicator scheduled animation")
            let ticks = indicator.animationTicks
            settle()
            precondition(indicator.animationTicks == ticks)
        }
        running.apply(to: item, indicator: indicator)
        indicator.isHidden = true
        precondition(!indicator.isAnimating)
        indicator.isHidden = false
        precondition(indicator.isAnimating)
        indicator.stop()
        precondition(!indicator.isAnimating)
    }

    private static func verifyIdleExpiry() {
        var now = Date(timeIntervalSince1970: 2_000_000_000)
        let bridge = FixtureUsageBridge()
        let primary = instance("codex", used: 99)
        let secondary = instance("dodex", used: 99)
        let background = CompanionModel(usageBridge: bridge, clock: { now })
        let menu = CompanionModel(usageBridge: bridge, clock: { now })
        let controller = MenuBarController(model: background, menuModel: menu, themePreferences: ThemeTests.darkPreferences,
                                         settingsOverride: {}, clock: { now }, usageBridge: bridge)
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        background.stop()
        defer { controller.quit() }
        background.snapshot = CodexSnapshot(loading: false, instances: [primary, secondary])
        menu.snapshot = background.snapshot
        func tick() {
            background.refreshUsageSnapshot()
            background.snapshot.updatedAt = now.timeIntervalSince1970
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        }
        var quota = official(43, at: now, resetsAt: now.timeIntervalSince1970 + 50)
        bridge.set(quota, for: primary.usageSource)
        bridge.set(official(28, at: now), for: secondary.usageSource)
        tick()
        for advance in [49.0, 1, 250, 10000] {
            now += advance
            tick()
            precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"], "Age or reset added a star")
        }
        bridge.set(quota, for: primary.usageSource, loading: true)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "57%")
        quota.limitsError = "Synthetic offline result"
        bridge.set(quota, for: primary.usageSource)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "57%*")
        let tooltip = controller.menuBarUsage!.accessibilityDescription
        precondition(tooltip.contains("Synthetic offline result") && tooltip.contains("last successful query")
                     && tooltip.contains("reset time passed"))
        bridge.set(quota, for: primary.usageSource, loading: true)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "57%*", "Retry cleared the failure before success")
        bridge.set(official(19, at: now), for: primary.usageSource)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "81%")
        bridge.set(SubscriptionUsage(readAt: now), for: primary.usageSource)
        tick()
        precondition(controller.menuBarUsage?.rows.first?.text == "—", "A missing window retained the old value")
        precondition(bridge.events.isEmpty, "Menu rendering or reset scheduled a query")
    }

    private static func verifyUpdates() {
        let bridge = FixtureUsageBridge()
        let background = CompanionModel(usageBridge: bridge)
        let menu = CompanionModel(usageBridge: bridge)
        let controller = MenuBarController(model: background, menuModel: menu, themePreferences: ThemeTests.darkPreferences,
                                         settingsOverride: {}, menuBarReducedMotion: { false }, usageBridge: bridge)
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        background.stop()
        defer { controller.quit() }
        func settle() { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
        let primary = instance("codex", used: 99), secondary = instance("dodex", used: 99)
        background.snapshot = CodexSnapshot(loading: false, instances: [primary, secondary])
        menu.snapshot = background.snapshot
        bridge.set(official(43), for: primary.usageSource)
        bridge.set(official(28), for: secondary.usageSource)
        background.refreshUsageSnapshot()
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "72%"])
        background.snapshot.tasks = [task("running"), task("completed", source: "dodex")]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.activity) == [.running, .completed])
        precondition(controller.menuBarIsAnimating)
        let animatedImage = controller.menuBarButton?.image
        let ticks = controller.menuBarActivityView.animationTicks
        RunLoop.main.run(until: Date().addingTimeInterval(0.35))
        precondition(controller.menuBarActivityView.animationTicks > ticks && controller.menuBarButton?.image === animatedImage)
        background.snapshot.tasks = [task("running"), task("waiting"), task("completed", source: "dodex")]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.activity) == [.waiting, .completed] && !controller.menuBarIsAnimating)
        background.snapshot.tasks = [task("failed"), task("stopped", source: "dodex")]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.activity) == [.failed, .idle])
        background.snapshot.tasks = []
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.activity) == [.idle, .idle] && bridge.events.isEmpty,
                     "Task updates or animation scheduled an account request")
        menu.showUsage(for: "dodex")
        precondition(bridge.events == [.history("dodex")] && menu.subscriptionUsage.limits != nil)
        menu.refreshUsage()
        precondition(bridge.events == [.history("dodex"), .refresh("dodex")])
        menu.showTasks()
        menu.stop()
        bridge.set(official(31), for: secondary.usageSource)
        background.refreshUsageSnapshot()
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "69%"], "Closing discarded the result")
        for field in ["home", "runtime", "database"] {
            var replaced = secondary
            if field == "home" { replaced.codexHome = "/replaced" }
            if field == "runtime" { replaced.executablePath = "/replaced/codex" }
            if field == "database" { replaced.databasePath = "/replaced/db" }
            background.snapshot.instances = [primary, replaced]
            settle()
            precondition(controller.menuBarUsage?.rows.map(\.text) == ["57%", "—"], "Replacement source inherited a quota")
        }
        background.snapshot.instances = [primary]
        settle()
        precondition(controller.menuBarUsage?.rows.map(\.id) == ["codex"])
        background.snapshot.tasks = [task("running")]
        settle()
        precondition(controller.menuBarIsVisible && controller.menuBarIsAnimating)
        controller.quit()
        precondition(!controller.menuBarIsVisible && !controller.menuBarIsAnimating)
        precondition(bridge.events.count == 2, "Closing or shutting down emitted a query")
    }
}
