// SPDX-License-Identifier: GPL-3.0-only
// Window placement/behavior adapted from Open Island's OverlayPanelController.
// See THIRD_PARTY_NOTICES.md for the pinned source and license.
import AppKit
import Combine

private final class NotchPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

final class NotchController: NSObject, NSApplicationDelegate {
    private let model = CompanionModel()
    private var panel: NotchPanel!
    private var presentation: NotchPresentation!
    private var globalMonitor: Any?
    private var localMonitor: Any?
    private var observers: [NSObjectProtocol] = []
    private var changes: AnyCancellable?
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
        // Defer sizing until @Published has committed the new value.
        changes = model.objectWillChange.sink { [weak self] _ in
            DispatchQueue.main.async { self?.layout() }
        }
        observers.append(NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self?.selectScreen() })
        observers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { [weak self] _ in self?.selectScreen() })
        observers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in self?.selectScreen(); self?.model.refresh() })
        DisplayPreferences.shared.start { [weak self] in self?.selectScreen() }
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
        selectScreen()
        panel.orderFrontRegardless()
        model.start()
        hover.start()
    }

    private func selectScreen() {
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
        guard let screen = selectedScreen else { return }
        presentation.update(screen: screen.frame, availableHeight: screen.frame.maxY - screen.visibleFrame.minY,
                            animated: animated)
    }

    private func expand(activate: Bool) {
        if activate { hover.pin() }
        if !model.expanded { model.showingCompleted = false; model.showTasks() }
        model.expanded = true
        layout()
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

    @objc private func showTasks() { model.showTasks(); expand(activate: true) }
    @objc private func showUsage() { model.showUsage() }
    @objc private func showSettings() { model.openSettings() }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        expand(activate: true)
        return false
    }

    @objc private func quit() {
        guard !quitting else { return }
        quitting = true
        presentation.stop()
        hover.stop()
        model.stop()
        DisplayPreferences.shared.stop()
        changes?.cancel()
        if let globalMonitor { NSEvent.removeMonitor(globalMonitor) }
        if let localMonitor { NSEvent.removeMonitor(localMonitor) }
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
            NSWorkspace.shared.notificationCenter.removeObserver(observer)
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
