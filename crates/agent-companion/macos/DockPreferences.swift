// SPDX-License-Identifier: GPL-3.0-only
import AppKit

/// A fixed domain keeps the app bundle, CLI editor and notch in sync without
/// putting Agent Companion preferences into Codex's config.toml.
struct DockPreferenceStore {
    let domain: String
    private let key = "showDockIcon" as CFString

    init(domain: String = "com.wxgopher.agent-companion") { self.domain = domain }

    func read() -> Bool {
        CFPreferencesAppSynchronize(domain as CFString)
        return CFPreferencesCopyAppValue(key, domain as CFString) as? Bool ?? true
    }

    func write(_ visible: Bool) -> Bool {
        let previous = CFPreferencesCopyAppValue(key, domain as CFString)
        CFPreferencesSetAppValue(key, visible ? kCFBooleanTrue : kCFBooleanFalse, domain as CFString)
        guard CFPreferencesAppSynchronize(domain as CFString) else {
            CFPreferencesSetAppValue(key, previous, domain as CFString)
            CFPreferencesAppSynchronize(domain as CFString)
            return false
        }
        return true
    }
}

final class DockPreferences {
    static let shared = DockPreferences()
    static let editorParentKey = "AGENT_COMPANION_NOTCH_PID"
    private let store: DockPreferenceStore
    private var changed: Notification.Name { Notification.Name("\(store.domain).dockPreferenceChanged") }
    private var parent: NSRunningApplication?
    private var parentPID: Int32?
    private var observers: [NSObjectProtocol] = []

    init(store: DockPreferenceStore = DockPreferenceStore()) { self.store = store }

    deinit {
        for observer in observers {
            DistributedNotificationCenter.default().removeObserver(observer)
            NSWorkspace.shared.notificationCenter.removeObserver(observer)
        }
    }

    var visible: Bool { store.read() }
    var ownsIcon: Bool { parentPID == nil || parent?.isTerminated == true }
    var showsIcon: Bool { visible && ownsIcon }

    func configureEditor() {
        if let text = ProcessInfo.processInfo.environment[Self.editorParentKey],
           let pid = Int32(text), pid > 0, pid != ProcessInfo.processInfo.processIdentifier {
            parentPID = pid
        }
    }

    func start() {
        if let pid = parentPID {
            parent = NSRunningApplication(processIdentifier: pid)
            if parent?.executableURL != Bundle.main.executableURL {
                parent = nil
                parentPID = nil
            }
        }
        if observers.isEmpty {
            observers.append(DistributedNotificationCenter.default().addObserver(forName: changed, object: nil, queue: .main) { [weak self] _ in
                self?.apply()
            })
            observers.append(NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didTerminateApplicationNotification, object: nil, queue: .main) { [weak self] _ in
                guard let self, let parent, parent.isTerminated else { return }
                // A settings window left open after quitting the notch can
                // own the optional icon until that window is closed too.
                self.parent = nil
                self.parentPID = nil
                apply()
            })
        }
        apply()
    }

    @discardableResult func setVisible(_ visible: Bool) -> Bool {
        guard store.write(visible) else { return false }
        apply()
        DistributedNotificationCenter.default().postNotificationName(changed, object: nil, userInfo: nil, deliverImmediately: true)
        return true
    }

    private func apply() {
        let app = NSApplication.shared
        let policy: NSApplication.ActivationPolicy = showsIcon ? .regular : .accessory
        guard app.activationPolicy() != policy else { return }
        let keyWindow = app.keyWindow
        let wasActive = app.isActive
        app.setActivationPolicy(policy)
        // Changing policy must not dismiss settings or lose keyboard input.
        if wasActive, let keyWindow {
            app.activate(ignoringOtherApps: true)
            keyWindow.makeKeyAndOrderFront(nil)
        }
    }
}

@_cdecl("agent_companion_prepare_editor")
public func prepareAgentCompanionEditor() -> Bool {
    // Install workspace observers once Slint's AppKit event loop is ready.
    DockPreferences.shared.configureEditor()
    return DockPreferences.shared.showsIcon
}

@_cdecl("agent_companion_start_editor_preferences")
public func startAgentCompanionEditorPreferences() { DockPreferences.shared.start() }

@_cdecl("agent_companion_dock_visible")
public func agentCompanionDockVisible() -> Bool { DockPreferences.shared.visible }

@_cdecl("agent_companion_set_dock_visible")
public func setAgentCompanionDockVisible(_ visible: Bool) -> Bool {
    DockPreferences.shared.setVisible(visible)
}
