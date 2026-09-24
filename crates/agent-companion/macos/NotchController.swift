// SPDX-License-Identifier: GPL-3.0-only
// Window placement/behavior adapted from Open Island's OverlayPanelController.
// See THIRD_PARTY_NOTICES.md for the pinned source and license.
import AppKit
import Combine
import SwiftUI

private let reopenSettingsNotification = Notification.Name("com.wxgopher.agent-companion.reopenSettings")

@_cdecl("agent_companion_reopen_settings")
public func reopenAgentCompanionSettings() {
    DistributedNotificationCenter.default().postNotificationName(reopenSettingsNotification, object: nil, userInfo: nil, deliverImmediately: true)
}

private final class NotchPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

final class NotchController: NSObject, NSApplicationDelegate, NSPopoverDelegate {
    private let model: CompanionModel
    private let menuModel: CompanionModel
    private var statusItem: NSStatusItem?
    private let popover = NSPopover()
    private var notchEnabled = false
    private let entries: EntryPreferences
    private let settingsOverride: (() -> Void)?
    private let clock: () -> Date
    private let usageCoordinator: SubscriptionUsageCoordinator
    init(entries: EntryPreferences = .shared, model: CompanionModel = CompanionModel(), menuModel: CompanionModel = CompanionModel(), settingsOverride: (() -> Void)? = nil,
         clock: @escaping () -> Date = Date.init,
         usageReader: @escaping (SubscriptionSource) -> SubscriptionReading = { _ in CodexSubscriptionReader() }) {
        self.entries = entries
        self.model = model
        self.menuModel = menuModel
        self.settingsOverride = settingsOverride
        self.clock = clock
        usageCoordinator = SubscriptionUsageCoordinator(makeReader: usageReader, clock: clock)
        super.init()
        model.usageCoordinator = usageCoordinator
        menuModel.usageCoordinator = usageCoordinator
        usageCoordinator.onChange = { [weak self] source, value, loading in
            guard let self else { return }
            self.model.receiveUsage(source: source, value: value, loading: loading)
            self.menuModel.receiveUsage(source: source, value: value, loading: loading)
            RunLoop.main.perform(inModes: [.common]) { [weak self] in self?.updateMenuBarUsage() }
        }
    }
    var notchIsVisible: Bool { panel?.isVisible == true }
    var notchIsAnimating: Bool { presentation?.isAnimating == true }
    var hasMouseMonitoring: Bool { globalMonitor != nil || localMonitor != nil }
    var menuBarIsVisible: Bool { statusItem != nil }
    var menuPanelIsVisible: Bool { popover.isShown }
    private(set) var menuBarUsage: MenuBarUsage?
    private var panel: NotchPanel!
    private var presentation: NotchPresentation!
    private var globalMonitor: Any?
    private var localMonitor: Any?
    private var observers: [NSObjectProtocol] = []
    private var changes: AnyCancellable?
    private var menuChanges: AnyCancellable?
    private lazy var hover = NotchHover(
        containsPointer: { [weak self] in
            guard let self, let panel, panel.isVisible, let screen = selectedScreen else { return false }
            return NotchHoverRegion.contains(NSEvent.mouseLocation, panel: panel.frame,
                                             screen: screen.frame, cameraHeight: model.metrics.cameraHeight)
        },
        expand: { [weak self] in self?.expand(activate: false) },
        collapse: { [weak self] in self?.collapse() }
    )
    private var quitting = false
    private var selectedScreen: NSScreen?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let menu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Show Codex tasks", action: #selector(showTasks), keyEquivalent: "0").target = self
        appMenu.addItem(withTitle: "Subscription usage", action: #selector(showUsage), keyEquivalent: "3").target = self
        appMenu.addItem(withTitle: "Settings…", action: #selector(showSettings), keyEquivalent: ",").target = self
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Quit Agent Companion", action: #selector(quit), keyEquivalent: "q").target = self
        appItem.submenu = appMenu
        menu.addItem(appItem)
        NSApp.mainMenu = menu

        panel = NotchPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.title = "Agent Companion — Codex tasks"
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.isFloatingPanel = true
        panel.level = .statusBar
        panel.hidesOnDeactivate = false
        panel.isMovable = false
        panel.isReleasedWhenClosed = false
        panel.acceptsMouseMovedEvents = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        presentation = NotchPresentation(panel: panel, model: model)
        presentation.surface.hover = { [weak self] in self?.hover.update() }
        model.expand = { [weak self] in self?.expand(activate: true) }
        model.collapse = { [weak self] in self?.collapse() }
        model.quit = { [weak self] in self?.quit() }
        menuModel.metrics = NotchMetrics(panelWidth: 356)
        menuModel.expand = { [weak self] in self?.menuModel.expanded = true }
        menuModel.collapse = { [weak self] in self?.popover.performClose(nil) }
        menuModel.quit = { [weak self] in self?.quit() }
        menuModel.settingsAction = { [weak self] in
            self?.model.openSettings()
            self?.popover.performClose(nil)
        }
        popover.behavior = .transient
        popover.delegate = self
        popover.contentSize = NSSize(width: 356, height: 560)
        popover.contentViewController = NSHostingController(rootView:
            CompanionView(model: menuModel, showsDetails: true, drawsBackground: false,
                          detailsHeight: 560 - menuModel.compactHeight)
                .frame(width: 356, height: 560, alignment: .top).background(Color.black))
        // Defer sizing until @Published has committed the new value.
        changes = model.objectWillChange.sink { [weak self] _ in
            DispatchQueue.main.async { self?.layout() }
        }
        menuChanges = Publishers.Merge(model.objectWillChange, menuModel.objectWillChange).sink { [weak self] _ in
            RunLoop.main.perform(inModes: [.common]) { self?.updateMenuBarUsage() }
        }
        observers.append(NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self?.selectScreen() })
        observers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { [weak self] _ in self?.selectScreen() })
        observers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in self?.selectScreen(); self?.model.refresh() })
        DisplayPreferences.shared.start { [weak self] in self?.selectScreen() }
        observers.append(DistributedNotificationCenter.default().addObserver(forName: reopenSettingsNotification, object: nil, queue: .main) { [weak self] _ in self?.showSettings() })
        entries.start { [weak self] in self?.applyEntryPreferences() }
        applyEntryPreferences()
        model.start()
        if !entries.menuBarVisible && !DockPreferences.shared.visible && !notchEnabled {
            showSettings()
        }
    }

    private func startMouseMonitoring() {
        guard globalMonitor == nil, localMonitor == nil else { return }
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDown, .rightMouseDown]) { [weak self] event in
            guard let self else { return }
            if event.type == .mouseMoved { hover.update(); return }
            guard !panel.frame.contains(NSEvent.mouseLocation) else { return }
            collapse()
        }
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDown, .rightMouseDown, .keyDown]) { [weak self] event in
            guard let self else { return event }
            if event.type == .mouseMoved { hover.update(); return event }
            if event.type == .keyDown && event.keyCode == 53 { collapse(); return nil }
            if (event.type == .leftMouseDown || event.type == .rightMouseDown) && event.window === panel {
                hover.pin()
                panel.makeKeyAndOrderFront(nil)
            }
            return event
        }
    }

    private func stopMouseMonitoring() {
        if let globalMonitor { NSEvent.removeMonitor(globalMonitor) }
        if let localMonitor { NSEvent.removeMonitor(localMonitor) }
        globalMonitor = nil
        localMonitor = nil
    }

    private func applyEntryPreferences() {
        if entries.menuBarVisible {
            if statusItem == nil {
                let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
                item.button?.target = self
                item.button?.action = #selector(toggleMenuPanel)
                statusItem = item
                updateMenuBarUsage()
            }
        } else if let item = statusItem {
            popover.performClose(nil)
            NSStatusBar.system.removeStatusItem(item)
            statusItem = nil
            menuBarUsage = nil
            updateMenuBarUsage()
        }
        let enabled = entries.notchVisible
        guard enabled != notchEnabled else { return }
        notchEnabled = enabled
        if enabled {
            selectScreen()
            startMouseMonitoring()
            hover.start()
        } else {
            hover.stop()
            stopMouseMonitoring()
            model.expanded = false
            presentation.stop()
            panel.orderOut(nil)
        }
    }

    private func updateMenuBarUsage() {
        guard !quitting else { return }
        // The always-running primary snapshot alone owns source membership;
        // a closed popup may still have an older snapshot.
        usageCoordinator.synchronize(instances: model.instances,
                                     backgroundEnabled: statusItem != nil && !model.snapshot.loading)
        guard let statusItem else { return }
        usageCoordinator.refreshBackground()
        let value = MenuBarUsage(instances: model.instances, now: clock(), reading: { usageCoordinator.reading(for: $0) })
        guard value != menuBarUsage else { return }
        menuBarUsage = value
        value.apply(to: statusItem)
    }

    @objc func toggleMenuPanel() {
        if popover.isShown { popover.performClose(nil); return }
        guard let button = statusItem?.button else { return }
        menuModel.showTasks()
        menuModel.expanded = true
        menuModel.start()
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
    }

    func popoverDidClose(_ notification: Notification) {
        menuModel.expanded = false
        menuModel.stop()
    }

    private func selectScreen() {
        guard notchEnabled else { panel?.orderOut(nil); return }
        // AppKit's first screen is the configured primary, unlike NSScreen.main
        // which follows keyboard focus. An explicit preference overrides it;
        // an absent display falls back without forgetting the saved identity.
        selectedScreen = DisplayPreferences.shared.selectedScreen()
        guard let screen = selectedScreen else { panel?.orderOut(nil); return }
        model.metrics = NotchMetrics(screen: screen)
        layout(animated: false)
        panel.orderFrontRegardless()
        hover.update()
    }

    private func layout(animated: Bool = true) {
        guard notchEnabled, let screen = selectedScreen else { return }
        presentation.update(screen: screen.frame, availableHeight: screen.frame.maxY - screen.visibleFrame.minY,
                            animated: animated)
    }

    private func expand(activate: Bool) {
        guard notchEnabled else { return }
        if activate { hover.pin() }
        if !model.expanded {
            model.showingCompleted = false
            model.showTasks()
            model.expanded = true
            layout()
        }
        if activate { panel.makeKeyAndOrderFront(nil) }
    }

    private func collapse() {
        model.message = nil
        model.failedTask = nil
        // A jump in progress may complete after dismissal, without stealing focus.
        model.expanded = false
        panel.resignKey()
        layout()
        // Esc or the collapse button must stay dismissed while the pointer is
        // still over the compact strip. Hover is re-armed after it leaves.
        hover.dismiss()
    }

    @objc private func showTasks() {
        if statusItem != nil { if !popover.isShown { toggleMenuPanel() }; menuModel.showTasks() }
        else if notchEnabled { model.showTasks(); expand(activate: true) }
        else { model.openSettings() }
    }
    @objc func showUsage() {
        if statusItem != nil { if !popover.isShown { toggleMenuPanel() }; menuModel.showUsage() }
        else if notchEnabled { model.showUsage() }
        else { model.openSettings() }
    }
    @objc private func showSettings() {
        if let settingsOverride { settingsOverride() }
        else { model.openSettings() }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showSettings()
        return false
    }

    @objc func quit() {
        guard !quitting else { return }
        quitting = true
        presentation.stop()
        hover.stop()
        model.stop()
        menuModel.stop()
        usageCoordinator.stop()
        popover.performClose(nil)
        if let statusItem { NSStatusBar.system.removeStatusItem(statusItem) }
        entries.stop()
        DisplayPreferences.shared.stop()
        changes?.cancel()
        menuChanges?.cancel()
        stopMouseMonitoring()
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
            NSWorkspace.shared.notificationCenter.removeObserver(observer)
            DistributedNotificationCenter.default().removeObserver(observer)
        }
        panel?.orderOut(nil)
        NSApp.stop(nil)
        if let event = NSEvent.otherEvent(with: .applicationDefined, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, subtype: 0, data1: 0, data2: 0) {
            NSApp.postEvent(event, atStart: false)
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        quit()
        // Do not cancel a system logout/shutdown request. Explicit in-app Quit
        // returns through NSApp.run so Rust can join its monitor; system exit
        // also closes the process-owned lock and its read-only worker.
        return .terminateNow
    }
}

@_cdecl("agent_companion_run_notch")
public func runAgentCompanionNotch() -> Int32 {
    let app = NSApplication.shared
    DockPreferences.shared.start()
    let delegate = NotchController()
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
    return 0
}
