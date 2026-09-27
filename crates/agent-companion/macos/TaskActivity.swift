// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

/// Shared by menu-bar dots, the popup header and individual task marks.
/// Only an entirely completed group is green; failed/stopped/unknown tasks
/// must never be mistaken for successful completion.
enum TaskActivity: Equatable {
    case running, completed, waiting, failed, idle

    init(state: String) {
        switch state {
        case "running": self = .running
        case "completed": self = .completed
        case "waiting": self = .waiting
        case "failed": self = .failed
        default: self = .idle
        }
    }

    static func aggregate(_ tasks: [CodexTask]) -> TaskActivity {
        if tasks.contains(where: { $0.activity == .waiting }) { return .waiting }
        if tasks.contains(where: { $0.activity == .running }) { return .running }
        if tasks.contains(where: { $0.activity == .failed }) { return .failed }
        if !tasks.isEmpty && tasks.allSatisfy({ $0.activity == .completed }) { return .completed }
        return .idle
    }

    var description: String {
        switch self {
        case .running: return "Tasks running"
        case .completed: return "All tasks completed"
        case .waiting: return "Approval or input needed"
        case .failed: return "A task failed"
        case .idle: return "No active tasks"
        }
    }

    var nsColor: NSColor {
        let rgb: (CGFloat, CGFloat, CGFloat)
        switch self {
        case .running: rgb = (0x4a, 0x9d, 0xe8)
        case .completed: rgb = (0x55, 0xc9, 0x8d)
        case .waiting, .failed: rgb = (0xe3, 0xbf, 0x52)
        case .idle: rgb = (0x6a, 0x6a, 0x80)
        }
        return NSColor(srgbRed: rgb.0 / 255, green: rgb.1 / 255, blue: rgb.2 / 255, alpha: 1)
    }

    func color(in theme: CompanionTheme) -> Color {
        // Keep the shared hue while retaining small-symbol contrast on the
        // popup's pale background. Native menu dots use a contrasting outline.
        let color = theme == .light && self != .idle
            ? nsColor.blended(withFraction: 0.32, of: .black) ?? nsColor : nsColor
        return Color(nsColor: color)
    }

    /// High-visibility colors shared by menu dots and breathing task marks.
    var indicatorColor: NSColor {
        let rgb: (CGFloat, CGFloat, CGFloat)
        switch self {
        case .running: rgb = (0x32, 0xb8, 0xff)
        case .completed: rgb = (0x25, 0xd7, 0x7a)
        case .waiting, .failed: rgb = (0xff, 0xc5, 0x2f)
        case .idle: rgb = (0xa2, 0xad, 0xbd)
        }
        return NSColor(srgbRed: rgb.0 / 255, green: rgb.1 / 255, blue: rgb.2 / 255, alpha: 1)
    }

    static let breathingGray = NSColor(srgbRed: 0x92 / 255.0, green: 0x97 / 255.0,
                                       blue: 0x9f / 255.0, alpha: 1)

    /// A full, opaque color change keeps both ends visible on any wallpaper.
    /// Every breathing hue shares the same two-second bright → gray → bright cycle.
    static func breathingColor(_ bright: NSColor, at time: TimeInterval) -> NSColor {
        let grayFraction = (1 - cos(time * .pi)) / 2
        return bright.blended(withFraction: grayFraction, of: breathingGray) ?? bright
    }
}

struct TaskActivityMark: View {
    let activity: TaskActivity
    var symbol = "circle.inset.filled"
    var size: CGFloat = 6
    var animating = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.companionTheme) private var theme

    var body: some View {
        let breathing = animating && activity == .running && !reduceMotion
        TimelineView(.animation(minimumInterval: 0.1, paused: !breathing)) { context in
            Image(systemName: symbol)
                .font(.system(size: size, weight: .semibold))
                .foregroundStyle(breathing
                    ? Color(nsColor: TaskActivity.breathingColor(activity.indicatorColor,
                                at: context.date.timeIntervalSinceReferenceDate))
                    : activity.color(in: theme))
        }
        .accessibilityHidden(true)
    }
}
