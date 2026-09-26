// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

@MainActor enum PopupHeaderTests {
    private final class Reader: SubscriptionReading {
        var requests = 0
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) { requests += 1 }
        func cancel() {}
    }

    static func run(output: URL) throws {
        let expectedVersion = ProcessInfo.processInfo.environment["AGENT_COMPANION_TEST_VERSION"] ?? "0.0.0-test"
        precondition(CompanionAppInfo.version == expectedVersion,
                     "The native header did not read the version supplied by the binary")
        let attribute = "AXEnhancedUserInterface" as NSString
        let read = NSSelectorFromString("accessibilityAttributeValue:")
        let write = NSSelectorFromString("accessibilitySetValue:forAttribute:")
        let original = NSApp.perform(read, with: attribute)?.takeUnretainedValue() ?? NSNumber(value: false)
        NSApp.perform(write, with: NSNumber(value: true), with: attribute)
        defer { NSApp.perform(write, with: original, with: attribute) }

        try verifyPopup(version: expectedVersion, output: output)
        try verifyPopup(version: "10.20.30-rc.12", output: output)
        print("Popup header: binary version, full app title, exact running-only counts, separate waiting status, prerelease fit, stable footer/scroll state passed")
    }

    private static func snapshot(running: Int, waiting: Bool = true) -> CodexSnapshot {
        let now = Date().timeIntervalSince1970
        let states = Array(repeating: "running", count: running) + (waiting ? ["waiting"] : []) + ["completed", "failed"]
        return CodexSnapshot(activeCount: running + (waiting ? 1 : 0), completedCount: 2,
            tasks: states.enumerated().map { index, state in
                CodexTask(id: "header-\(index)", title: "Synthetic task \(index)", project: "Workspace", cwd: nil,
                          client: "cli", state: state, updatedAt: now - 90, transcriptPath: nil)
            }, weekly: WeeklyUsage(usedPercent: 13, resetsAt: now + 3 * 86_400, expired: false), loading: false)
    }

    private static func verifyPopup(version: String, output: URL) throws {
        let store = ThemePreferenceStore(domain: "com.wxgopher.agent-companion.tests.header.\(UUID().uuidString)")
        let preferences = ThemePreferences(store: store)
        defer {
            CFPreferencesSetAppValue(ThemePreferenceStore.key, nil, store.domain as CFString)
            CFPreferencesAppSynchronize(store.domain as CFString)
        }
        let reader = Reader()
        let model = CompanionModel(usageReader: reader)
        model.isPresented = true
        model.subscriptionUsage = SubscriptionUsageTests.fixture
        let panel = NSPanel(contentRect: CGRect(x: -10000, y: -10000, width: 356, height: 560),
                            styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        let host = NSHostingView(rootView: CompanionPopupView(model: model, themePreferences: preferences, version: version))
        panel.contentView = host
        host.frame = CGRect(x: 0, y: 0, width: 356, height: 560)
        panel.orderFront(nil)
        defer { model.stop(); panel.close() }
        settle()
        let frame = panel.frame
        let footer = element("companion-theme-toggle", in: host).accessibilityFrame!()
        let prerelease = version != CompanionAppInfo.version
        for theme in [CompanionTheme.dark, .light] {
            precondition(preferences.set(theme))
            for count in prerelease ? [12] : [0, 1, 2, 125] {
                model.snapshot = snapshot(running: count)
                settle()
                let name = "popup-header-\(theme.rawValue)-\(prerelease ? "prerelease" : String(count))"
                try capture(host).representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent(name + ".png"))
                verifyHeader(in: host, version: version, count: count, waiting: true)
                precondition(panel.frame == frame && host.bounds.size == CGSize(width: 356, height: 560),
                             "Changing the header's count or theme resized the popup")
                precondition(element("companion-theme-toggle", in: host).accessibilityFrame!() == footer,
                             "The header moved the footer as its count changed")
            }
        }
        model.snapshot = snapshot(running: 12, waiting: false)
        model.dashboardError = String(repeating: "Synthetic session guidance. ", count: 24)
        settle()
        verifyHeader(in: host, version: version, count: 12, waiting: false)
        let scrolls = scrollViews(in: host)
        precondition(scrolls.count >= 4, "The fixture must include both pages' inner and outer scroll views")
        for scroll in scrolls {
            if let document = scroll.documentView {
                scroll.contentView.scroll(to: CGPoint(x: 0, y: min(35, max(0, document.bounds.height - scroll.contentSize.height))))
                scroll.reflectScrolledClipView(scroll.contentView)
            }
        }
        settle()
        let offsets = scrolls.map { $0.contentView.bounds.origin }
        precondition(offsets.contains { $0.y > 20 }, "The header fixture did not exercise an actual scroll offset")
        for usage in [true, false] {
            model.showingUsage = usage
            precondition(preferences.toggle())
            settle()
            verifyHeader(in: host, version: version, count: 12, waiting: false)
            precondition(panel.frame == frame && element("companion-theme-toggle", in: host).accessibilityFrame!() == footer,
                         "A page or theme change moved the popup header or footer")
            precondition(scrollViews(in: host).elementsEqual(scrolls, by: { $0 === $1 })
                         && scrolls.map { $0.contentView.bounds.origin } == offsets,
                         "Updating the header replaced a scroll view or changed a saved offset")
        }
        precondition(reader.requests == 0, "Rendering or updating the app header requested subscription usage")
    }

    private static func verifyHeader(in host: NSView, version: String, count: Int, waiting: Bool) {
        let elements = accessibilityElements(in: host)
        let title = element("popup-app-title", in: host)
        let versionLabel = element("popup-app-version", in: host)
        let running = element("popup-running-count", in: host)
        precondition(accessibleText(title) == CompanionAppInfo.title,
                     "The app title is missing from the native text element: \(String(describing: accessibleText(title)))")
        precondition(accessibleText(versionLabel) == "Version " + version,
                     "The native header exposes the wrong version: \(String(describing: accessibleText(versionLabel)))")
        precondition(running.accessibilityLabel?() == "\(count) running \(count == 1 ? "task" : "tasks")",
                     "Waiting or finished tasks inflated the running count, or a live count stayed stale")
        precondition(elements.contains { $0.accessibilityIdentifier?() == "popup-needs-input" } == waiting,
                     "The header lost or retained a stale waiting-for-input indicator")
        let header = element("popup-header", in: host).accessibilityFrame!()
        // A contained AX group reports the union of its children, not the
        // padding around them. Verify that union stays inside the actual strip.
        let strip = CGRect(x: host.bounds.minX, y: host.isFlipped ? host.bounds.minY : host.bounds.maxY - 28,
                           width: host.bounds.width, height: 28)
        let headerBounds = host.window!.convertToScreen(host.convert(strip, to: nil))
        precondition(header.height > 8 && headerBounds.insetBy(dx: -1, dy: -1).contains(header),
                     "The popup header overflowed its 28-point strip: \(header) outside \(headerBounds)")
        let titleFrame = title.accessibilityFrame!(), versionFrame = versionLabel.accessibilityFrame!()
        let runningFrame = running.accessibilityFrame!()
        for frame in [titleFrame, versionFrame, runningFrame] {
            precondition(header.insetBy(dx: -1, dy: -1).contains(frame), "Header text overflowed its fixed strip")
        }
        precondition(titleFrame.maxX <= versionFrame.minX && versionFrame.maxX <= runningFrame.minX,
                     "The full app name, version and running count overlap")
        let titleWidth = (CompanionAppInfo.title as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12, weight: .semibold)]).width
        let versionWidth = (("v" + version) as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 10)]).width
        precondition(titleFrame.width >= titleWidth - 1 && versionFrame.width >= versionWidth * 0.8 - 1,
                     "The title or prerelease version was truncated instead of fitting the header")
    }

    private static func element(_ identifier: String, in host: NSView) -> AnyObject {
        guard let element = accessibilityElements(in: host).first(where: { $0.accessibilityIdentifier?() == identifier }) else {
            fatalError("Missing native accessibility element: \(identifier)")
        }
        return element
    }

    private static func accessibleText(_ element: AnyObject) -> String? {
        // AppKit static text uses AXValue; explicitly labeled groups use AXLabel.
        if let label = element.accessibilityLabel?() { return label }
        return (element as? NSObject)?.perform(NSSelectorFromString("accessibilityValue"))?.takeUnretainedValue() as? String
    }

    private static func accessibilityElements(in root: Any) -> [AnyObject] {
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

    private static func settle() { RunLoop.main.run(until: Date().addingTimeInterval(0.08)) }

    private static func capture(_ view: NSView) -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return NSBitmapImageRep(data: bitmap.representation(using: .png, properties: [:])!)!
    }
}
