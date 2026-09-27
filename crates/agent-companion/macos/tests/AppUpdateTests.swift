// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum AppUpdateTests {
    static func run(output: URL) throws {
        verifyReadOnlyPolling()
        verifyReleaseTargets()
        let attribute = "AXEnhancedUserInterface" as NSString
        let read = NSSelectorFromString("accessibilityAttributeValue:")
        let write = NSSelectorFromString("accessibilitySetValue:forAttribute:")
        let original = NSApp.perform(read, with: attribute)?.takeUnretainedValue() ?? NSNumber(value: false)
        NSApp.perform(write, with: NSNumber(value: true), with: attribute)
        defer { NSApp.perform(write, with: original, with: attribute) }
        try verifyNotice(output: output)
        print("Updates: read-only open-panel polling, safe release link, native button action, hidden/available states and stable Tasks/Usage footer passed")
    }

    private static func verifyReadOnlyPolling() {
        let updates = FixtureAppUpdateBridge()
        let model = CompanionModel(usageBridge: FixtureUsageBridge(), updateBridge: updates)
        model.start()
        defer { model.stop() }
        precondition(updates.panelOpens == 0 && updates.snapshotReads == 0,
                     "Starting the background model requested or polled for an update")
        model.isPresented = true
        model.refresh()
        let initialReads = updates.snapshotReads
        updates.value = FixtureAppUpdateBridge.available
        let deadline = Date().addingTimeInterval(1.4)
        while model.appUpdate != FixtureAppUpdateBridge.available && Date() < deadline { settle(0.02) }
        precondition(model.appUpdate == FixtureAppUpdateBridge.available && updates.snapshotReads > initialReads,
                     "An open panel did not receive the completed background update check")
        model.showUsage()
        model.showTasks()
        precondition(updates.panelOpens == 0, "Rendering, polling or page navigation requested a release check")
        model.isPresented = false
        let closedReads = updates.snapshotReads
        settle(1.1)
        precondition(updates.snapshotReads == closedReads && updates.panelOpens == 0,
                     "The closed panel continued polling for updates")
    }

    private static func verifyReleaseTargets() {
        let updates = FixtureAppUpdateBridge()
        var opened: [URL] = []
        let model = CompanionModel(usageBridge: FixtureUsageBridge(), updateBridge: updates,
                                   openReleaseURL: { opened.append($0) })
        model.isPresented = true
        for url in ["https://example.com/releases/tag/v0.3.22",
                    "http://github.com/WXGopher/agent-companion/releases/tag/v0.3.22",
                    "https://github.com/another/project/releases/tag/v0.3.22",
                    "https://user@github.com/WXGopher/agent-companion/releases/tag/v0.3.22",
                    "https://github.com/WXGopher/agent-companion/releases/tag/"] {
            updates.value = AppUpdateSnapshot(latestVersion: "0.3.22", releaseUrl: url)
            model.refreshUpdateSnapshot()
            model.viewRelease()
            precondition(model.appUpdate.release == nil && opened.isEmpty,
                         "An untrusted or incomplete release URL was accepted")
        }
    }

    private static func verifyNotice(output: URL) throws {
        let store = ThemePreferenceStore(domain: "com.wxgopher.agent-companion.tests.update.\(UUID().uuidString)")
        let preferences = ThemePreferences(store: store)
        defer {
            CFPreferencesSetAppValue(ThemePreferenceStore.key, nil, store.domain as CFString)
            CFPreferencesAppSynchronize(store.domain as CFString)
        }
        let updates = FixtureAppUpdateBridge(), usage = FixtureUsageBridge()
        var opened: [URL] = []
        let model = CompanionModel(usageBridge: usage, updateBridge: updates, openReleaseURL: { opened.append($0) })
        model.isPresented = true
        model.snapshot = CodexSnapshot(activeCount: 10, tasks: (0..<10).map { index in
            CodexTask(id: "update-task-\(index)", title: "Synthetic task \(index)", project: "Workspace", cwd: nil,
                      client: "cli", state: "running", updatedAt: 0, transcriptPath: nil)
        }, loading: false, instances: [
            CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/synthetic/codex"),
            CodexInstance(instanceId: "dodex", label: "Dodex", codexHome: "/synthetic/dodex")
        ])
        for instance in model.instances { usage.set(SubscriptionUsageTests.fixture, for: instance.usageSource) }
        model.refreshUsageSnapshot()
        let fixture = PopupTestWindow(model: model, themePreferences: preferences)
        let panel = fixture.panel, host = fixture.host
        defer { model.stop(); panel.close() }
        let panelFrame = panel.frame
        let footerFrame = element("companion-theme-toggle", in: host).accessibilityFrame!()
        let scrolls = scrollViews(in: host)
        let initialHeights = scrolls.map { $0.contentSize.height }
        precondition(!elements(in: host).contains { $0.accessibilityIdentifier?() == "popup-update-notice" },
                     "A current or unknown release displayed an update notice")
        updates.value = FixtureAppUpdateBridge.available
        model.refreshUpdateSnapshot()
        settle()
        let shownHeights = scrolls.map { $0.contentSize.height }
        precondition(zip(initialHeights, shownHeights).contains { $0.0 - $0.1 >= 30 },
                     "The update row did not consume the page viewport")
        for theme in [CompanionTheme.dark, .light] {
            precondition(preferences.set(theme))
            for showUsage in [false, true] {
                model.showingUsage = showUsage
                settle()
                precondition(panel.frame == panelFrame && host.bounds.size == CGSize(width: 356, height: 560),
                             "An update notice resized the popup")
                precondition(element("companion-theme-toggle", in: host).accessibilityFrame!() == footerFrame,
                             "An update notice moved or clipped the footer")
                precondition(scrollViews(in: host).elementsEqual(scrolls, by: { $0 === $1 }),
                             "The update row replaced a page's native scroll view")
                let title = element("popup-update-version", in: host)
                let value = (title as? NSObject)?.perform(NSSelectorFromString("accessibilityValue"))?.takeUnretainedValue() as? String
                precondition((title.accessibilityLabel?() ?? value) == "新版本 v0.3.22 可用")
                let button = element("popup-update-link", in: host)
                let buttonFrame = button.accessibilityFrame!()
                let bounds = panel.convertToScreen(host.convert(host.bounds, to: nil))
                precondition(bounds.contains(buttonFrame) && buttonFrame.minY > footerFrame.maxY,
                             "The update button overlaps the footer or falls outside the popup")
                precondition(button.accessibilityPerformPress?() == true,
                             "The native release button rejected its press action")
                precondition(opened.last?.absoluteString == FixtureAppUpdateBridge.releaseURL,
                             "The update button did not open the advertised GitHub release")
                try capture(host).representation(using: .png, properties: [:])!.write(to:
                    output.appendingPathComponent("update-popup-\(theme.rawValue)-\(showUsage ? "usage" : "tasks").png"))
            }
        }
        updates.value = AppUpdateSnapshot()
        model.refreshUpdateSnapshot()
        settle()
        precondition(!elements(in: host).contains { $0.accessibilityIdentifier?() == "popup-update-notice" }
                     && element("companion-theme-toggle", in: host).accessibilityFrame!() == footerFrame,
                     "Removing a release notice left stale UI or shifted the footer")
        precondition(updates.panelOpens == 0 && usage.events.isEmpty,
                     "Rendering the update notice queried releases or account usage")
    }

    private static func element(_ identifier: String, in root: NSView) -> AnyObject {
        guard let value = elements(in: root).first(where: { $0.accessibilityIdentifier?() == identifier }) else {
            fatalError("Missing update accessibility element: \(identifier)")
        }
        return value
    }

    private static func elements(in root: Any) -> [AnyObject] {
        var seen = Set<ObjectIdentifier>()
        func visit(_ object: Any) -> [AnyObject] {
            let element = object as AnyObject
            guard seen.insert(ObjectIdentifier(element)).inserted else { return [] }
            return [element] + (element.accessibilityChildren?() ?? []).flatMap(visit)
        }
        return visit(root)
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

    private static func settle(_ interval: TimeInterval = 0.08) {
        RunLoop.main.run(until: Date().addingTimeInterval(interval))
    }
}
