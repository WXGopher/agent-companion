// SPDX-License-Identifier: GPL-3.0-only
import AppKit

/// Keep known readings visible while they refresh, with an explicit stale mark.
/// Rendering never starts an account request or substitutes another instance.
struct MenuBarUsage: Equatable {
    struct Reading {
        let usage: WeeklyUsage
        var stale = false
    }
    struct Row: Equatable {
        let id: String
        let label: String
        let remaining: Int
        var stale = false
        var text: String { "\(remaining)%\(stale ? "*" : "")" }
    }
    let rows: [Row]

    init(instances: [CodexInstance], now: Date = Date(),
         usage: (CodexInstance) -> WeeklyUsage?) {
        self.init(instances: instances, now: now, reading: { instance in
            usage(instance).map { Reading(usage: $0) }
        })
    }

    init(instances: [CodexInstance], now: Date = Date(),
         reading: (CodexInstance) -> Reading?) {
        rows = instances.sorted { ($0.id == "codex" ? 0 : 1) < ($1.id == "codex" ? 0 : 1) }
            .compactMap { instance in
                guard let value = reading(instance) else { return nil }
                return Row(id: instance.id, label: instance.label,
                           remaining: 100 - min(100, max(0, value.usage.usedPercent)),
                           stale: value.stale || Self.remaining(value.usage, at: now) == nil)
            }
    }

    static func remaining(_ usage: WeeklyUsage?, at now: Date) -> Int? {
        guard let usage, !usage.expired,
              usage.resetsAt.map({ $0 > now.timeIntervalSince1970 }) ?? true else { return nil }
        return 100 - min(100, max(0, usage.usedPercent))
    }

    var accessibilityDescription: String {
        guard !rows.isEmpty else { return "Agent Companion tasks and usage" }
        return rows.map {
            $0.stale ? "\($0.label): Last known weekly quota remaining: \($0.remaining)%; awaiting update"
                : "\($0.label): \($0.text) weekly quota remaining"
        }.joined(separator: "; ")
    }

    var image: NSImage? {
        guard !rows.isEmpty else {
            return NSImage(systemSymbolName: "terminal", accessibilityDescription: accessibilityDescription)
        }
        // A template image retains the native status button's highlight,
        // contrast and click handling. Compact stacked figures match menu-bar
        // meters such as Stats, without a separate badge or background.
        let font = NSFont.monospacedDigitSystemFont(ofSize: rows.count == 1 ? 12 : 9, weight: .medium)
        let lines = rows.map { NSAttributedString(string: $0.text, attributes: [.font: font, .foregroundColor: NSColor.black]) }
        let size = NSSize(width: ceil(lines.map { $0.size().width }.max() ?? 0) + 4, height: 22)
        let image = NSImage(size: size, flipped: false) { rect in
            for (index, line) in lines.enumerated() {
                let centerY = rect.height * (1 - (CGFloat(index) + 0.5) / CGFloat(lines.count))
                line.draw(at: NSPoint(x: (rect.width - line.size().width) / 2,
                                     y: centerY - line.size().height / 2))
            }
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = accessibilityDescription
        return image
    }

    func apply(to item: NSStatusItem) {
        guard let button = item.button else { return }
        button.title = ""
        button.image = image
        button.imagePosition = .imageOnly
        button.imageScaling = .scaleNone
        button.toolTip = accessibilityDescription
        button.setAccessibilityLabel(accessibilityDescription)
        item.length = rows.isEmpty ? NSStatusItem.squareLength : (button.image?.size.width ?? 22) + 10
    }
}
