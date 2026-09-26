// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

/// Hosts the production popup root offscreen with its actual fixed dimensions.
@MainActor final class PopupTestWindow {
    let panel: NSPanel
    let host: NSHostingView<CompanionPopupView>

    init(model: CompanionModel, themePreferences: ThemePreferences? = nil) {
        panel = NSPanel(contentRect: CGRect(x: -10000, y: -10000,
            width: CompanionPopupLayout.width, height: CompanionPopupLayout.height),
            styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        host = NSHostingView(rootView: CompanionPopupView(model: model, themePreferences: themePreferences ?? ThemeTests.darkPreferences))
        panel.contentView = host
        host.frame = CGRect(origin: .zero, size: panel.frame.size)
        panel.orderFront(nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    }
}
