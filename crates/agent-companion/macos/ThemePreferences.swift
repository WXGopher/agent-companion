// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import Combine
import SwiftUI

enum CompanionTheme: String {
    case dark, light

    var toggled: CompanionTheme { self == .dark ? .light : .dark }
    var colorScheme: ColorScheme { self == .dark ? .dark : .light }
    var appearance: NSAppearance? { NSAppearance(named: self == .dark ? .darkAqua : .aqua) }
    var toggleLabel: String { self == .dark ? "Switch to light theme" : "Switch to dark theme" }
    var toggleSymbol: String { self == .dark ? "sun.max" : "moon" }
    var palette: CompanionPalette { CompanionPalette(theme: self) }
}

/// Semantic colors align popup content with its native AppKit chrome.
/// The dark palette retains the original black, mint and translucent surfaces.
struct CompanionPalette {
    let theme: CompanionTheme
    private var isLight: Bool { theme == .light }

    var backgroundNSColor: NSColor {
        isLight ? NSColor(srgbRed: 0.97, green: 0.98, blue: 0.975, alpha: 1) : .black
    }
    var background: Color { Color(nsColor: backgroundNSColor) }
    var primaryText: Color { isLight ? Color(red: 0.13, green: 0.16, blue: 0.18) : .white }
    var secondaryText: Color { isLight ? Color(red: 0.36, green: 0.39, blue: 0.41) : .secondary }
    var mutedText: Color { isLight ? Color(red: 0.39, green: 0.42, blue: 0.44) : .white.opacity(0.55) }
    var mutedIcon: Color { isLight ? Color(red: 0.44, green: 0.47, blue: 0.49) : .white.opacity(0.35) }
    var accent: Color {
        isLight ? Color(red: 0.08, green: 0.43, blue: 0.32) : Color(red: 0.45, green: 0.90, blue: 0.74)
    }
    var warning: Color { isLight ? Color(red: 0.62, green: 0.31, blue: 0.015) : .orange }
    var surface: Color { isLight ? .black.opacity(0.035) : .white.opacity(0.055) }
    var row: Color { isLight ? .black.opacity(0.03) : .white.opacity(0.045) }
    var hover: Color { isLight ? accent.opacity(0.10) : .white.opacity(0.10) }
    var controlTrack: Color { isLight ? .black.opacity(0.045) : .white.opacity(0.06) }
    var selectedControl: Color { isLight ? .white : .white.opacity(0.10) }
    var inactiveControl: Color { isLight ? .black.opacity(0.035) : .white.opacity(0.035) }
    var notice: Color { isLight ? .black.opacity(0.05) : .white.opacity(0.08) }
    var separator: Color { isLight ? .black.opacity(0.07) : .white.opacity(0.06) }
    var quotaTrack: Color { isLight ? .black.opacity(0.10) : .white.opacity(0.10) }
    var chartTrack: Color { isLight ? .black.opacity(0.05) : .white.opacity(0.05) }
    var accentSurface: Color { accent.opacity(0.08) }
}

private struct CompanionThemeKey: EnvironmentKey {
    static let defaultValue = CompanionTheme.dark
}

extension EnvironmentValues {
    var companionTheme: CompanionTheme {
        get { self[CompanionThemeKey.self] }
        set { self[CompanionThemeKey.self] = newValue }
    }
}

struct ThemePreferenceStore {
    let domain: String
    static let key = "companionTheme" as CFString

    init(domain: String = "com.wxgopher.agent-companion") { self.domain = domain }

    func read() -> CompanionTheme {
        CFPreferencesAppSynchronize(domain as CFString)
        guard let value = CFPreferencesCopyAppValue(Self.key, domain as CFString) as? String,
              let theme = CompanionTheme(rawValue: value) else { return .dark }
        return theme
    }

    func write(_ theme: CompanionTheme) -> Bool {
        CFPreferencesAppSynchronize(domain as CFString)
        let previous = CFPreferencesCopyAppValue(Self.key, domain as CFString)
        CFPreferencesSetAppValue(Self.key, theme.rawValue as CFString, domain as CFString)
        guard CFPreferencesAppSynchronize(domain as CFString) else {
            CFPreferencesSetAppValue(Self.key, previous, domain as CFString)
            CFPreferencesAppSynchronize(domain as CFString)
            return false
        }
        return true
    }
}

/// The observable preference updates the popup without changing its model,
/// root identity, scroll views or subscription refresh lifecycle.
final class ThemePreferences: ObservableObject {
    static let shared = ThemePreferences()
    @Published private(set) var theme: CompanionTheme
    private let store: ThemePreferenceStore
    private var observer: NSObjectProtocol?
    var notificationName: Notification.Name { Notification.Name("\(store.domain).themePreferenceChanged") }

    init(store: ThemePreferenceStore = ThemePreferenceStore()) {
        self.store = store
        theme = store.read()
        observer = DistributedNotificationCenter.default().addObserver(forName: notificationName, object: nil, queue: .main) { [weak self] _ in
            self?.reload()
        }
    }

    deinit {
        if let observer { DistributedNotificationCenter.default().removeObserver(observer) }
    }

    func reload() {
        let saved = store.read()
        if theme != saved { theme = saved }
    }

    @discardableResult func set(_ theme: CompanionTheme) -> Bool {
        guard store.write(theme) else { return false }
        if self.theme != theme { self.theme = theme }
        DistributedNotificationCenter.default().postNotificationName(notificationName, object: nil, userInfo: nil, deliverImmediately: true)
        return true
    }

    @discardableResult func toggle() -> Bool { set(theme.toggled) }
}
