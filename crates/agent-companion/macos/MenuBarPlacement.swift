// SPDX-License-Identifier: GPL-3.0-only
import AppKit

struct MenuBarPlacement {
    let autosaveName: String
    private var positionKey: String { "NSStatusItem Preferred Position \(autosaveName)" }

    init(autosaveName: String = "AgentCompanionUsage") {
        self.autosaveName = autosaveName
    }

    func makeItem() -> NSStatusItem {
        // AppKit otherwise inserts a new item at the left end of the extras,
        // which can be entirely behind the camera on a crowded built-in screen.
        // Seed only the initial position; subsequent launches respect user moves.
        let defaults = UserDefaults.standard
        if defaults.object(forKey: positionKey) == nil {
            defaults.set(0, forKey: positionKey)
        }
        // Loading a saved hidden state while assigning autosaveName clears its
        // position immediately. Restore the always-visible entry first.
        defaults.set(true, forKey: "NSStatusItem Visible \(autosaveName)")
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.autosaveName = autosaveName
        // The menu entry is always visible, including after a saved
        // AppKit visibility flag or a previous removal hid the item.
        item.isVisible = true
        return item
    }

    func remove(_ item: NSStatusItem) {
        let defaults = UserDefaults.standard
        let position = defaults.object(forKey: positionKey)
        NSStatusBar.system.removeStatusItem(item)
        // Removing an item can clear AppKit's saved position. Keep it when the
        // app quits, so relaunching it does not move it.
        if let position { defaults.set(position, forKey: positionKey) }
    }
}
