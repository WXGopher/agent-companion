// SPDX-License-Identifier: GPL-3.0-only
import AppKit

enum EntryPreferenceTests {
    private final class UsageFixture: SubscriptionReading {
        var reads = 0
        func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
            reads += 1
            completion(.failure("Synthetic sign-in guidance"))
        }
        func cancel() {}
    }
    @MainActor static func run() {
        let domain = "com.wxgopher.agent-companion.tests.entries.\(UUID().uuidString)"
        let store = EntryPreferenceStore(domain: domain)
        defer {
            for entry in [CompanionEntry.menuBar, .notch] {
                CFPreferencesSetAppValue(entry.rawValue as CFString, nil, domain as CFString)
            }
            CFPreferencesAppSynchronize(domain as CFString)
        }
        precondition(store.read(.menuBar) && !store.read(.notch))
        let preferences = EntryPreferences(store: store)
        var settingsOpened = 0
        let reader = UsageFixture()
        let menuModel = CompanionModel(usageReader: reader)
        let controller = NotchController(entries: preferences, menuModel: menuModel, settingsOverride: { settingsOpened += 1 })
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        precondition(controller.menuBarIsVisible && !controller.notchIsVisible && !controller.hasMouseMonitoring)
        NSApp.activate(ignoringOtherApps: true)
        RunLoop.main.run(until: Date().addingTimeInterval(0.25))
        controller.toggleMenuPanel()
        RunLoop.main.run(until: Date().addingTimeInterval(0.25))
        precondition(controller.menuPanelIsVisible && menuModel.expanded && !menuModel.showingUsage,
                     "Menu popup: visible=\(controller.menuPanelIsVisible), expanded=\(menuModel.expanded), usage=\(menuModel.showingUsage)")
        controller.showUsage()
        precondition(menuModel.showingUsage && reader.reads == 1)
        controller.toggleMenuPanel()
        let deadline = Date().addingTimeInterval(2)
        while (controller.menuPanelIsVisible || menuModel.expanded) && Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        }
        precondition(!controller.menuPanelIsVisible && !menuModel.expanded,
                     "Menu close: visible=\(controller.menuPanelIsVisible), expanded=\(menuModel.expanded)")
        precondition(preferences.set(.notch, visible: true))
        if !NSScreen.screens.isEmpty { precondition(controller.notchIsVisible) }
        precondition(controller.hasMouseMonitoring)
        precondition(preferences.set(.notch, visible: false))
        precondition(!controller.notchIsVisible && !controller.hasMouseMonitoring && !controller.notchIsAnimating)
        for name in [NSWorkspace.didWakeNotification, NSWorkspace.activeSpaceDidChangeNotification] {
            NSWorkspace.shared.notificationCenter.post(name: name, object: nil)
        }
        NotificationCenter.default.post(name: NSApplication.didChangeScreenParametersNotification, object: nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.25))
        precondition(!controller.notchIsVisible && !controller.hasMouseMonitoring && !controller.notchIsAnimating)
        precondition(preferences.set(.menuBar, visible: false))
        precondition(!controller.menuBarIsVisible)
        precondition(!EntryPreferenceStore(domain: domain).read(.menuBar))
        precondition(!EntryPreferenceStore(domain: domain).read(.notch))
        _ = controller.applicationShouldHandleReopen(NSApp, hasVisibleWindows: false)
        precondition(settingsOpened == 1, "Reopening must recover Settings when all entries are hidden")
        precondition(preferences.set(.notch, visible: true))
        precondition(controller.hasMouseMonitoring, "Notch hover must restart after re-enabling")
        controller.quit()
        print("Entry preferences: independent defaults/persistence, hidden-notch wake/display/space lifecycle, re-enable and reopen recovery passed")
    }
}
