// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum MenuBarLifecycleTests {
    private final class Reader: SubscriptionReading {
        var reads = 0
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            reads += 1
            completion(.failure("Synthetic sign-in guidance"))
        }
        func cancel() {}
    }

    static func run() {
        let name = "MenuLifecycle-\(UUID().uuidString)"
        let defaults = UserDefaults.standard
        let keys = ["showMenuBarIcon", "showNotch", "showDockIcon", "notchDisplay",
                    "NSStatusItem Visible \(name)", "NSStatusItem Preferred Position \(name)"]
        let previous = keys.map { defaults.object(forKey: $0) }
        defer {
            for (key, value) in zip(keys, previous) {
                if let value { defaults.set(value, forKey: key) } else { defaults.removeObject(forKey: key) }
            }
        }
        // Only the standalone test domain is changed. Stale settings must not
        // hide the sole entry, create a Dock tile, or spawn an overlay window.
        precondition(Bundle.main.bundleIdentifier != "com.wxgopher.agent-companion")
        defaults.set(false, forKey: "showMenuBarIcon")
        defaults.set(true, forKey: "showNotch")
        defaults.set(true, forKey: "showDockIcon")
        defaults.set(["id": "missing-display", "name": "Disconnected"], forKey: "notchDisplay")
        defaults.set(false, forKey: "NSStatusItem Visible \(name)")
        let reader = Reader(), model = CompanionModel(usageReader: Reader())
        let menu = CompanionModel(usageReader: reader)
        var settingsOpened = 0
        let controller = MenuBarController(model: model, menuModel: menu,
            themePreferences: ThemeTests.darkPreferences, menuBarPlacement: MenuBarPlacement(autosaveName: name),
            settingsOverride: { settingsOpened += 1 }, usageReader: { _ in reader })
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        model.stop()
        defer { controller.quit() }
        precondition(controller.menuBarIsVisible && NSApp.activationPolicy() == .accessory,
                     "Legacy settings hid the only app entry or restored a Dock tile")
        precondition(!NSApp.windows.contains { $0.isVisible && $0 is NSPanel },
                     "Launching the menu app created an unsolicited overlay panel")
        NSApp.activate(ignoringOtherApps: true)
        settle(0.2)
        ThemeTests.openMenuPanel(controller)
        menu.stop()
        precondition(menu.isPresented && !menu.showingUsage)
        controller.showUsage()
        precondition(menu.showingUsage && reader.reads == 1)
        close(controller, model: menu)
        precondition(controller.menuBarIsVisible && !menu.animatesTaskActivity)

        // Reopening the app is a real route to its popup, not a settings-only
        // recovery path. Observe the actual native opening event as for clicks.
        ThemeTests.openMenuPanel(controller) {
            _ = controller.applicationShouldHandleReopen(NSApp, hasVisibleWindows: false)
        }
        menu.stop()
        precondition(!menu.showingUsage && settingsOpened == 0)
        controller.showSettings()
        waitUntilClosed(controller, model: menu)
        precondition(settingsOpened == 1 && controller.menuBarIsVisible)
        for name in [NSWorkspace.didWakeNotification, NSWorkspace.activeSpaceDidChangeNotification] {
            NSWorkspace.shared.notificationCenter.post(name: name, object: nil)
        }
        NotificationCenter.default.post(name: NSApplication.didChangeScreenParametersNotification, object: nil)
        settle(0.1)
        precondition(controller.menuBarIsVisible && !controller.menuPanelIsVisible
                     && NSApp.activationPolicy() == .accessory)
        ThemeTests.openMenuPanel(controller)
        controller.quit()
        precondition(!controller.menuBarIsVisible && !controller.menuBarIsAnimating)
        print("Menu lifecycle: legacy settings ignored, accessory-only launch, popup click/reopen, Usage, Settings, wake and clean shutdown passed")
    }

    private static func close(_ controller: MenuBarController, model: CompanionModel) {
        controller.toggleMenuPanel()
        waitUntilClosed(controller, model: model)
    }
    private static func waitUntilClosed(_ controller: MenuBarController, model: CompanionModel) {
        let deadline = Date().addingTimeInterval(2)
        while (controller.menuPanelIsVisible || model.isPresented) && Date() < deadline { settle(0.05) }
        precondition(!controller.menuPanelIsVisible && !model.isPresented,
                     "Closing the popup failed within two seconds")
    }
    private static func settle(_ interval: TimeInterval) { RunLoop.main.run(until: Date().addingTimeInterval(interval)) }
}
