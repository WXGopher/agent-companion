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
        let remaining: Int?
        var stale = false
        let activity: TaskActivity
        let hasTasks: Bool
        var text: String { remaining.map { "\($0)%\(stale ? "*" : "")" } ?? "—" }
        var statusDescription: String { hasTasks ? activity.description : "No tasks" }
    }
    let rows: [Row]

    init(instances: [CodexInstance], tasks: [CodexTask] = [], now: Date = Date(),
         usage: (CodexInstance) -> WeeklyUsage?) {
        self.init(instances: instances, tasks: tasks, now: now, reading: { instance in
            usage(instance).map { Reading(usage: $0) }
        })
    }

    init(instances: [CodexInstance], tasks: [CodexTask] = [], now: Date = Date(),
         reading: (CodexInstance) -> Reading?) {
        rows = instances.sorted { ($0.id == "codex" ? 0 : 1) < ($1.id == "codex" ? 0 : 1) }
            .map { instance in
                let value = reading(instance)
                let tasks = tasks.filter { $0.sourceID == instance.id }
                return Row(id: instance.id, label: instance.label,
                           remaining: value.map { 100 - min(100, max(0, $0.usage.usedPercent)) },
                           stale: value.map { $0.stale || Self.remaining($0.usage, at: now) == nil } ?? false,
                           activity: TaskActivity.aggregate(tasks), hasTasks: !tasks.isEmpty)
            }
    }

    static func remaining(_ usage: WeeklyUsage?, at now: Date) -> Int? {
        guard let usage, !usage.expired,
              usage.resetsAt.map({ $0 > now.timeIntervalSince1970 }) ?? true else { return nil }
        return 100 - min(100, max(0, usage.usedPercent))
    }

    var accessibilityDescription: String {
        guard !rows.isEmpty else { return "Agent Companion tasks and usage" }
        return rows.map { row in
            let quota: String
            if let remaining = row.remaining {
                quota = row.stale ? "Last known weekly quota remaining: \(remaining)%; awaiting update"
                    : "\(row.text) weekly quota remaining"
            } else { quota = "Weekly quota unavailable" }
            return "\(row.label): \(row.statusDescription). \(quota)"
        }.joined(separator: "; ")
    }

    var image: NSImage? {
        guard !rows.isEmpty else {
            return NSImage(systemSymbolName: "terminal", accessibilityDescription: accessibilityDescription)
        }
        // Reserve a gutter for a separate colored overlay. Keeping percentages
        // templated preserves AppKit's wallpaper contrast and pressed highlight.
        let font = NSFont.monospacedDigitSystemFont(ofSize: rows.count == 1 ? 12 : 9, weight: .medium)
        let lines = rows.map { NSAttributedString(string: $0.text, attributes: [.font: font, .foregroundColor: NSColor.black]) }
        let size = NSSize(width: ceil(lines.map { $0.size().width }.max() ?? 0) + 14, height: 22)
        let image = NSImage(size: size, flipped: false) { rect in
            for (index, line) in lines.enumerated() {
                let centerY = rect.height * (1 - (CGFloat(index) + 0.5) / CGFloat(lines.count))
                line.draw(at: NSPoint(x: 10 + (rect.width - 10 - line.size().width) / 2,
                                     y: centerY - line.size().height / 2))
            }
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = accessibilityDescription
        return image
    }

    @discardableResult func apply(to item: NSStatusItem, indicator: MenuBarActivityView? = nil) -> MenuBarActivityView? {
        guard let button = item.button else { return nil }
        button.title = ""
        button.image = image
        button.imagePosition = .imageOnly
        button.imageScaling = .scaleNone
        button.toolTip = accessibilityDescription
        button.setAccessibilityLabel(accessibilityDescription)
        item.length = rows.isEmpty ? NSStatusItem.squareLength : (button.image?.size.width ?? 22) + 10
        let dots = indicator ?? button.subviews.compactMap { $0 as? MenuBarActivityView }.first ?? MenuBarActivityView()
        if dots.superview !== button {
            dots.removeFromSuperview()
            dots.frame = button.bounds
            dots.autoresizingMask = [.width, .height]
            button.addSubview(dots)
        }
        dots.update(self)
        return dots
    }
}

private extension TaskActivity {
    /// Small menu-bar marks need a stronger fill than the popup's larger icons.
    var menuBarColor: NSColor {
        let rgb: (CGFloat, CGFloat, CGFloat)
        switch self {
        case .running: rgb = (0x16, 0x8b, 0xff)
        case .completed: rgb = (0x25, 0xd7, 0x7a)
        case .waiting, .failed: rgb = (0xff, 0xc5, 0x2f)
        case .idle: rgb = (0xa2, 0xad, 0xbd)
        }
        return NSColor(srgbRed: rgb.0 / 255, green: rgb.1 / 255, blue: rgb.2 / 255, alpha: 1)
    }
}

/// Color lives outside the template image. This view cannot intercept clicks,
/// accessibility, or Command-drag placement handled by the native status button.
final class MenuBarActivityView: NSView {
    private var value = MenuBarUsage(instances: [], usage: { _ in nil })
    private let reduceMotion: () -> Bool
    private let clock: () -> TimeInterval
    private let visible: (NSView) -> Bool
    private var enabled = true
    private var timer: Timer?
    private var motionObserver: NSObjectProtocol?
    private(set) var animationTicks = 0
    var isAnimating: Bool { timer != nil }

    init(reduceMotion: @escaping () -> Bool = { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion },
         clock: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime },
         visible: @escaping (NSView) -> Bool = { $0.window?.isVisible == true }) {
        self.reduceMotion = reduceMotion
        self.clock = clock
        self.visible = visible
        super.init(frame: .zero)
        setAccessibilityElement(false)
        motionObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.refreshAnimation(); self?.needsDisplay = true }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    deinit {
        timer?.invalidate()
        if let motionObserver { NSWorkspace.shared.notificationCenter.removeObserver(motionObserver) }
    }
    override var isOpaque: Bool { false }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); refreshAnimation() }
    override func viewDidHide() { super.viewDidHide(); refreshAnimation() }
    override func viewDidUnhide() { super.viewDidUnhide(); refreshAnimation() }
    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); needsDisplay = true }

    func update(_ value: MenuBarUsage) {
        self.value = value
        enabled = true
        needsDisplay = true
        refreshAnimation()
        RunLoop.main.perform(inModes: [.common]) { [weak self] in self?.refreshAnimation() }
    }

    func stop() {
        enabled = false
        timer?.invalidate()
        timer = nil
    }

    private var shouldAnimate: Bool {
        enabled && visible(self) && !isHiddenOrHasHiddenAncestor
            && !reduceMotion() && value.rows.contains { $0.activity == .running }
    }

    func refreshAnimation() {
        guard shouldAnimate else { timer?.invalidate(); timer = nil; return }
        guard timer == nil else { return }
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
            guard let self else { return }
            guard shouldAnimate else { refreshAnimation(); return }
            animationTicks += 1
            needsDisplay = true
        }
        timer.tolerance = 0.015
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    override func draw(_ dirtyRect: NSRect) {
        guard !value.rows.isEmpty, let button = superview as? NSStatusBarButton,
              let image = button.image else { return }
        // NSButton centers its image inside the complete native button. Dots
        // share the template's row centers, including taller notched menu bars.
        let origin = NSPoint(x: (bounds.width - image.size.width) / 2,
                             y: (bounds.height - image.size.height) / 2)
        let diameter: CGFloat = value.rows.count == 1 ? 8 : 7
        // Keep the blue fill visible throughout the two-second breathing cycle.
        let pulse = isAnimating ? 0.9 + 0.1 * cos(clock() * .pi) : 1
        for (index, row) in value.rows.enumerated() {
            let centerY = origin.y + image.size.height * (1 - (CGFloat(index) + 0.5) / CGFloat(value.rows.count))
            let circle = NSBezierPath(ovalIn: NSRect(x: origin.x + 5 - diameter / 2,
                                                    y: centerY - diameter / 2,
                                                    width: diameter, height: diameter))
            row.activity.menuBarColor.withAlphaComponent(row.activity == .running ? pulse : 1).setFill()
            circle.fill()
            // Dark inner and light outer edges separate the fill from bright,
            // dark and colored wallpapers, including the pressed background.
            NSColor.black.withAlphaComponent(0.55).setStroke()
            circle.lineWidth = 0.7
            circle.stroke()
            let outline = NSBezierPath(ovalIn: circle.bounds.insetBy(dx: -0.55, dy: -0.55))
            NSColor.white.withAlphaComponent(0.85).setStroke()
            outline.lineWidth = 0.6
            outline.stroke()
        }
    }
}
