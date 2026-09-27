// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import Combine

enum SubscriptionUsageTests {
    static let tokenJSON = """
    {"summary":{"lifetimeTokens":1234567890,"peakDailyTokens":45000000,"longestRunningTurnSec":540,
    "currentStreakDays":8,"longestStreakDays":14},"dailyUsageBuckets":[
    {"startDate":"2026-09-13","tokens":1200000},{"startDate":"2026-09-17","tokens":3600000},
    {"startDate":"2026-09-14","tokens":0},{"startDate":"2026-09-16","tokens":9000000},
    {"startDate":"2026-09-15","tokens":4000000},{"startDate":"2026-09-12","tokens":7000000},
    {"startDate":"2026-09-11","tokens":8000000},{"startDate":"2026-09-10","tokens":5000000}]}
    """
    static let limitJSON = """
    {"rateLimits":{"primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":4000000000}},
    "rateLimitsByLimitId":{"codex":{"planType":"pro","primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":4000000000},
    "secondary":{"usedPercent":32,"windowDurationMins":10080,"resetsAt":4000300000}}}}
    """
    static var fixture: SubscriptionUsage {
        SubscriptionUsage(tokens: try! JSONDecoder().decode(SubscriptionTokens.self, from: Data(tokenJSON.utf8)),
            limits: try! JSONDecoder().decode(SubscriptionLimits.self, from: Data(limitJSON.utf8)),
            readAt: Date(timeIntervalSince1970: 1_789_660_800))
    }

    @MainActor static func run(output: URL) throws {
        try PrimaryCodexAppTests.run()
        try parsing()
        try bridgeContract()
        try terminalIsolation()
        triggerBoundaries()
        deferredHistoryBoundaries()
        quotaPresentation()
        try layouts(output: output)
        try pinnedNavigation(output: output)
        print("Subscription usage: Rust bridge schema, trigger boundaries, independent history, retained quota/error/reset state and native layouts passed")
    }

    private static func parsing() throws {
        let data = fixture
        precondition(data.tokens?.recentDays.count == 7 && data.tokens?.recentDays.first?.startDate == "2026-09-17")
        precondition(data.tokens?.recentDays.contains { $0.tokens == 0 } == true)
        let empty = try JSONDecoder().decode(SubscriptionTokens.self, from: Data("{\"summary\":{\"lifetimeTokens\":null},\"dailyUsageBuckets\":null}".utf8))
        precondition(empty.summary.lifetimeTokens == nil && empty.recentDays.isEmpty)
        precondition(UsageNumber.short(nil) == "—" && UsageNumber.short(-1) == "—" && UsageNumber.short(0) == "0")
        precondition(UsageNumber.short(1234567890).hasSuffix("B") && UsageNumber.short(Int64.max).hasSuffix("T"))
        precondition(UsageNumber.duration(540) == "9m 0s" && UsageNumber.duration(nil) == "—")
        precondition(data.limits?.buckets.count == 1)
        let now = Date(timeIntervalSince1970: 100)
        let reset = SubscriptionLimits.Window(usedPercent: 90, windowDurationMins: 10080, resetsAt: 99)
        precondition(reset.remaining(at: now) == 10 && reset.title(fallback: "Primary") == "Weekly")
        precondition(SubscriptionLimits.Window(usedPercent: 120, windowDurationMins: 15, resetsAt: nil).remaining(at: now) == 0)
        precondition(SubscriptionLimits.Window(usedPercent: -2, windowDurationMins: nil, resetsAt: nil).remaining(at: now) == 100)
    }

    private static func bridgeContract() throws {
        let json = """
        {"intervalMinutes":7,"instances":[{"source":{"instanceId":"dodex","codexHome":"/fixture/home","executablePath":"/fixture/codex","databasePath":"/fixture/db"},
        "limits":{"value":\(limitJSON),"lastSuccessAt":1000,"completedAt":1001,"loading":true,"error":"Offline","nextQueryAt":1421,"elapsedMs":75},
        "history":{"value":\(tokenJSON),"lastSuccessAt":999,"completedAt":999,"loading":false,"error":null,"nextQueryAt":1299,"elapsedMs":32}}]}
        """
        let decoded = try JSONDecoder().decode(SubscriptionSnapshot.self, from: Data(json.utf8))
        precondition(decoded.intervalMinutes == 7 && decoded.instances[0].source.instanceID == "dodex")
        precondition(decoded.instances[0].limits.elapsedMs == 75 && decoded.instances[0].usage.limitsError == "Offline")
        precondition(decoded.instances[0].usage.tokens?.summary.lifetimeTokens == 1234567890)
        let event = try JSONSerialization.jsonObject(with: JSONEncoder().encode(SubscriptionEvent.refresh("dodex"))) as! [String: String]
        precondition(event == ["event": "refresh", "instanceId": "dodex"])
        let fractional = try JSONDecoder().decode(SubscriptionLimits.Window.self,
            from: Data("{\"usedPercent\":32.5,\"windowDurationMins\":10080}".utf8))
        precondition(fractional.remaining() == 67)
        let explicit = SubscriptionLimits.Window(usedPercent: 20, windowDurationMins: 10080, resetsAt: nil)
        let legacy = SubscriptionLimits.Window(usedPercent: 50, windowDurationMins: nil, resetsAt: nil)
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: explicit, secondary: legacy)
        let priority = SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: nil), readAt: Date())
        precondition(SubscriptionUsageCoordinator.weekly(priority)?.usage.usedPercent == 20)
        let resolved = SubscriptionSource(instanceID: "codex", codexHome: "/fixture/home", executablePath: "/runtime/codex", databasePath: "/fixture/home")
        precondition(resolved.matches(.init(codexHome: "/fixture/./home")))
        precondition(!resolved.matches(.init(codexHome: "/fixture/home", executablePath: "/different/codex")))
        precondition(!resolved.matches(.init(instanceID: "dodex", codexHome: "/fixture/home")))
        let decodedAgain = try JSONDecoder().decode(SubscriptionSnapshot.self, from: JSONEncoder().encode(decoded))
        precondition(decoded == decodedAgain)
    }

    @MainActor private static func terminalIsolation() throws {
        let id = "1b966260-04f3-4281-9179-219ee11f60a0"
        let secondary = CodexInstance(instanceId: "dodex", label: "Dodex", codexHome: "/synthetic/second",
            executablePath: "/synthetic/second/codex", databasePath: "/synthetic/second/sqlite")
        let secondTask = CodexTask(id: "dodex:\(id)", title: "Synthetic second task", project: "fixture", cwd: nil,
            client: "desktop", state: "running", updatedAt: 0, transcriptPath: nil,
            sessionId: id, instanceId: "dodex", instanceLabel: "Dodex")
        var routed: CodexInstance?
        let model = CompanionModel(openTask: { _, instance, completion in routed = instance; completion(nil) },
                                   usageBridge: FixtureUsageBridge())
        model.snapshot.instances = [CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/main"), secondary]
        model.jump(to: secondTask)
        precondition(routed?.id == "dodex", "Task navigation followed the selected quota instance")
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("companion-instance-'" + UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let executable = directory.appendingPathComponent("fake-codex")
        let script = """
        #!/usr/bin/python3
        import json, os, sys
        forbidden = [key for key in os.environ if key.startswith(('OPENAI_', 'CHATGPT_', 'ELECTRON_', 'DYLD_')) or key in ('CODEX_AUTH_TOKEN', 'CODEX_APP_SERVER_WS_URL', 'NODE_OPTIONS')]
        with open(__file__ + '.result', 'w') as output:
            json.dump({'clean': not forbidden, 'home': os.environ.get('CODEX_HOME'), 'database': os.environ.get('CODEX_SQLITE_HOME'), 'args': sys.argv[1:]}, output)
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        var commandInstance = secondary
        commandInstance.executablePath = executable.path
        commandInstance.codexHome = directory.appendingPathComponent("home '").path
        commandInstance.databasePath = directory.appendingPathComponent("sqlite '").path
        let command = TerminalJump.resumeCommand(secondTask, instance: commandInstance)!
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        process.environment = ["HOME": directory.path, "PATH": "/usr/bin:/bin", "OPENAI_API_KEY": "synthetic-only",
            "CODEX_HOME": "/wrong", "CODEX_APP_SERVER_WS_URL": "synthetic-only", "NODE_OPTIONS": "synthetic-only"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run(); process.waitUntilExit()
        precondition(process.terminationStatus == 0)
        let result = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: executable.path + ".result"))) as! [String: Any]
        precondition(result["clean"] as? Bool == true)
        precondition(result["home"] as? String == commandInstance.codexHome && result["database"] as? String == commandInstance.databasePath)
        let arguments = result["args"] as! [String]
        precondition(arguments.suffix(2) == ["resume", id] && arguments.contains("cli_auth_credentials_store=\"file\""))
        precondition(arguments.contains { $0.hasPrefix("sqlite_home=") })
    }

    @MainActor private static func triggerBoundaries() {
        let bridge = FixtureUsageBridge()
        let model = CompanionModel(usageBridge: bridge)
        let primary = CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/fixture/main")
        let secondary = CodexInstance(instanceId: "dodex", label: "Dodex", codexHome: "/fixture/second")
        model.snapshot = CodexSnapshot(loading: false, instances: [primary, secondary])
        bridge.set(SubscriptionUsage(), for: primary.usageSource)
        bridge.set(SubscriptionUsage(), for: secondary.usageSource)
        model.refreshUsageSnapshot()
        model.isPresented = true
        model.showUsage()
        model.showUsage()
        precondition(bridge.events == [.history("codex")], "Usage entry queried quota or duplicated history")
        for _ in 0..<10 { model.refreshUsageSnapshot() }
        precondition(bridge.events == [.history("codex")], "Snapshot polling loaded history or quota")
        model.selectInstance("dodex")
        precondition(bridge.events.last == .history("dodex"))
        model.refreshUsage()
        precondition(bridge.events.last == .refresh("dodex") && bridge.count("history") == 2,
                     "The refresh button refreshed history or selected the wrong quota")
        model.showTasks()
        model.selectInstance("codex")
        model.isPresented = false
        model.stop()
        precondition(bridge.events.count == 3, "Leaving Usage, selection on Tasks or closing emitted a query")
        bridge.set(fixture, for: secondary.usageSource)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText(for: secondary) == "68%", "A closed panel discarded the background result")
        model.isPresented = true
        model.showUsage()
        precondition(bridge.events.last == .history("codex"))
        model.usageCoordinator.panelOpened()
        precondition(bridge.events.last == .panelOpen)
    }

    @MainActor private static func deferredHistoryBoundaries() {
        let bridge = FixtureUsageBridge()
        let model = CompanionModel(usageBridge: bridge)
        let initial = CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/startup")
        model.snapshot.codexHome = initial.codexHome
        model.isPresented = true
        model.showUsage()
        for _ in 0..<3 { model.refreshUsageSnapshot() }
        precondition(bridge.events.isEmpty, "A source-free startup dispatched history before registration")
        model.snapshot.instances = [initial]
        bridge.set(SubscriptionUsage(), for: initial.usageSource)
        model.refreshUsageSnapshot()
        precondition(bridge.events == [.history("codex")], "Startup lost the Usage-entry intent")
        for _ in 0..<5 { model.refreshUsageSnapshot() }
        precondition(bridge.events.count == 1, "Snapshot polling replayed a fulfilled history intent")

        var replacement = initial
        replacement.executablePath = "/replacement/codex"
        model.snapshot.instances = [replacement]
        model.refreshUsageSnapshot()
        precondition(bridge.events.count == 1 && model.subscriptionUsage.tokens == nil)
        bridge.value.instances.removeAll()
        bridge.set(SubscriptionUsage(), for: replacement.usageSource)
        model.refreshUsageSnapshot()
        precondition(bridge.events == [.history("codex"), .history("codex")],
                     "Replacing a source while Usage was open did not load its independent history")
        for _ in 0..<5 { model.refreshUsageSnapshot() }
        precondition(bridge.events.count == 2, "Source replacement caused history polling")

        var third = replacement
        third.databasePath = "/replacement/db"
        model.snapshot.instances = [third]
        model.refreshUsageSnapshot()
        model.showTasks()
        bridge.set(SubscriptionUsage(), for: third.usageSource)
        model.refreshUsageSnapshot()
        precondition(bridge.events.count == 2, "Leaving Usage kept a deferred history intent")
        model.showUsage()
        precondition(bridge.events.count == 3)

        var fourth = third
        fourth.codexHome = "/replacement/home"
        model.snapshot.instances = [fourth]
        model.refreshUsageSnapshot()
        model.isPresented = false
        bridge.set(SubscriptionUsage(), for: fourth.usageSource)
        model.refreshUsageSnapshot()
        precondition(bridge.events.count == 3, "Closing the panel kept a deferred history intent")
        model.isPresented = true
        model.showUsage()
        precondition(bridge.events.count == 4)
        bridge.value.instances.removeAll()
        model.refreshUsageSnapshot()
        bridge.set(SubscriptionUsage(), for: fourth.usageSource)
        model.refreshUsageSnapshot()
        precondition(bridge.events.count == 5, "Re-registering the active source lost its new history boundary")
        precondition(bridge.count("refresh") == 0 && bridge.count("panelOpen") == 0,
                     "History source registration triggered quota")
    }

    @MainActor private static func quotaPresentation() {
        var now = Date(timeIntervalSince1970: 1000)
        let bridge = FixtureUsageBridge()
        let model = CompanionModel(usageBridge: bridge, clock: { now })
        model.snapshot.weekly = WeeklyUsage(usedPercent: 5, resetsAt: 9999, expired: false)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "—", "Session-log quota leaked into the GUI")
        bridge.set(.failure("Offline"))
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "—" && model.subscriptionUsage.limitsError == "Offline")
        var success = FixtureUsageBridge.quota(32, readAt: now, resetsAt: 1200)
        bridge.set(success)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "68%")
        now += 10000
        precondition(model.weeklyText == "68%", "Age or reset changed the last successful value")
        bridge.set(success, loading: true)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "68%" && model.usageLoading)
        success.limitsError = "Network unavailable"
        bridge.set(success)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "68%*" && model.subscriptionUsage.readAt?.timeIntervalSince1970 == 1000)
        bridge.set(success, loading: true)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "68%*", "Retrying cleared the last failure before success")
        success.limitsError = nil
        bridge.set(success)
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "68%")
        let emptyBucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: nil, secondary: nil)
        bridge.set(SubscriptionUsage(limits: SubscriptionLimits(rateLimits: emptyBucket, rateLimitsByLimitId: nil), readAt: now))
        model.refreshUsageSnapshot()
        precondition(model.weeklyText == "—" && model.subscriptionUsage.limitsError == nil,
                     "A successful response without a weekly window preserved an old quota")
        precondition(bridge.events.isEmpty, "Displaying old, failed or reset readings requested quota")
    }

    @MainActor private static func layouts(output: URL) throws {
        let model = CompanionModel(usageBridge: FixtureUsageBridge())
        model.snapshot = CodexSnapshot(weekly: WeeklyUsage(usedPercent: 32, resetsAt: 4_000_000_000, expired: false), loading: false)
        model.isPresented = true
        model.showingUsage = true
        let popup = PopupTestWindow(model: model)
        let panel = popup.panel, view = popup.host
        defer { model.stop(); panel.close() }
        let expected = panel.frame
        for state in ["loading", "ready", "signed-out", "unsupported", "dual", "history-loading", "quota-failure", "reset"] {
            model.subscriptionUsage = fixture
            model.usageLoading = state == "loading"
            if state == "loading" { model.subscriptionUsage = SubscriptionUsage() }
            if state == "signed-out" { model.subscriptionUsage = .failure("Sign in to Codex with your ChatGPT subscription, then refresh.") }
            if state == "unsupported" {
                model.subscriptionUsage.tokens = nil
                model.subscriptionUsage.tokenError = "Update Codex CLI to read token activity."
            }
            if state == "history-loading" {
                model.subscriptionUsage.tokens = nil
                model.subscriptionUsage.tokensLoading = true
            }
            if state == "quota-failure" { model.subscriptionUsage.limitsError = "The quota query failed. Check your connection, then refresh." }
            if state == "reset" { model.subscriptionUsage = FixtureUsageBridge.quota(32, resetsAt: 0) }
            if state == "dual" {
                model.snapshot.instances = [
                    CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/synthetic/main"),
                    CodexInstance(instanceId: "dodex", label: "Dodex", codexHome: "/synthetic/second")
                ]
                model.selectedInstanceID = "dodex"
            }
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            precondition(panel.frame == expected && view.bounds.size == CGSize(width: 356, height: 560),
                         "A Usage state resized the popup")
            let image = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
            view.cacheDisplay(in: view.bounds, to: image)
            try image.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("usage-popup-\(state).png"))
            if state == "ready" {
                let usageHeight = (330 * CompanionPopupLayout.contentScale).rounded()
                guard let scroll = scrollViews(in: view).first(where: { abs($0.frame.height - usageHeight) < 1 }),
                      let document = scroll.documentView else { fatalError("No usage scroll view") }
                precondition(abs(scroll.contentView.bounds.minY) < 1, "The first account response scrolled past subscription limits")
                precondition(document.bounds.height > scroll.contentSize.height, "Remaining usage fields must be reachable by scrolling")
                scroll.contentView.scroll(to: CGPoint(x: 0, y: document.bounds.height - scroll.contentSize.height))
                scroll.reflectScrolledClipView(scroll.contentView)
                RunLoop.main.run(until: Date().addingTimeInterval(0.05))
                let offset = scroll.contentView.bounds.minY
                model.subscriptionUsage.readAt = model.subscriptionUsage.readAt?.addingTimeInterval(1)
                RunLoop.main.run(until: Date().addingTimeInterval(0.05))
                precondition(scrollViews(in: view).contains(where: { $0 === scroll }) && abs(scroll.contentView.bounds.minY - offset) < 1,
                             "A usage refresh reset the scroll position or replaced the scroll view")
                view.cacheDisplay(in: view.bounds, to: image)
                try image.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("usage-popup-bottom.png"))
            }
        }
        model.showTasks()
        precondition(!model.showingUsage && panel.frame == expected)
    }

    private static func scrollViews(in view: NSView) -> [NSScrollView] {
        ((view as? NSScrollView).map { [$0] } ?? []) + view.subviews.flatMap { scrollViews(in: $0) }
    }

    @MainActor private static func pinnedNavigation(output: URL) throws {
        let bridge = FixtureUsageBridge()
        let model = CompanionModel(usageBridge: bridge)
        model.isPresented = true
        model.showingCompleted = true
        model.showUsage()
        bridge.set(fixture)
        model.refreshUsageSnapshot()
        let popup = PopupTestWindow(model: model)
        let surface = popup.host
        defer { model.stop(); popup.panel.close() }
        let scrolls = scrollViews(in: surface)
        precondition(scrolls.count >= 4, "The popup fixture did not exercise both page viewports")
        let top = scrolls[0].convert(scrolls[0].bounds, to: surface).minY
        precondition(top > CompanionPopupLayout.headerHeight + 30, "Page navigation scrolled with the body")
        func capture() -> NSBitmapImageRep {
            let bitmap = surface.bitmapImageRepForCachingDisplay(in: surface.bounds)!
            surface.cacheDisplay(in: surface.bounds, to: bitmap)
            return bitmap
        }
        let before = capture()
        for scroll in scrolls {
            if let document = scroll.documentView {
                scroll.contentView.scroll(to: CGPoint(x: 0, y: max(0, document.bounds.height - scroll.contentSize.height)))
                scroll.reflectScrolledClipView(scroll.contentView)
            }
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        let after = capture()
        let scale = CGFloat(before.pixelsHigh) / surface.bounds.height
        for y in stride(from: Int((CompanionPopupLayout.headerHeight + 4) * scale), to: Int((top - 4) * scale), by: 2) {
            for x in stride(from: 10, to: before.pixelsWide - 10, by: 2) {
                precondition(before.colorAt(x: x, y: y) == after.colorAt(x: x, y: y),
                             "Scrolling moved or obscured Tasks / Usage navigation")
            }
        }
        try after.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("usage-pinned-navigation-popup.png"))
        for _ in 0..<10 {
            model.showTasks()
            precondition(!model.showingUsage && model.showingCompleted, "Returning lost the previous task filter")
            model.showUsage()
        }
        precondition(bridge.count("refresh") == 0 && bridge.count("panelOpen") == 0, "Page navigation queried quota")
    }
}
