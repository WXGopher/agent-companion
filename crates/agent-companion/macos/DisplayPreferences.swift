// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import ColorSync

struct CompanionDisplay: Equatable {
    let id: String
    let name: String

    init(id: String, name: String) { self.id = id; self.name = name }

    init?(screen: NSScreen) {
        guard let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber,
              let uuid = CGDisplayCreateUUIDFromDisplayID(number.uint32Value)?.takeRetainedValue() else { return nil }
        id = (CFUUIDCreateString(nil, uuid) as String).lowercased()
        name = screen.localizedName
    }

    static func selectedIndex(preferredID: String?, identifiers: [String?]) -> Int? {
        guard !identifiers.isEmpty else { return nil }
        // A disconnected preference stays saved. Reconnecting selects it again.
        return preferredID.flatMap { preferred in identifiers.firstIndex { $0 == preferred } } ?? 0
    }
}

struct DisplayPreferenceStore {
    let domain: String
    static let key = "notchDisplay" as CFString

    init(domain: String = "com.wxgopher.agent-companion") { self.domain = domain }

    func read() -> CompanionDisplay? {
        CFPreferencesAppSynchronize(domain as CFString)
        guard let value = CFPreferencesCopyAppValue(Self.key, domain as CFString) as? [String: String],
              let id = value["id"], UUID(uuidString: id) != nil,
              let name = value["name"] else { return nil }
        return CompanionDisplay(id: id.lowercased(), name: name)
    }

    func write(_ display: CompanionDisplay?) -> Bool {
        CFPreferencesAppSynchronize(domain as CFString)
        let previous = CFPreferencesCopyAppValue(Self.key, domain as CFString)
        let value = display.map { ["id": $0.id, "name": $0.name] as CFDictionary }
        CFPreferencesSetAppValue(Self.key, value, domain as CFString)
        guard CFPreferencesAppSynchronize(domain as CFString) else {
            CFPreferencesSetAppValue(Self.key, previous, domain as CFString)
            CFPreferencesAppSynchronize(domain as CFString)
            return false
        }
        return true
    }
}

struct DisplaySettingsSnapshot: Encodable, Equatable {
    struct Option: Encodable, Equatable {
        let id: String
        let label: String
    }
    let options: [Option]
    let selectedId: String
    let status: String
}

final class DisplayPreferences {
    static let shared = DisplayPreferences()
    private let store: DisplayPreferenceStore
    private var observer: NSObjectProtocol?
    var notificationName: Notification.Name { Notification.Name("\(store.domain).displayPreferenceChanged") }

    init(store: DisplayPreferenceStore = DisplayPreferenceStore()) { self.store = store }
    deinit { stop() }

    func start(onChange: @escaping () -> Void) {
        guard observer == nil else { return }
        observer = DistributedNotificationCenter.default().addObserver(forName: notificationName, object: nil, queue: .main) { _ in
            onChange()
        }
    }

    func stop() {
        if let observer { DistributedNotificationCenter.default().removeObserver(observer) }
        observer = nil
    }

    func selectedScreen(in screens: [NSScreen] = NSScreen.screens) -> NSScreen? {
        let identifiers = screens.map { CompanionDisplay(screen: $0)?.id }
        guard let index = CompanionDisplay.selectedIndex(preferredID: store.read()?.id, identifiers: identifiers) else { return nil }
        return screens[index]
    }

    func snapshot(displays: [CompanionDisplay] = NSScreen.screens.compactMap(CompanionDisplay.init(screen:))) -> DisplaySettingsSnapshot {
        let selected = store.read()
        var options = [DisplaySettingsSnapshot.Option(id: "", label: "Follow primary display")]
        for (index, display) in displays.enumerated() {
            // A short stable suffix distinguishes identical model names.
            let peers = displays.filter { $0.name == display.name && $0.id != display.id }
            let length = [4, 8, 12, 36].first { length in
                peers.allSatisfy { $0.id.suffix(length) != display.id.suffix(length) }
            } ?? 36
            let label = display.name + (!peers.isEmpty ? " · \(display.id.suffix(length))" : "") + (index == 0 ? " (Primary)" : "")
            options.append(.init(id: display.id, label: label))
        }
        let missing = selected.map { target in !displays.contains { $0.id == target.id } } ?? false
        if let selected, missing {
            options.append(.init(id: selected.id, label: "\(selected.name) (Disconnected)"))
        }
        let status: String
        if missing {
            status = "Display disconnected. Using the primary display until it returns."
        } else if selected == nil {
            status = "Follows the primary display set in macOS. Changes save automatically."
        } else {
            status = "Stays on this display. Changes save automatically."
        }
        return DisplaySettingsSnapshot(options: options, selectedId: selected?.id ?? "", status: status)
    }

    @discardableResult func select(_ id: String, displays: [CompanionDisplay] = NSScreen.screens.compactMap(CompanionDisplay.init(screen:))) -> Bool {
        let selection: CompanionDisplay?
        if id.isEmpty { selection = nil }
        else if let display = displays.first(where: { $0.id == id }) { selection = display }
        else { return store.read()?.id == id } // Keep an already disconnected selection.
        guard store.write(selection) else { return false }
        DistributedNotificationCenter.default().postNotificationName(notificationName, object: nil, userInfo: nil, deliverImmediately: true)
        return true
    }
}

@_cdecl("agent_companion_display_settings_json")
public func agentCompanionDisplaySettingsJSON() -> UnsafeMutablePointer<CChar>? {
    guard let data = try? JSONEncoder().encode(DisplayPreferences.shared.snapshot()),
          let json = String(data: data, encoding: .utf8) else { return nil }
    return strdup(json)
}

@_cdecl("agent_companion_free_native_string")
public func agentCompanionFreeNativeString(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }

@_cdecl("agent_companion_select_display")
public func agentCompanionSelectDisplay(_ identifier: UnsafePointer<CChar>?) -> Bool {
    guard let identifier else { return false }
    return DisplayPreferences.shared.select(String(cString: identifier))
}
