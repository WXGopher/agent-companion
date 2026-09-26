// SPDX-License-Identifier: GPL-3.0-only
import AppKit

@MainActor enum MenuBarPlacementTests {
    private final class PositionObserver: NSObject {
        var writes: [Double] = []

        override func observeValue(forKeyPath keyPath: String?, of object: Any?,
                                   change: [NSKeyValueChangeKey: Any]?, context: UnsafeMutableRawPointer?) {
            if let value = change?[.newKey] as? NSNumber { writes.append(value.doubleValue) }
        }
    }

    static func run() {
        precondition(Bundle.main.bundleIdentifier != "com.wxgopher.agent-companion",
                     "Placement tests must run in the standalone test process, not the production app")
        verifyInitialHint()
        verifySavedPositionAndVisibility()
        print("Menu placement: initial right-side hint, saved position, explicit visibility and removal/recreation persistence passed")
    }

    private static func verifyInitialHint() {
        let name = "PlacementTest-\(UUID().uuidString)"
        let key = "NSStatusItem Preferred Position \(name)"
        let defaults = UserDefaults.standard
        let placement = MenuBarPlacement(autosaveName: name)
        let observer = PositionObserver()
        defaults.addObserver(observer, forKeyPath: key, options: [.new], context: nil)
        var item: NSStatusItem?
        defer {
            defaults.removeObserver(observer, forKeyPath: key)
            if let item { placement.remove(item) }
            cleanup(name: name)
        }
        precondition(defaults.object(forKey: key) == nil)
        item = placement.makeItem()
        precondition(observer.writes.first == 0,
                     "The first placement must seed the right-side hint before AppKit lays out the item")
        precondition(item?.autosaveName == name && item?.isVisible == true)
        item?.button?.title = "1%"
        item?.length = 32
        // AppKit may replace the zero hint with its actual position. Check for
        // a persisted coordinate after layout, without requiring it to stay 0.
        _ = settledPosition(key: key)
    }

    private static func verifySavedPositionAndVisibility() {
        let name = "PlacementTest-\(UUID().uuidString)"
        let key = "NSStatusItem Preferred Position \(name)"
        let defaults = UserDefaults.standard
        let placement = MenuBarPlacement(autosaveName: name)
        var item: NSStatusItem?
        defer {
            if let item { placement.remove(item) }
            cleanup(name: name)
        }
        // Simulate a user-chosen location and AppKit's independent hidden flag.
        // Only this unique test item's keys in the test process domain change.
        defaults.set(320, forKey: key)
        defaults.set(false, forKey: "NSStatusItem Visible \(name)")
        item = placement.makeItem()
        precondition(item?.autosaveName == name && item?.isVisible == true,
                     "The app's enabled entry must override AppKit's saved hidden state")
        precondition((defaults.object(forKey: key) as? NSNumber)?.doubleValue == 320,
                     "Creating an item must preserve a previously saved position")
        item?.button?.title = "1%"
        item?.length = 32
        let position = settledPosition(key: key)
        precondition(item?.isVisible == true, "A delayed AppKit restore hid the enabled entry")

        placement.remove(item!)
        item = nil
        precondition(settledPosition(key: key) == position,
                     "Removing an item cleared its preferred position after the run loop settled")
        item = placement.makeItem()
        precondition(item?.isVisible == true)
        item?.button?.title = "1%"
        item?.length = 32
        precondition(settledPosition(key: key) == position,
                     "Recreating the item lost its saved position")
        precondition(item?.isVisible == true)
    }

    private static func settledPosition(key: String) -> Double {
        let defaults = UserDefaults.standard
        let deadline = Date().addingTimeInterval(2)
        var previous = (defaults.object(forKey: key) as? NSNumber)?.doubleValue
        var stableSince = Date()
        while Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
            let current = (defaults.object(forKey: key) as? NSNumber)?.doubleValue
            if current != previous {
                previous = current
                stableSince = Date()
            }
            if Date().timeIntervalSince(stableSince) >= 0.25 { break }
        }
        precondition(Date().timeIntervalSince(stableSince) >= 0.25,
                     "AppKit's persisted status-item position did not settle")
        guard let position = previous else {
            preconditionFailure("The status item's preferred position disappeared")
        }
        precondition(position.isFinite && position >= 0)
        return position
    }

    private static func cleanup(name: String) {
        // Let native removal notifications finish before deleting the unique
        // test keys, so delayed persistence does not recreate them afterward.
        RunLoop.main.run(until: Date().addingTimeInterval(0.25))
        let defaults = UserDefaults.standard
        defaults.removeObject(forKey: "NSStatusItem Preferred Position \(name)")
        defaults.removeObject(forKey: "NSStatusItem Visible \(name)")
        defaults.synchronize()
    }
}
