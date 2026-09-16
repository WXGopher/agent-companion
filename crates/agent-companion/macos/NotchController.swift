// SPDX-License-Identifier: GPL-3.0-only
// Window placement/behavior adapted from Open Island's OverlayPanelController.
// See THIRD_PARTY_NOTICES.md for the pinned source and license.
import AppKit
import Combine
import SwiftUI

private final class NotchPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

private final class NotchHostingView: NSHostingView<CompanionView> {
    var hover: ((Bool) -> Void)?
    private var tracking: NSTrackingArea?
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
        super.updateTrackingAreas()
    }
    override func mouseEntered(with event: NSEvent) { hover?(true) }
    override func mouseExited(with event: NSEvent) { hover?(false) }
}

final class NotchController: NSObject, NSApplicationDelegate {
    private let model = CompanionModel()
    private var panel: NotchPanel!
    private var host: NotchHostingView!
    private var globalMonitor: Any?
    private var localMonitor: Any?
    private var observers: [NSObjectProtocol] = []
    private var changes: AnyCancellable?
    private var hoverWork: DispatchWorkItem?
    private var pinned = false
    private var hoverDismissed = false
    private var quitting = false
    private var selectedScreen: NSScreen?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let menu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Show Codex tasks", action: #selector(showTasks), keyEquivalent: "0").target = self
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
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        host = NotchHostingView(rootView: CompanionView(model: model))
        host.sizingOptions = [.intrinsicContentSize]
        host.hover = { [weak self] inside in self?.hover(inside) }
        panel.contentView = host
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
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
            guard let self, !panel.frame.contains(NSEvent.mouseLocation) else { return }
            collapse()
        }
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .keyDown]) { [weak self] event in
            guard let self else { return event }
            if event.type == .keyDown && event.keyCode == 53 { collapse(); return nil }
            if (event.type == .leftMouseDown || event.type == .rightMouseDown) && event.window === panel {
                hoverWork?.cancel()
                pinned = true
                panel.makeKeyAndOrderFront(nil)
            }
            return event
        }
        selectScreen()
        panel.orderFrontRegardless()
        model.start()
    }

    private func selectScreen() {
        selectedScreen = NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 }) ?? NSScreen.main ?? NSScreen.screens.first
        guard let screen = selectedScreen else { panel?.orderOut(nil); return }
        let cameraHeight = screen.safeAreaInsets.top
        if cameraHeight > 0, let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea {
            model.cameraWidth = max(0, screen.frame.width - left.width - right.width) + 4
            model.compactHeight = cameraHeight
        } else {
            model.cameraWidth = 0
            model.compactHeight = 28
        }
        layout()
        panel.orderFrontRegardless()
    }

    private func layout() {
        guard let screen = selectedScreen, let panel, let host else { return }
        let width = model.compactWidth
        // Intrinsic size follows rows/messages. Never leave an invisible expanded
        // window over the user's apps after collapse.
        let height = model.expanded ? max(model.compactHeight, host.fittingSize.height) : model.compactHeight
        let top = model.hasCamera ? screen.frame.maxY : screen.visibleFrame.maxY - 4
        let frame = NSRect(x: screen.frame.midX - width / 2, y: top - height, width: width, height: height)
        if panel.frame != frame { panel.setFrame(frame, display: true) }
    }

    private func hover(_ inside: Bool) {
        hoverWork?.cancel()
        let pointerInside = panel.frame.contains(NSEvent.mouseLocation)
        if !pointerInside { hoverDismissed = false }
        // Replacing tracking areas while resizing can produce stale enter/exit
        // events. Check the pointer again before either opening or dismissing.
        guard !pinned, inside == pointerInside, !inside || !hoverDismissed else { return }
        let work = DispatchWorkItem { [weak self] in
            guard let self, !pinned, panel.frame.contains(NSEvent.mouseLocation) == inside else { return }
            if inside { expand(activate: false) } else { collapse() }
        }
        hoverWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (inside ? 0.22 : 0.40), execute: work)
    }

    private func expand(activate: Bool) {
        hoverWork?.cancel()
        hoverDismissed = false
        if !model.expanded { model.showingCompleted = false }
        pinned = pinned || activate
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            model.expanded = true
        } else {
            withAnimation(.easeOut(duration: 0.18)) { model.expanded = true }
        }
        layout()
        if activate { panel.makeKeyAndOrderFront(nil) }
    }

    private func collapse() {
        hoverWork?.cancel()
        pinned = false
        model.message = nil
        model.failedTask = nil
        // A jump in progress may complete after dismissal, without stealing focus.
        model.expanded = false
        panel.resignKey()
        layout()
        // Esc or the collapse button must stay dismissed while the pointer is
        // still over the compact strip. Hover is re-armed after it leaves.
        hoverDismissed = panel.frame.contains(NSEvent.mouseLocation)
    }

    @objc private func showTasks() { expand(activate: true) }
    @objc private func showSettings() { model.openSettings() }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        expand(activate: true)
        return false
    }

    @objc private func quit() {
        guard !quitting else { return }
        quitting = true
        hoverWork?.cancel()
        model.stop()
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
    app.setActivationPolicy(.accessory)
    let delegate = NotchController()
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
    return 0
}
