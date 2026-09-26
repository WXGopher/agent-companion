// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

@MainActor enum ThemeTests {
    // Existing pixel/layout tests deliberately use dark mode regardless of the
    // user's saved preference. This read-only, unique domain writes no defaults.
    static let darkPreferences = ThemePreferences(store: ThemePreferenceStore(
        domain: "com.wxgopher.agent-companion.tests.dark.\(UUID().uuidString)"))

    private struct Preferences {
        let domain = "com.wxgopher.agent-companion.tests.theme.\(UUID().uuidString)"
        let store: ThemePreferenceStore
        let value: ThemePreferences
        init() {
            store = ThemePreferenceStore(domain: domain)
            value = ThemePreferences(store: store)
        }
        func remove() {
            for key in [ThemePreferenceStore.key as String] {
                CFPreferencesSetAppValue(key as CFString, nil, domain as CFString)
            }
            CFPreferencesAppSynchronize(domain as CFString)
        }
    }

    private final class Reader: SubscriptionReading {
        var requests = 0
        var cancellations = 0
        var completion: ((SubscriptionUsage) -> Void)?
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            requests += 1
            self.completion = completion
        }
        func cancel() { cancellations += 1 }
    }

    // isShown becomes true before AppKit finishes opening. Ordinary dismissal
    // tests must observe completion instead of racing a fixed animation delay.
    static func openMenuPanel(_ controller: MenuBarController, open: (() -> Void)? = nil) {
        guard let host = controller.menuPanelContentView else { fatalError("The menu popup has no content view") }
        precondition(!controller.menuPanelIsVisible, "The popup must be closed before testing its opening")
        var didShow = false
        let observer = NotificationCenter.default.addObserver(forName: NSPopover.didShowNotification,
                                                               object: nil, queue: .main) { notification in
            guard let popover = notification.object as? NSPopover,
                  popover.contentViewController?.view === host else { return }
            didShow = true
        }
        defer { NotificationCenter.default.removeObserver(observer) }
        if let open { open() } else { controller.toggleMenuPanel() }
        let deadline = Date().addingTimeInterval(2)
        while !didShow && Date() < deadline { settle(0.01) }
        precondition(didShow && controller.menuPanelIsVisible,
                     "Native popup opening failed within two seconds: didShow=\(didShow), isShown=\(controller.menuPanelIsVisible), " +
                     "active=\(NSApp.isActive), policy=\(NSApp.activationPolicy().rawValue), hostUnchanged=\(controller.menuPanelContentView === host), " +
                     "windowVisible=\(String(describing: host.window?.isVisible)), buttonFrame=\(String(describing: controller.menuBarButton?.window?.frame))")
    }

    static func run(output: URL) throws {
        let originalAppearance = NSApp.appearance
        // SwiftUI lazily publishes virtual accessibility nodes when an assistive
        // client requests enhanced UI. Enable it on this test process only; no
        // cross-process AX access, desktop automation or TCC permission is used.
        let attribute = "AXEnhancedUserInterface" as NSString
        let readAttribute = NSSelectorFromString("accessibilityAttributeValue:")
        let writeAttribute = NSSelectorFromString("accessibilitySetValue:forAttribute:")
        let originalAccessibility = NSApp.perform(readAttribute, with: attribute)?.takeUnretainedValue() ?? NSNumber(value: false)
        NSApp.perform(writeAttribute, with: NSNumber(value: true), with: attribute)
        defer {
            NSApp.appearance = originalAppearance
            NSApp.perform(writeAttribute, with: originalAccessibility, with: attribute)
        }
        verifyPersistence()
        try verifyPopupLayouts(output: output)
        try verifyLiveSwitching(output: output)
        try verifyControllerAppearance(output: output)
        print("Themes: isolated persistence/fallback, native footer actions, opposite system appearance, popup chrome, retained navigation/scroll state and 356×560 layouts passed")
    }

    private static func verifyPersistence() {
        let preferences = Preferences()
        defer { preferences.remove() }
        precondition(preferences.store.read() == .dark && preferences.value.theme == .dark,
                     "A missing theme preference must retain the original dark appearance")
        for theme in [CompanionTheme.light, .dark, .light] {
            precondition(preferences.value.set(theme))
            precondition(ThemePreferences(store: ThemePreferenceStore(domain: preferences.domain)).theme == theme,
                         "The selected theme did not survive a fresh preferences object")
            precondition(CFPreferencesCopyAppValue(ThemePreferenceStore.key as CFString,
                                                  preferences.domain as CFString) as? String == theme.rawValue)
        }
        for invalid: CFPropertyList in ["sepia" as CFString, NSNumber(value: 9)] {
            CFPreferencesSetAppValue(ThemePreferenceStore.key as CFString, invalid, preferences.domain as CFString)
            CFPreferencesAppSynchronize(preferences.domain as CFString)
            preferences.value.reload()
            precondition(preferences.value.theme == .dark && preferences.store.read() == .dark,
                         "An unknown or incorrectly typed preference must fall back to dark")
        }
        precondition(preferences.store.write(.light))
        preferences.value.reload()
        precondition(preferences.value.theme == .light, "Reload missed a separately written preference")
        precondition(preferences.value.toggle() && preferences.store.read() == .dark)
        precondition(preferences.value.toggle() && preferences.store.read() == .light)
    }

    private static func snapshot(taskCount: Int = 3, completed: Bool = false) -> CodexSnapshot {
        let titles = ["Review session recovery", "Update dashboard layout", "Run workspace checks"]
        let tasks = (0..<taskCount).map { index in
            CodexTask(id: "theme-task-\(index)", title: titles[index % titles.count], project: "Workspace",
                      cwd: "/synthetic/workspace", client: index == 1 ? "desktop" : "cli",
                      state: completed ? "completed" : (index == 2 ? "waiting" : "running"),
                      updatedAt: Date().timeIntervalSince1970 - 180, transcriptPath: nil,
                      instanceId: index == 1 ? "dodex" : "codex", instanceLabel: index == 1 ? "Dodex" : "Codex")
        }
        return CodexSnapshot(activeCount: completed ? 0 : taskCount, completedCount: completed ? taskCount : 0,
            tasks: tasks, loading: false, instances: [
                CodexInstance(instanceId: "codex", label: "Codex", codexHome: "/synthetic/codex",
                              weekly: WeeklyUsage(usedPercent: 43, resetsAt: 4_000_000_000, expired: false)),
                CodexInstance(instanceId: "dodex", label: "Dodex", codexHome: "/synthetic/dodex",
                              weekly: WeeklyUsage(usedPercent: 28, resetsAt: 4_000_000_000, expired: false))
            ])
    }

    private static func popup(model: CompanionModel, preferences: ThemePreferences)
        -> (NSPanel, NSHostingView<CompanionPopupView>) {
        let panel = NSPanel(contentRect: CGRect(x: -10000, y: -10000, width: 356, height: 560),
                            styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        let host = NSHostingView(rootView: CompanionPopupView(model: model, themePreferences: preferences))
        panel.contentView = host
        host.frame = CGRect(x: 0, y: 0, width: 356, height: 560)
        // Exercise normal AppKit window/accessibility behavior while keeping
        // the synthetic host entirely offscreen.
        panel.orderFront(nil)
        settle()
        return (panel, host)
    }

    private static func verifyPopupLayouts(output: URL) throws {
        let preferences = Preferences()
        defer { preferences.remove() }
        let reader = Reader()
        let model = CompanionModel(usageReader: reader)
        model.isPresented = true
        model.snapshot = snapshot()
        let (panel, host) = popup(model: model, preferences: preferences.value)
        defer { model.stop(); panel.close() }
        let frame = panel.frame
        for theme in [CompanionTheme.dark, .light] {
            // Explicit app themes must remain readable when the surrounding
            // system uses the opposite appearance, including semantic colors.
            NSApp.appearance = theme.toggled.appearance
            precondition(preferences.value.set(theme))
            for scenario in ["tasks", "usage", "empty", "error", "low-quota"] {
                model.snapshot = snapshot(taskCount: scenario == "empty" ? 0 : 3)
                model.dashboardError = nil
                model.showingUsage = scenario == "usage"
                model.subscriptionUsage = SubscriptionUsageTests.fixture
                if scenario == "error" {
                    model.dashboardError = "Could not read local sessions. Check access to the workspace, then refresh."
                    model.snapshot.instances?[1].error = "Usage is temporarily unavailable."
                } else if scenario == "low-quota" {
                    model.snapshot.instances?[0].weekly = WeeklyUsage(usedPercent: 97, resetsAt: 4_000_000_000, expired: false)
                    model.snapshot.instances?[1].weekly = nil
                }
                settle()
                let pixels = capture(host)
                precondition(panel.frame == frame && host.bounds.size == CGSize(width: 356, height: 560),
                             "Theme or content changes resized the fixed menu popup")
                verifyBackground(pixels, theme: theme, size: host.bounds.size)
                verifyContent(pixels, theme: theme, size: host.bounds.size)
                try write(pixels, name: "theme-popup-\(theme.rawValue)-\(scenario)", output: output)
                if scenario == "tasks" {
                    NSApp.appearance = theme.appearance
                    settle()
                    verifySamePixels(capture(host), pixels,
                        message: "The explicit \(theme.rawValue) theme inherited system appearance colors")
                    NSApp.appearance = theme.toggled.appearance
                }
            }
        }
        precondition(reader.requests == 0, "Rendering a theme requested subscription data")
    }

    private static func verifyLiveSwitching(output: URL) throws {
        let preferences = Preferences()
        defer { preferences.remove() }
        let reader = Reader()
        let model = CompanionModel(usageReader: reader)
        model.snapshot = snapshot(taskCount: 18, completed: true)
        model.dashboardError = String(repeating: "Synthetic local sessions could not be read. ", count: 16)
        model.isPresented = true
        model.showingCompleted = true
        model.showUsage(for: "dodex")
        reader.completion?(SubscriptionUsageTests.fixture)
        precondition(reader.requests == 1)
        let (panel, host) = popup(model: model, preferences: preferences.value)
        defer { model.stop(); panel.close() }
        let frame = panel.frame
        let scrolls = scrollViews(in: host)
        precondition(scrolls.count >= 4, "The fixture must exercise outer and inner page viewports")
        for usage in [true, false] {
            if !usage { model.showTasks() }
            settle()
            for scroll in scrolls {
                if let document = scroll.documentView {
                    scroll.contentView.scroll(to: CGPoint(x: 0, y: min(70, max(0, document.bounds.height - scroll.contentSize.height))))
                    scroll.reflectScrolledClipView(scroll.contentView)
                }
            }
            settle()
            let offsets = scrolls.map { $0.contentView.bounds.origin }
            precondition(offsets.contains { $0.y > 20 }, "The fixture did not exercise saved scroll positions")
            let requests = reader.requests, cancellations = reader.cancellations
            for _ in 0..<2 {
                pressToggle(in: host, preferences: preferences.value)
                for _ in 0..<6 {
                    settle(0.01)
                    precondition(panel.frame == frame && panel.contentView === host,
                                 "Changing theme resized the popup or recreated its hosting view")
                    precondition(scrollViews(in: host).elementsEqual(scrolls, by: { $0 === $1 }),
                                 "Changing theme recreated a native scroll view")
                    precondition(scrolls.map { $0.contentView.bounds.origin } == offsets,
                                 "Changing theme reset a page's saved scroll position")
                    precondition(model.isPresented && model.showingUsage == usage && model.showingCompleted
                                 && model.selectedInstanceID == "dodex", "Changing theme lost navigation state or closed the popup")
                    precondition(reader.requests == requests && reader.cancellations == cancellations,
                                 "Changing theme restarted or cancelled a usage query")
                }
                verifyBackground(capture(host), theme: preferences.value.theme, size: host.bounds.size)
            }
        }
    }

    private static func verifyControllerAppearance(output: URL) throws {
        let preferences = Preferences()
        defer { preferences.remove() }
        NSApp.appearance = NSAppearance(named: .aqua)
        let model = CompanionModel(usageReader: Reader()), menu = CompanionModel(usageReader: Reader())
        var readers: [String: Reader] = [:]
        let controller = MenuBarController(model: model, menuModel: menu,
            themePreferences: preferences.value, settingsOverride: {}, usageReader: { source in
                if let reader = readers[source.instanceID] { return reader }
                let reader = Reader(); readers[source.instanceID] = reader; return reader
            })
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        model.stop()
        defer { controller.quit() }
        NSApp.activate(ignoringOtherApps: true)
        settle(0.2)
        openMenuPanel(controller)
        menu.stop()
        model.snapshot = snapshot()
        menu.snapshot = snapshot()
        menu.showingCompleted = true
        menu.showUsage(for: "dodex")
        settle()
        for reader in readers.values { reader.completion?(SubscriptionUsageTests.fixture) }
        settle()
        guard let host = controller.menuPanelContentView, let window = host.window,
              let statusButton = controller.menuBarButton, let image = statusButton.image else {
            fatalError("The native menu popup or template status image is missing")
        }
        precondition(controller.menuPanelIsVisible && image.isTemplate)
        let frame = window.frame
        let requests = readers.values.map(\.requests).reduce(0, +)
        for theme in [CompanionTheme.light, .dark] {
            NSApp.appearance = theme.toggled.appearance
            pressToggle(in: host, preferences: preferences.value)
            precondition(preferences.value.theme == theme && controller.menuPanelIsVisible && window.frame == frame,
                         "The native footer theme action closed or resized the popup")
            precondition(controller.menuPanelContentView === host && menu.isPresented && menu.showingUsage
                         && menu.showingCompleted && menu.selectedInstanceID == "dodex")
            precondition(controller.menuPanelAppearance?.bestMatch(from: [.aqua, .darkAqua]) == appearanceName(theme)
                         && host.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == appearanceName(theme),
                         "The popover's native chrome retained the opposite appearance")
            precondition(statusButton.image === image && image.isTemplate,
                         "An app theme replaced or recolored the system menu-bar template image")
            precondition(readers.values.map(\.requests).reduce(0, +) == requests,
                         "A native popup theme action requested subscription data")
            if let chrome = window.contentView {
                try write(capture(chrome), name: "theme-popup-native-\(theme.rawValue)", output: output)
            }
        }
        // Reopening normally resets navigation and refreshes its snapshot.
        // Check dismissal/persistence separately from the live-switch invariants.
        for theme in [CompanionTheme.dark, .light] {
            if preferences.value.theme != theme { pressToggle(in: host, preferences: preferences.value) }
            controller.toggleMenuPanel()
            let deadline = Date().addingTimeInterval(2)
            while (controller.menuPanelIsVisible || menu.isPresented) && Date() < deadline { settle(0.05) }
            precondition(!controller.menuPanelIsVisible && !menu.isPresented,
                         "The \(theme.rawValue) popup did not close and collapse its model")
            openMenuPanel(controller)
            menu.stop()
            precondition(menu.isPresented && preferences.value.theme == theme && preferences.store.read() == theme,
                         "Reopening the popup lost the saved \(theme.rawValue) theme")
            precondition(controller.menuPanelAppearance?.bestMatch(from: [.aqua, .darkAqua]) == appearanceName(theme)
                         && host.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == appearanceName(theme),
                         "Reopening the popup lost the native \(theme.rawValue) appearance")
        }
    }

    private static func pressToggle(in view: NSView, preferences: ThemePreferences) {
        let expected = preferences.theme.toggled
        guard let button = accessibilityElements(in: view).first(where: {
            $0.accessibilityIdentifier?() == "companion-theme-toggle"
        }) else {
            let elements = accessibilityElements(in: view)
            for element in elements {
                print("AX node \(type(of: element)): id=\(String(describing: element.accessibilityIdentifier?())) label=\(String(describing: element.accessibilityLabel?()))")
            }
            fatalError("The footer does not expose its theme toggle to accessibility")
        }
        precondition(button.accessibilityLabel?() == preferences.theme.toggleLabel
                     && button.accessibilityHelp?() == preferences.theme.toggleLabel,
                     "The theme button does not describe the action it will perform")
        precondition(button.accessibilityPerformPress?() == true, "The native theme button rejected its accessibility press action")
        settle()
        precondition(preferences.theme == expected, "Pressing the footer button did not switch theme")
        guard let changedButton = accessibilityElements(in: view).first(where: {
            $0.accessibilityIdentifier?() == "companion-theme-toggle"
        }) else { fatalError("Switching theme removed the footer button") }
        precondition(changedButton.accessibilityLabel?() == expected.toggleLabel,
                     "The theme button's accessible action did not update")
    }

    private static func accessibilityElements(in root: Any) -> [AnyObject] {
        var seen = Set<ObjectIdentifier>()
        func visit(_ object: Any) -> [AnyObject] {
            let element = object as AnyObject
            guard seen.insert(ObjectIdentifier(element)).inserted else { return [] }
            // SwiftUI's virtual nodes implement the Objective-C accessibility
            // selectors without declaring conformance to the full protocol.
            let children = element.accessibilityChildren?() ?? []
            return [element] + children.flatMap(visit)
        }
        return visit(root)
    }

    private static func scrollViews(in view: NSView) -> [NSScrollView] {
        ((view as? NSScrollView).map { [$0] } ?? []) + view.subviews.flatMap { scrollViews(in: $0) }
    }

    private static func settle(_ interval: TimeInterval = 0.08) {
        RunLoop.main.run(until: Date().addingTimeInterval(interval))
    }

    private static func appearanceName(_ theme: CompanionTheme) -> NSAppearance.Name {
        theme == .dark ? .darkAqua : .aqua
    }

    private static func capture(_ view: NSView) -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()
        let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return NSBitmapImageRep(data: bitmap.representation(using: .png, properties: [:])!)!
    }

    private static func write(_ pixels: NSBitmapImageRep, name: String, output: URL) throws {
        try pixels.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent("\(name).png"))
    }

    private static func verifyBackground(_ pixels: NSBitmapImageRep, theme: CompanionTheme, size: CGSize) {
        let scale = CGFloat(pixels.pixelsWide) / size.width
        for point in [CGPoint(x: 4, y: 100), CGPoint(x: 4, y: 300), CGPoint(x: 4, y: 530)] {
            verifyBackground(pixels.colorAt(x: Int(point.x * scale), y: Int(point.y * scale))!, theme: theme)
        }
    }

    private static func verifyBackground(_ color: NSColor, theme: CompanionTheme) {
        let rgb = color.usingColorSpace(.deviceRGB)!
        let luminance = (rgb.redComponent + rgb.greenComponent + rgb.blueComponent) / 3
        precondition(rgb.alphaComponent > 0.98 && (theme == .dark ? luminance < 0.1 : luminance > 0.9),
                     "The \(theme.rawValue) surface retained an incorrect or transparent background: \(rgb)")
    }

    private static func verifyContent(_ pixels: NSBitmapImageRep, theme: CompanionTheme, size: CGSize) {
        let scale = CGFloat(pixels.pixelsWide) / size.width
        for (name, area) in [("app identity", CGRect(x: 14, y: 5, width: 170, height: 19)),
                             ("footer actions", CGRect(x: 14, y: 500, width: 126, height: 50))] {
            var textPixels = 0
            for y in Int(area.minY * scale)..<Int(area.maxY * scale) {
                for x in Int(area.minX * scale)..<Int(area.maxX * scale) {
                    let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                    let luminance = (color.redComponent + color.greenComponent + color.blueComponent) / 3
                    if color.alphaComponent > 0.9 && (theme == .dark ? luminance > 0.3 : luminance < 0.6) {
                        textPixels += 1
                    }
                }
            }
            precondition(textPixels > 100, "The \(theme.rawValue) popup lost readable \(name): \(textPixels) pixels")
        }
        var ink = 0
        for y in stride(from: Int(60 * scale), to: Int(500 * scale), by: 2) {
            for x in stride(from: Int(20 * scale), to: Int(336 * scale), by: 2) {
                let color = pixels.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                let luminance = (color.redComponent + color.greenComponent + color.blueComponent) / 3
                if theme == .dark ? luminance > 0.45 : luminance < 0.45 { ink += 1 }
            }
        }
        precondition(ink > 100, "The \(theme.rawValue) popup lost readable foreground content")
    }

    private static func verifySamePixels(_ after: NSBitmapImageRep, _ before: NSBitmapImageRep, message: String) {
        precondition(after.pixelsWide == before.pixelsWide && after.pixelsHigh == before.pixelsHigh)
        var changed = 0, count = 0
        for y in stride(from: 4, to: after.pixelsHigh - 4, by: 3) {
            for x in stride(from: 4, to: after.pixelsWide - 4, by: 3) {
                let a = after.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                let b = before.colorAt(x: x, y: y)!.usingColorSpace(.deviceRGB)!
                if max(abs(a.redComponent - b.redComponent), abs(a.greenComponent - b.greenComponent),
                       abs(a.blueComponent - b.blueComponent)) > 3.0 / 255 { changed += 1 }
                count += 1
            }
        }
        precondition(Double(changed) / Double(count) < 0.005, "\(message): \(changed)/\(count) sampled pixels changed")
    }
}
