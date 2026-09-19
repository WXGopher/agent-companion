// SPDX-License-Identifier: GPL-3.0-only
import AppKit

enum CompanionEntry: String {
    case menuBar = "showMenuBarIcon"
    case notch = "showNotch"
    var defaultValue: Bool { self == .menuBar }
}

struct EntryPreferenceStore {
    let domain: String
    init(domain: String = "com.wxgopher.agent-companion") { self.domain = domain }

    func read(_ entry: CompanionEntry) -> Bool {
        CFPreferencesAppSynchronize(domain as CFString)
        return CFPreferencesCopyAppValue(entry.rawValue as CFString, domain as CFString) as? Bool ?? entry.defaultValue
    }

    func write(_ entry: CompanionEntry, _ visible: Bool) -> Bool {
        let key = entry.rawValue as CFString
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

final class EntryPreferences {
    static let shared = EntryPreferences()
    private let store: EntryPreferenceStore
    private var observer: NSObjectProtocol?
    private var onChange: (() -> Void)?
    private var changed: Notification.Name { Notification.Name("\(store.domain).entryPreferenceChanged") }
    init(store: EntryPreferenceStore = EntryPreferenceStore()) { self.store = store }
    deinit { stop() }
    var menuBarVisible: Bool { store.read(.menuBar) }
    var notchVisible: Bool { store.read(.notch) }

    func start(_ onChange: @escaping () -> Void) {
        self.onChange = onChange
        guard observer == nil else { return }
        observer = DistributedNotificationCenter.default().addObserver(forName: changed, object: nil, queue: .main) { [weak self] _ in self?.onChange?() }
    }

    func stop() {
        if let observer { DistributedNotificationCenter.default().removeObserver(observer) }
        observer = nil
        onChange = nil
    }

    func set(_ entry: CompanionEntry, visible: Bool) -> Bool {
        guard store.write(entry, visible) else { return false }
        onChange?()
        DistributedNotificationCenter.default().postNotificationName(changed, object: nil, userInfo: nil, deliverImmediately: true)
        return true
    }
}

@_cdecl("agent_companion_menu_bar_visible")
public func agentCompanionMenuBarVisible() -> Bool { EntryPreferences.shared.menuBarVisible }
@_cdecl("agent_companion_notch_visible")
public func agentCompanionNotchVisible() -> Bool { EntryPreferences.shared.notchVisible }
@_cdecl("agent_companion_set_menu_bar_visible")
public func setAgentCompanionMenuBarVisible(_ visible: Bool) -> Bool { EntryPreferences.shared.set(.menuBar, visible: visible) }
@_cdecl("agent_companion_set_notch_visible")
public func setAgentCompanionNotchVisible(_ visible: Bool) -> Bool { EntryPreferences.shared.set(.notch, visible: visible) }
