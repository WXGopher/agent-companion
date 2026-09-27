// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum MenuBarLifecycleTests {
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
        let bridge = FixtureUsageBridge(), model = CompanionModel(usageBridge: FixtureUsageBridge())
        let updates = FixtureAppUpdateBridge()
        updates.value = FixtureAppUpdateBridge.available
        let menu = CompanionModel(usageBridge: bridge, updateBridge: updates)
        var settingsOpened = 0
        let controller = MenuBarController(model: model, menuModel: menu,
            themePreferences: ThemeTests.darkPreferences, menuBarPlacement: MenuBarPlacement(autosaveName: name),
            settingsOverride: { settingsOpened += 1 }, usageBridge: bridge)
        updates.onPanelOpen = {
            precondition(controller.menuPanelIsVisible && menu.isPresented,
                         "The update request started before the native popup was shown")
            precondition(menu.appUpdate == FixtureAppUpdateBridge.available,
                         "A cached update was not visible before requesting another release check")
        }
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        model.stop()
        for instance in model.instances { bridge.set(SubscriptionUsage(), for: instance.usageSource) }
        model.refreshUsageSnapshot()
        defer { controller.quit() }
        precondition(controller.menuBarIsVisible && NSApp.activationPolicy() == .accessory,
                     "Legacy settings hid the only app entry or restored a Dock tile")
        precondition(!NSApp.windows.contains { $0.isVisible && $0 is NSPanel },
                     "Launching the menu app created an unsolicited overlay panel")
        precondition(updates.panelOpens == 0 && updates.snapshotReads == 0,
                     "Launching the menu app requested or polled for releases")
        NSApp.activate(ignoringOtherApps: true)
        settle(0.2)
        ThemeTests.openMenuPanel(controller)
        menu.stop()
        precondition(menu.isPresented && !menu.showingUsage)
        precondition(updates.panelOpens == 1, "An intentional popup opening did not notify the update service")
        controller.showUsage()
        precondition(menu.showingUsage && bridge.count("history") == 1 && bridge.count("panelOpen") == 1)
        controller.showUsage()
        controller.showTasks()
        precondition(bridge.count("panelOpen") == 1 && bridge.count("history") == 1,
                     "Navigation in an open popup refreshed quota")
        menu.refresh()
        precondition(updates.panelOpens == 1, "Navigation or snapshot polling requested another release check")
        close(controller, model: menu)
        precondition(controller.menuBarIsVisible && !menu.animatesTaskActivity)

        // Reopening the app is a real route to its popup, not a settings-only
        // recovery path. Observe the actual native opening event as for clicks.
        ThemeTests.openMenuPanel(controller) {
            _ = controller.applicationShouldHandleReopen(NSApp, hasVisibleWindows: false)
        }
        menu.stop()
        precondition(!menu.showingUsage && settingsOpened == 0 && bridge.count("panelOpen") == 2 && updates.panelOpens == 2)
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
        precondition(updates.panelOpens == 2, "Wake or display notifications requested a release check")
        ThemeTests.openMenuPanel(controller)
        precondition(updates.panelOpens == 3)
        controller.quit()
        precondition(!controller.menuBarIsVisible && !controller.menuBarIsAnimating)
        print("Menu lifecycle: accessory-only launch, popup click/reopen, explicit post-show update checks, Usage, Settings, wake and clean shutdown passed")
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
