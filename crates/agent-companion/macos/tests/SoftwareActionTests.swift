// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum SoftwareActionTests {
    static func run() {
        verifyNotificationBridge()
        verifyEditorReuse()
        verifyContextMenu()
        print("Software actions: right-click menu, unchanged left-click popup, editor launch/reuse, visible failures and PID-scoped main-thread delivery passed")
    }

    private static func verifyNotificationBridge() {
        FixtureSoftwareActionRequests.actions.removeAll()
        let center = DistributedNotificationCenter.default()
        let pid = ProcessInfo.processInfo.processIdentifier
        var readyCount = 0
        let readyObserver = center.addObserver(forName: SoftwareAction.readyNotification, object: String(pid), queue: .main) { _ in
            readyCount += 1
        }
        defer { center.removeObserver(readyObserver) }
        listenForAgentCompanionSoftwareUpdates()
        listenForAgentCompanionSoftwareUpdates()
        center.postNotificationName(SoftwareAction.notification, object: "\(pid)-other-editor",
                                    userInfo: ["action": "update-codex"], deliverImmediately: true)
        for payload: [String: Any] in [["action": "unknown"], ["action": 123], ["target": "update-codex"]] {
            center.postNotificationName(SoftwareAction.notification, object: String(pid),
                                        userInfo: payload, deliverImmediately: true)
        }
        SoftwareAction.updateCodex.send(to: pid)
        wait { !FixtureSoftwareActionRequests.actions.isEmpty && readyCount > 0 }
        precondition(readyCount == 1, "Repeated listener registration sent duplicate readiness signals")
        precondition(FixtureSoftwareActionRequests.actions == ["update-codex"],
                     "Another PID, a malformed action or duplicate registration reached the editor")
        SoftwareAction.updateDodex.send(to: pid)
        wait { FixtureSoftwareActionRequests.actions.count == 2 }
        precondition(FixtureSoftwareActionRequests.actions == ["update-codex", "update-dodex"])
    }

    private static func verifyEditorReuse() {
        let fixture = EditorFixture()
        let launcher = fixture.launcher()
        var results: [String?] = []
        launcher.open { results.append($0) }
        launcher.open(action: .updateCodex) { results.append($0) }
        launcher.open(action: .updateDodex) { results.append($0) }
        precondition(fixture.arguments == [["codex-tui"]] && results.count == 1
                     && results[0]?.contains("already waiting") == true,
                     "Requests during launch spawned extra editors or silently queued repeated operations")
        fixture.finishLaunching()
        precondition(fixture.sent.isEmpty && results.count == 2,
                     "Launch completion sent a notification before the editor installed its listener")
        launcher.open(action: .updateCodex) { results.append($0) }
        precondition(fixture.sent.isEmpty, "A reused editor received an action before it was ready")
        fixture.becomeReady(pid: fixture.pid + 1)
        precondition(fixture.sent.isEmpty, "Another editor's readiness released the pending action")
        fixture.becomeReady()
        precondition(fixture.sent == [.updateCodex] && fixture.activations == 2)
        precondition(results.count == 4 && results[1] == nil && results[3] == nil
                     && results[2]?.contains("already waiting") == true,
                     "Only one pending action may be dispatched after readiness")
        launcher.open(action: .updateCodex) { results.append($0) }
        launcher.open { results.append($0) }
        precondition(fixture.arguments.count == 1 && fixture.activations == 4,
                     "A running editor was not reused by both Settings and software actions")
        precondition(fixture.sent == [.updateCodex, .updateCodex] && results.count == 6)

        fixture.running = false
        launcher.open(action: .updateDodex) { results.append($0) }
        launcher.open(action: .updateCodex) { results.append($0) }
        precondition(fixture.arguments.last == ["codex-tui", "--software-action", "update-dodex"])
        fixture.fail()
        precondition(results.count == 8 && results.suffix(2).allSatisfy { $0 == EditorFixture.errorText },
                     "A launch failure did not reach every waiting request")
        let sentBeforeRetry = fixture.sent
        launcher.open(action: .updateCodex) { results.append($0) }
        // Launch Services may report completion after the ready notification.
        fixture.becomeReady()
        fixture.finishLaunching()
        precondition(fixture.arguments.last == ["codex-tui", "--software-action", "update-codex"])
        precondition(fixture.arguments.count == 3 && fixture.sent == sentBeforeRetry && results.last! == nil,
                     "Retry replayed a failed request or duplicated the action already supplied on the command line")
        launcher.open(action: .updateDodex) { results.append($0) }
        precondition(fixture.sent.last == .updateDodex, "Readiness arriving before the launch callback was lost")

        let timeoutFixture = EditorFixture()
        let timeoutLauncher = timeoutFixture.launcher(readinessTimeout: 0.02)
        var timeoutError: String?
        timeoutLauncher.open { precondition($0 == nil) }
        timeoutFixture.finishLaunching()
        timeoutLauncher.open(action: .updateCodex) { timeoutError = $0 }
        wait { timeoutError != nil }
        precondition(timeoutFixture.sent.isEmpty && timeoutError!.contains("did not become ready"),
                     "An editor that never becomes ready silently lost its pending request")
        timeoutFixture.becomeReady()
        precondition(timeoutFixture.sent.isEmpty, "A timed-out request was replayed after its visible failure")
    }

    private static func verifyContextMenu() {
        let name = "SoftwareActions-\(UUID().uuidString)"
        defer {
            for key in ["NSStatusItem Visible \(name)", "NSStatusItem Preferred Position \(name)"] {
                UserDefaults.standard.removeObject(forKey: key)
            }
        }
        let fixture = EditorFixture(), usage = FixtureUsageBridge(), updates = FixtureAppUpdateBridge()
        let model = CompanionModel(usageBridge: FixtureUsageBridge(), editorLauncher: fixture.launcher())
        let popup = CompanionModel(usageBridge: usage, updateBridge: updates)
        var presented: [NSMenu] = [], errors: [String] = []
        let controller = MenuBarController(model: model, menuModel: popup,
            themePreferences: ThemeTests.darkPreferences, menuBarPlacement: MenuBarPlacement(autosaveName: name),
            usageBridge: usage, presentContextMenu: { menu, _ in presented.append(menu) },
            reportSoftwareActionError: { errors.append($0) })
        controller.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        model.stop()
        defer { controller.quit() }
        NSApp.activate(ignoringOtherApps: true)
        RunLoop.main.run(until: Date().addingTimeInterval(0.2))

        let mask = controller.menuBarButton!.sendAction(on: [.leftMouseUp])
        controller.menuBarButton!.sendAction(on: NSEvent.EventTypeMask(rawValue: UInt64(mask)))
        let clicks = NSEvent.EventTypeMask.leftMouseUp.union(.rightMouseUp).rawValue
        precondition(UInt64(mask) & clicks == clicks, "The native status button does not receive both mouse buttons")
        ThemeTests.openMenuPanel(controller) { controller.handleMenuBarClick(mouse(.leftMouseUp)) }
        popup.stop()
        precondition(presented.isEmpty && updates.panelOpens == 1)
        controller.handleMenuBarClick(mouse(.rightMouseUp))
        wait { !controller.menuPanelIsVisible && !popup.isPresented }
        precondition(presented.count == 1 && fixture.arguments.isEmpty && updates.panelOpens == 1,
                     "Opening the context menu launched an editor or requested another release check")
        let menu = presented[0]
        precondition(menu.items.filter { !$0.isSeparatorItem }.map(\.title)
                     == ["更新 Codex TUI", "更新 Dodex TUI", "Exit"])
        menu.performActionForItem(at: 0)
        precondition(fixture.arguments == [["codex-tui", "--software-action", "update-codex"]])
        precondition(!controller.menuPanelIsVisible, "An action left the task popup open")
        fixture.finishLaunching()
        fixture.becomeReady()
        menu.performActionForItem(at: 1)
        precondition(fixture.arguments.count == 1 && fixture.sent == [.updateDodex] && fixture.activations == 2)

        fixture.running = false
        menu.performActionForItem(at: 1)
        fixture.fail()
        precondition(errors.count == 1 && errors[0].contains("更新 Dodex TUI")
                     && errors[0].contains(EditorFixture.errorText), "A failed action left no visible error")
        controller.handleMenuBarClick(mouse(.leftMouseUp, modifiers: .control))
        precondition(presented.count == 2 && !controller.menuPanelIsVisible)
        ThemeTests.openMenuPanel(controller) { controller.menuBarButton?.performClick(nil) }
        popup.stop()
        precondition(presented.count == 2 && controller.menuPanelIsVisible && updates.panelOpens == 2,
                     "Showing the context menu replaced the status button's normal popup action")
        menu.performActionForItem(at: menu.numberOfItems - 1)
        precondition(!controller.menuBarIsVisible && !controller.menuBarIsAnimating,
                     "Exit did not use the existing clean shutdown path")
    }

    private static func mouse(_ type: NSEvent.EventType, modifiers: NSEvent.ModifierFlags = []) -> NSEvent {
        NSEvent.mouseEvent(with: type, location: .zero, modifierFlags: modifiers, timestamp: 0,
                          windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 0)!
    }

    private static func wait(_ condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(2)
        while !condition() && Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.02)) }
        precondition(condition(), "The native software-action event did not complete within two seconds")
    }

    private final class EditorFixture {
        static let errorText = "Synthetic editor launch failure"
        let pid: Int32 = 321
        var running = false
        var activations = 0
        var arguments: [[String]] = []
        var sent: [SoftwareAction] = []
        private let readyCenter = NotificationCenter()
        private var pending: ((Result<CompanionEditorLauncher.Editor, Error>) -> Void)?

        func launcher(readinessTimeout: TimeInterval = 15) -> CompanionEditorLauncher {
            CompanionEditorLauncher(launch: { arguments, completion in
                precondition(self.pending == nil, "Launching a second editor while the first is opening")
                self.arguments.append(arguments)
                self.pending = completion
            }, send: { action, pid in
                precondition(pid == self.pid && self.running, "A request targeted the wrong or terminated editor")
                self.sent.append(action)
            }, readyCenter: readyCenter, readinessTimeout: readinessTimeout)
        }

        func finishLaunching() {
            running = true
            let completion = pending
            pending = nil
            completion?(.success(CompanionEditorLauncher.Editor(processIdentifier: pid,
                isRunning: { self.running }, activate: { self.activations += 1 })))
        }

        func becomeReady(pid: Int32? = nil) {
            readyCenter.post(name: SoftwareAction.readyNotification, object: String(pid ?? self.pid))
        }

        func fail() {
            let completion = pending
            pending = nil
            completion?(.failure(NSError(domain: "SoftwareActionTests", code: 1,
                userInfo: [NSLocalizedDescriptionKey: Self.errorText])))
        }
    }
}
