// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import Combine
import SwiftUI

private let reopenMenuNotification = Notification.Name("com.wxgopher.agent-companion.reopenMenu")

@_cdecl("agent_companion_reopen_menu")
public func reopenAgentCompanionMenu() {
    DistributedNotificationCenter.default().postNotificationName(reopenMenuNotification, object: nil, userInfo: nil, deliverImmediately: true)
}

/// The menu bar is the single persistent macOS entry. Legacy visibility, Dock
/// and display preferences are deliberately not consulted or migrated.
final class MenuBarController: NSObject, NSApplicationDelegate, NSPopoverDelegate {
    private let model: CompanionModel
    private let menuModel: CompanionModel
    private let themePreferences: ThemePreferences
    private let menuBarPlacement: MenuBarPlacement
    private let settingsOverride: (() -> Void)?
    private let presentContextMenu: (NSMenu, NSStatusBarButton) -> Void
    private let reportSoftwareActionError: (String) -> Void
    private let clock: () -> Date
    private let usageCoordinator: SubscriptionUsageCoordinator
    private let menuBarActivity: MenuBarActivityView
    private var statusItem: NSStatusItem?
    private let popover = NSPopover()
    private var observers: [NSObjectProtocol] = []
    private var menuChanges: AnyCancellable?
    private var themeChanges: AnyCancellable?
    private var quitting = false
    private var pendingUpdateCheck = false
    private lazy var contextMenu: NSMenu = {
        let menu = NSMenu()
        for action in SoftwareAction.allCases {
            let item = menu.addItem(withTitle: action.title, action: #selector(performSoftwareAction(_:)), keyEquivalent: "")
            item.representedObject = action.rawValue
            item.target = self
        }
        menu.addItem(.separator())
        menu.addItem(withTitle: "Exit", action: #selector(quit), keyEquivalent: "").target = self
        return menu
    }()

    init(model: CompanionModel = CompanionModel(), menuModel: CompanionModel = CompanionModel(),
         themePreferences: ThemePreferences = .shared,
         menuBarPlacement: MenuBarPlacement = MenuBarPlacement(), settingsOverride: (() -> Void)? = nil,
         clock: @escaping () -> Date = Date.init,
         menuBarReducedMotion: @escaping () -> Bool = { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion },
         usageBridge: SubscriptionUsageBridging = RustSubscriptionUsageBridge(),
         presentContextMenu: @escaping (NSMenu, NSStatusBarButton) -> Void = { menu, button in
             menu.popUp(positioning: nil, at: NSPoint(x: button.bounds.minX, y: button.bounds.minY), in: button)
         },
         reportSoftwareActionError: @escaping (String) -> Void = { message in
             let alert = NSAlert()
             alert.alertStyle = .warning
             alert.messageText = "无法执行版本操作"
             alert.informativeText = message
             alert.addButton(withTitle: "好")
             NSApp.activate(ignoringOtherApps: true)
             alert.runModal()
         }) {
        self.model = model
        self.menuModel = menuModel
        self.themePreferences = themePreferences
        self.menuBarPlacement = menuBarPlacement
        self.settingsOverride = settingsOverride
        self.presentContextMenu = presentContextMenu
        self.reportSoftwareActionError = reportSoftwareActionError
        self.clock = clock
        menuBarActivity = MenuBarActivityView(reduceMotion: menuBarReducedMotion)
        usageCoordinator = SubscriptionUsageCoordinator(bridge: usageBridge)
        super.init()
        model.usageCoordinator = usageCoordinator
        menuModel.usageCoordinator = usageCoordinator
        usageCoordinator.onChange = { [weak self] in
            guard let self else { return }
            self.model.receiveUsageSnapshot()
            self.menuModel.receiveUsageSnapshot()
            RunLoop.main.perform(inModes: [.common]) { [weak self] in self?.updateMenuBarUsage() }
        }
    }

    var menuBarIsVisible: Bool { statusItem?.isVisible == true }
    var menuPanelIsVisible: Bool { popover.isShown }
    var menuPanelContentView: NSView? { popover.contentViewController?.view }
    var menuPanelAppearance: NSAppearance? { popover.appearance }
    var menuBarButton: NSStatusBarButton? { statusItem?.button }
    var menuBarActivityView: MenuBarActivityView { menuBarActivity }
    var menuBarIsAnimating: Bool { menuBarActivity.isAnimating }
    private(set) var menuBarUsage: MenuBarUsage?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        let menu = NSMenu(), appItem = NSMenuItem(), appMenu = NSMenu()
        appMenu.addItem(withTitle: "Show Codex tasks", action: #selector(showTasks), keyEquivalent: "0").target = self
        appMenu.addItem(withTitle: "Subscription usage", action: #selector(showUsage), keyEquivalent: "3").target = self
        appMenu.addItem(withTitle: "Settings…", action: #selector(showSettings), keyEquivalent: ",").target = self
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Quit Agent Companion", action: #selector(quit), keyEquivalent: "q").target = self
        appItem.submenu = appMenu
        menu.addItem(appItem)
        NSApp.mainMenu = menu

        menuModel.present = { [weak self] in self?.menuModel.isPresented = true }
        menuModel.dismiss = { [weak self] in self?.popover.performClose(nil) }
        menuModel.quit = { [weak self] in self?.quit() }
        menuModel.settingsAction = { [weak self] in self?.showSettings() }
        popover.behavior = .transient
        popover.delegate = self
        popover.contentSize = NSSize(width: CompanionPopupLayout.width, height: CompanionPopupLayout.height)
        popover.contentViewController = NSHostingController(rootView:
            CompanionPopupView(model: menuModel, themePreferences: themePreferences))
        themeChanges = themePreferences.$theme.sink { [weak self] theme in
            self?.popover.appearance = theme.appearance
        }
        menuChanges = Publishers.Merge(model.objectWillChange, menuModel.objectWillChange).sink { [weak self] _ in
            RunLoop.main.perform(inModes: [.common]) { self?.updateMenuBarUsage() }
        }
        observers.append(NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.model.refresh() })
        observers.append(DistributedNotificationCenter.default().addObserver(
            forName: reopenMenuNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.showTasks() })
        let item = menuBarPlacement.makeItem()
        item.button?.target = self
        item.button?.action = #selector(menuBarClicked)
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        statusItem = item
        updateMenuBarUsage()
        model.start()
    }

    private func updateMenuBarUsage() {
        guard !quitting, let statusItem else { return }
        // Rendering reads the shared state without scheduling any requests.
        let value = MenuBarUsage(instances: model.instances, tasks: model.snapshot.tasks,
                                 now: clock(), reading: { usageCoordinator.reading(for: $0) })
        menuBarActivity.refreshAnimation()
        guard value != menuBarUsage else { return }
        menuBarUsage = value
        value.apply(to: statusItem, indicator: menuBarActivity)
    }

    @objc private func menuBarClicked() { handleMenuBarClick(NSApp.currentEvent) }

    func handleMenuBarClick(_ event: NSEvent?) {
        if event?.type == .rightMouseUp || (event?.type == .leftMouseUp && event?.modifierFlags.contains(.control) == true) {
            guard !quitting, let button = statusItem?.button else { return }
            popover.performClose(nil)
            presentContextMenu(contextMenu, button)
        } else { toggleMenuPanel() }
    }

    @objc private func performSoftwareAction(_ sender: NSMenuItem) {
        guard !quitting, let value = sender.representedObject as? String,
              let action = SoftwareAction(rawValue: value) else { return }
        popover.performClose(nil)
        model.openSoftwareAction(action) { [weak self] error in
            guard let self, !self.quitting, let error else { return }
            self.reportSoftwareActionError("\(action.title)：\(error)")
        }
    }

    @objc func toggleMenuPanel() {
        if popover.isShown { popover.performClose(nil); return }
        guard !quitting, let button = statusItem?.button else { return }
        usageCoordinator.panelOpened()
        menuModel.showTasks()
        menuModel.isPresented = true
        menuModel.start()
        pendingUpdateCheck = true
        // Focus the transient popover so returning to another app dismisses it.
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }

    func popoverDidShow(_ notification: Notification) {
        menuModel.animatesTaskActivity = true
        guard pendingUpdateCheck, popover.isShown, menuModel.isPresented, !quitting else { return }
        pendingUpdateCheck = false
        menuModel.panelOpenedForUpdates()
    }

    func popoverDidClose(_ notification: Notification) {
        pendingUpdateCheck = false
        menuModel.animatesTaskActivity = false
        menuModel.isPresented = false
        menuModel.message = nil
        menuModel.failedTask = nil
        menuModel.stop()
    }

    @objc func showTasks() {
        NSApp.activate(ignoringOtherApps: true)
        if !popover.isShown { toggleMenuPanel() }
        menuModel.showTasks()
    }

    @objc func showUsage() {
        if !popover.isShown { toggleMenuPanel() }
        menuModel.showUsage()
    }

    @objc func showSettings() {
        if let settingsOverride { settingsOverride() }
        else { model.openSettings() }
        popover.performClose(nil)
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showTasks()
        return false
    }

    @objc func quit() {
        guard !quitting else { return }
        quitting = true
        menuBarActivity.stop()
        menuModel.animatesTaskActivity = false
        model.stop()
        menuModel.stop()
        popover.performClose(nil)
        if let statusItem { menuBarPlacement.remove(statusItem) }
        statusItem = nil
        menuBarUsage = nil
        menuChanges?.cancel()
        themeChanges?.cancel()
        for observer in observers {
            NSWorkspace.shared.notificationCenter.removeObserver(observer)
            DistributedNotificationCenter.default().removeObserver(observer)
        }
        observers.removeAll()
        NSApp.stop(nil)
        if let event = NSEvent.otherEvent(with: .applicationDefined, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, subtype: 0, data1: 0, data2: 0) {
            NSApp.postEvent(event, atStart: false)
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        quit()
        return .terminateNow
    }
}

@_cdecl("agent_companion_run_menu_bar")
public func runAgentCompanionMenuBar() -> Int32 {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let delegate = MenuBarController()
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
    return 0
}
