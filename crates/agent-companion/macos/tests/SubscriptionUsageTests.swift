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

    // Spawn this same test executable as a fake CLI. It only sees a temporary
    // CODEX_HOME and never reads the user's auth files or contacts the network.
    static func serveFixture() throws {
        let directory = URL(fileURLWithPath: ProcessInfo.processInfo.environment["CODEX_HOME"]!)
        let mode = try String(contentsOf: directory.appendingPathComponent("fixture-mode"), encoding: .utf8)
        try String(getpid()).write(to: directory.appendingPathComponent("child-pid"), atomically: true, encoding: .utf8)
        if mode == "early-exit" { return }
        func send(_ id: Int, _ result: String) {
            let value = try! JSONSerialization.jsonObject(with: Data(result.utf8))
            var bytes = try! JSONSerialization.data(withJSONObject: ["id": id, "result": value])
            bytes.append(10)
            let middle = bytes.count / 2
            FileHandle.standardOutput.write(bytes.prefix(middle))
            usleep(1000)
            FileHandle.standardOutput.write(bytes.suffix(from: middle))
        }
        while let line = readLine() {
            let value = try JSONSerialization.jsonObject(with: Data(line.utf8)) as! [String: Any]
            let method = value["method"] as! String
            precondition(["initialize", "initialized", "account/read", "account/rateLimits/read", "account/usage/read"].contains(method),
                         "The subscription reader attempted a non-read-only method")
            guard let id = value["id"] as? Int else { continue }
            switch method {
            case "initialize": send(id, "{\"userAgent\":\"fixture\"}")
            case "account/read":
                precondition((value["params"] as? [String: Any])?["refreshToken"] as? Bool == false)
                send(id, mode == "signed-out" ? "{\"account\":null}" :
                    "{\"account\":{\"type\":\"\(mode == "api-key" ? "apiKey" : "chatgpt")\"}}")
            case "account/rateLimits/read":
                precondition(mode != "api-key" && mode != "signed-out")
                if mode != "success" { send(id, limitJSON) }
            case "account/usage/read":
                if mode == "hang" { while true { usleep(100000) } }
                if mode == "oversized" {
                    FileHandle.standardOutput.write(Data(repeating: 32, count: 9 * 1024 * 1024))
                } else if mode == "unsupported" {
                    print("{\"id\":\(id),\"error\":{\"code\":-32601,\"message\":\"private-server-detail\"}}")
                    fflush(stdout)
                } else {
                    send(id, mode == "malformed" ? "{\"unexpected\":true}" : tokenJSON)
                    // Deliberately deliver token activity before rate limits.
                    if mode == "success" { send(3, limitJSON) }
                }
            default: break
            }
        }
    }

    @MainActor static func run(output: URL) throws {
        try parsing()
        monitorLifecycle()
        try readerLifecycle()
        try layouts(output: output)
        try pinnedNavigation(output: output)
        print("Subscription usage: nullable metrics, quota windows, read-only RPC, framing, errors, timeout/cancellation, refresh lifecycle and layouts passed")
    }

    // Explicit opt-in only; CI and the default native suite stay offline.
    @MainActor static func liveRead() {
        let reader = CodexSubscriptionReader()
        var result: SubscriptionUsage?
        reader.read(codexHome: ProcessInfo.processInfo.environment["CODEX_HOME"] ?? "") { result = $0 }
        spin(until: { result != nil }, timeout: 25)
        guard let result, result.error == nil, result.tokens != nil, result.limits != nil else {
            fatalError("Live subscription read failed: \(result?.error ?? result?.tokenError ?? result?.limitsError ?? "timeout")")
        }
        print("Live subscription read passed: account token summary and \(result.tokens?.dailyUsageBuckets?.count ?? 0) daily buckets; \(result.limits?.buckets.count ?? 0) quota buckets. No credentials or account identifiers logged.")
    }

    private static func parsing() throws {
        let data = fixture
        precondition(data.tokens?.recentDays.count == 7 && data.tokens?.recentDays.first?.startDate == "2026-09-17")
        precondition(data.tokens?.recentDays.contains { $0.tokens == 0 } == true, "Real zero usage was discarded")
        let empty = try JSONDecoder().decode(SubscriptionTokens.self, from: Data("{\"summary\":{\"lifetimeTokens\":null},\"dailyUsageBuckets\":null}".utf8))
        precondition(empty.summary.lifetimeTokens == nil && empty.recentDays.isEmpty)
        precondition(UsageNumber.short(nil) == "—" && UsageNumber.short(-1) == "—" && UsageNumber.short(0) == "0")
        precondition(UsageNumber.short(1234567890).hasSuffix("B") && UsageNumber.short(Int64.max).hasSuffix("T"))
        precondition(UsageNumber.duration(540) == "9m 0s" && UsageNumber.duration(nil) == "—")
        precondition(data.limits?.buckets.count == 1, "The legacy quota and modern bucket were duplicated")
        let now = Date(timeIntervalSince1970: 100)
        let expired = SubscriptionLimits.Window(usedPercent: 90, windowDurationMins: 10080, resetsAt: 99)
        precondition(expired.remaining(at: now) == nil && expired.title(fallback: "Primary") == "Weekly")
        let clamped = SubscriptionLimits.Window(usedPercent: 120, windowDurationMins: 15, resetsAt: nil)
        precondition(clamped.remaining(at: now) == 0 && clamped.title(fallback: "Primary") == "15-minute")
        precondition(SubscriptionLimits.Window(usedPercent: -2, windowDurationMins: nil, resetsAt: nil).remaining(at: now) == 100)
    }

    private final class FakeReader: SubscriptionReading {
        var completions: [(SubscriptionUsage) -> Void] = []
        var homes: [String] = []
        var cancelled = 0
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            homes.append(codexHome); completions.append(completion)
        }
        func cancel() { cancelled += 1 }
    }

    @MainActor private static func monitorLifecycle() {
        let reader = FakeReader()
        var now = Date(timeIntervalSince1970: 1000)
        let monitor = SubscriptionMonitor(reader: reader, clock: { now })
        var latest: SubscriptionUsage?
        var loading = false
        monitor.onChange = { value, busy in if let value { latest = value }; loading = busy }
        monitor.refresh(codexHome: "/first")
        monitor.refresh(codexHome: "/first", force: true)
        precondition(reader.homes.count == 1 && loading, "Duplicate refresh launched another CLI")
        reader.completions[0](fixture)
        precondition(!loading && latest?.tokens != nil)
        monitor.refresh(codexHome: "/first")
        precondition(reader.homes.count == 1)
        for _ in 0..<10 {
            monitor.stop()
            monitor.refresh(codexHome: "/first")
        }
        precondition(reader.homes.count == 1, "Switching pages bypassed the five-minute cache")
        monitor.refresh(codexHome: "/first", force: true)
        precondition(reader.homes.count == 2, "Explicit refresh did not bypass the cache")
        reader.completions[1](fixture)
        now += 299
        monitor.stop()
        monitor.refresh(codexHome: "/first")
        precondition(reader.homes.count == 2 && !loading && latest?.tokens != nil,
                     "Opening Usage before five minutes discarded or refreshed its cache")
        now += 1
        monitor.stop()
        monitor.refresh(codexHome: "/first")
        precondition(reader.homes.count == 3 && loading, "Opening Usage at five minutes did not refresh")
        monitor.refresh(codexHome: "/first")
        precondition(reader.homes.count == 3, "An expired cache launched duplicate requests")
        reader.completions[2](.failure("offline"))
        precondition(latest?.tokens == nil && latest?.error == "offline", "Failure kept a possibly different account's data")
        monitor.refresh(codexHome: "/second")
        precondition(reader.homes.count == 4 && latest?.error == nil)
        monitor.stop()
        reader.completions[3](fixture)
        precondition(latest?.tokens == nil, "An old request updated the stopped monitor")
        monitor.refresh(codexHome: "/second")
        precondition(reader.homes.count == 5)

        let model = CompanionModel(usageReader: reader)
        model.snapshot.codexHome = "/model"
        model.expanded = true
        model.showUsage()
        let request = reader.completions.last!
        model.expanded = false
        request(fixture)
        precondition(!model.usageLoading && model.subscriptionUsage.tokens == nil,
                     "Closing the notch left a loading state or accepted a cancelled result")
        model.expanded = true
        model.showUsage()
        reader.completions.last!(fixture)
        let completedReads = reader.homes.count
        model.showTasks()
        model.expanded = false
        model.expanded = true
        model.showUsage()
        precondition(reader.homes.count == completedReads && !model.usageLoading && model.subscriptionUsage.tokens != nil,
                     "Returning to Usage after collapsing the notch bypassed the cache")
        model.snapshot.weekly = WeeklyUsage(usedPercent: 90, resetsAt: nil, expired: false)
        model.subscriptionUsage = fixture
        model.subscriptionUsage.readAt = Date()
        precondition(model.weeklyText == "68%", "Fresh account quota did not reach the compact strip")
        model.subscriptionUsage.readAt = Date().addingTimeInterval(-301)
        precondition(model.weeklyText == "10%", "An old account read overrode the local source")
    }

    @MainActor private static func readerLifecycle() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("companion-usage-test-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let executable = URL(fileURLWithPath: CommandLine.arguments[0])
        for mode in ["success", "unsupported", "malformed", "api-key", "signed-out", "early-exit", "hang", "oversized", "cancel"] {
            try (mode == "cancel" ? "hang" : mode).write(to: directory.appendingPathComponent("fixture-mode"), atomically: true, encoding: .utf8)
            try? FileManager.default.removeItem(at: directory.appendingPathComponent("child-pid"))
            let reader = CodexSubscriptionReader(executable: { executable }, timeout: mode == "hang" ? 0.8 : 3)
            var result: SubscriptionUsage?
            reader.read(codexHome: directory.path) { result = $0 }
            if mode == "cancel" {
                spin(until: { FileManager.default.fileExists(atPath: directory.appendingPathComponent("child-pid").path) }, timeout: 2)
                reader.cancel()
                RunLoop.main.run(until: Date().addingTimeInterval(0.5))
                precondition(result == nil, "Cancelled reader delivered a result")
            } else {
                spin(until: { result != nil }, timeout: 4)
                precondition(result != nil, "Reader never finished: \(mode)")
                if mode == "success" {
                    precondition(result?.tokens?.summary.lifetimeTokens == 1234567890 && result?.limits != nil,
                                 "Synthetic RPC failed: \(String(describing: result))")
                } else if ["unsupported", "malformed", "hang", "oversized"].contains(mode) {
                    precondition(result?.tokens == nil && result?.limits != nil && result?.tokenError != nil)
                    precondition(result?.tokenError?.contains("private-server-detail") == false)
                } else { precondition(result?.error != nil && result?.tokens == nil) }
            }
            if let pidText = try? String(contentsOf: directory.appendingPathComponent("child-pid"), encoding: .utf8), let pid = Int32(pidText) {
                spin(until: { kill(pid, 0) != 0 }, timeout: 2)
                precondition(kill(pid, 0) != 0, "The CLI helper survived \(mode)")
            }
        }
        let missing = CodexSubscriptionReader(executable: { nil })
        missing.read(codexHome: "") { precondition($0.error?.contains("Install Codex") == true) }
    }

    @MainActor private static func spin(until condition: () -> Bool, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.01)) }
    }

    @MainActor private static func layouts(output: URL) throws {
        for camera in [false, true] {
            let screen = CGRect(x: -10000, y: -10000, width: 1280, height: 720)
            let model = CompanionModel(usageReader: FakeReader())
            model.metrics = camera ? NotchMetrics(screen: screen, safeTop: 32,
                topLeft: CGRect(x: screen.minX, y: screen.maxY - 32, width: 550, height: 32),
                topRight: CGRect(x: screen.minX + 730, y: screen.maxY - 32, width: 550, height: 32)) : NotchMetrics(screen: screen)
            model.snapshot = CodexSnapshot(weekly: WeeklyUsage(usedPercent: 32, resetsAt: 4_000_000_000, expired: false), loading: false)
            model.expanded = true
            model.showingUsage = true
            let panel = NSPanel(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
            panel.isReleasedWhenClosed = false
            let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { true })
            defer { presentation.stop(); model.stop(); panel.close() }
            var expected: NSRect?
            for state in ["loading", "ready", "signed-out", "unsupported"] {
                model.subscriptionUsage = fixture
                model.usageLoading = state == "loading"
                if state == "loading" { model.subscriptionUsage = SubscriptionUsage() }
                if state == "signed-out" { model.subscriptionUsage = .failure("Sign in to Codex with your ChatGPT subscription, then refresh.") }
                if state == "unsupported" {
                    model.subscriptionUsage.tokens = nil
                    model.subscriptionUsage.tokenError = "Update Codex CLI to read token activity."
                }
                presentation.update(screen: screen, animated: false)
                RunLoop.main.run(until: Date().addingTimeInterval(0.1))
                precondition(panel.frame.width == model.compactWidth && panel.frame.maxY == screen.maxY && screen.contains(panel.frame))
                if let expected { precondition(panel.frame == expected, "Usage state resized the panel") }
                expected = panel.frame
                let view = presentation.surface
                let image = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
                view.cacheDisplay(in: view.bounds, to: image)
                try image.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("usage-\(camera ? "camera" : "external")-\(state).png"))
                if state == "ready" {
                    // Both pages remain mounted. Select the Usage viewport by
                    // its specified height, rather than the first page's view.
                    let usageHeight = (330 * model.metrics.detailScale).rounded()
                    guard let scroll = scrollViews(in: view).first(where: { abs($0.frame.height - usageHeight) < 1 }),
                          let document = scroll.documentView else { fatalError("No usage scroll view") }
                    precondition(abs(scroll.contentView.bounds.minY) < 1, "The first account response scrolled past the subscription limits")
                    precondition(document.bounds.height > scroll.contentSize.height, "Remaining usage fields must be reachable by scrolling")
                    scroll.contentView.scroll(to: CGPoint(x: 0, y: document.bounds.height - scroll.contentSize.height))
                    scroll.reflectScrolledClipView(scroll.contentView)
                    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
                    let offset = scroll.contentView.bounds.minY
                    model.subscriptionUsage.readAt = model.subscriptionUsage.readAt?.addingTimeInterval(1)
                    presentation.update(screen: screen, animated: false)
                    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
                    precondition(scrollViews(in: view).contains(where: { $0 === scroll }) && abs(scroll.contentView.bounds.minY - offset) < 1,
                                 "A usage refresh reset the scroll position or replaced the scroll view")
                    view.cacheDisplay(in: view.bounds, to: image)
                    try image.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("usage-\(camera ? "camera" : "external")-bottom.png"))
                }
            }
            model.showTasks()
            presentation.update(screen: screen, animated: false)
            precondition(!model.showingUsage && panel.frame.width == model.compactWidth)
            model.expanded = false
            presentation.update(screen: screen, animated: false)
            precondition(panel.frame.height == model.compactHeight)
        }
    }

    private static func scrollViews(in view: NSView) -> [NSScrollView] {
        ((view as? NSScrollView).map { [$0] } ?? []) + view.subviews.flatMap { scrollViews(in: $0) }
    }

    @MainActor private static func pinnedNavigation(output: URL) throws {
        for camera in [false, true] {
            let screen = CGRect(x: -10000, y: -10000, width: 1280, height: 720)
            let reader = FakeReader()
            let model = CompanionModel(usageReader: reader)
            if camera {
                model.metrics = NotchMetrics(screen: screen, safeTop: 32,
                    topLeft: CGRect(x: screen.minX, y: screen.maxY - 32, width: 550, height: 32),
                    topRight: CGRect(x: screen.minX + 730, y: screen.maxY - 32, width: 550, height: 32))
            }
            model.expanded = true
            model.showingCompleted = true
            model.showUsage()
            reader.completions.last!(fixture)
            let panel = NSPanel(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
            panel.isReleasedWhenClosed = false
            let presentation = NotchPresentation(panel: panel, model: model, reduceMotion: { true })
            defer { presentation.stop(); model.stop(); panel.close() }
            presentation.update(screen: screen, availableHeight: 280, animated: false)
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            let surface = presentation.surface
            let scrolls = scrollViews(in: surface)
            precondition(scrolls.count >= 2, "The short-screen fixture did not exercise overflow")
            let top = scrolls[0].convert(scrolls[0].bounds, to: surface).minY
            precondition(top > model.compactHeight + 30, "Page navigation scrolled with the body")
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
            for y in stride(from: Int((model.compactHeight + 4) * scale), to: Int((top - 4) * scale), by: 2) {
                for x in stride(from: 10, to: before.pixelsWide - 10, by: 2) {
                    precondition(before.colorAt(x: x, y: y) == after.colorAt(x: x, y: y),
                                 "Scrolling moved or obscured Tasks / Usage navigation")
                }
            }
            try after.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("usage-pinned-navigation-\(camera).png"))
            for _ in 0..<10 {
                model.showTasks()
                precondition(!model.showingUsage && model.showingCompleted, "Returning lost the previous task filter")
                model.showUsage()
            }
            precondition(reader.homes.count == 1, "Repeated navigation launched extra subscription requests")
        }
    }
}
