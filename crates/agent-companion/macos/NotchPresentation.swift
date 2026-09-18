// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import QuartzCore
import SwiftUI

/// A critically damped spring. Retargeting keeps both position and velocity,
/// so entering again during collapse never restarts from the compact frame.
struct NotchMotion {
    private(set) var height: CGFloat = 0
    private(set) var target: CGFloat = 0
    private(set) var velocity: CGFloat = 0
    var isMoving: Bool { height != target || velocity != 0 }

    mutating func retarget(_ height: CGFloat, animated: Bool) {
        target = height
        if !animated { finish() }
    }

    mutating func finish() {
        height = target
        velocity = 0
    }

    mutating func advance(by seconds: TimeInterval, minimum: CGFloat) {
        let rate: CGFloat = 30
        let time = CGFloat(max(0, seconds))
        let offset = height - target
        let coefficient = velocity + rate * offset
        let decay = exp(-rate * time)
        height = target + (offset + coefficient * time) * decay
        velocity = (velocity - rate * coefficient * time) * decay
        if height < minimum { height = minimum; velocity = max(0, velocity) }
        if abs(height - target) < 0.25 && abs(velocity) < 4 { finish() }
    }
}

private final class NotchContentView: NSHostingView<CompanionView> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// Content stays at its measured height (bounded by the screen) and pinned to the top.
/// Only this clipped, opaque silhouette grows; SwiftUI never replaces/fades
/// the compact strip or compresses the task layout into an intermediate size.
final class NotchSurfaceView: NSView {
    let model: CompanionModel
    private let host: NotchContentView
    private let measurement: NotchContentView
    private let outline = CAShapeLayer()
    private var tracking: NSTrackingArea?
    private var detailsHeight: CGFloat?
    private var contentHeight: CGFloat = 0
    var hover: (() -> Void)?
    var expansion: CGFloat = 0
    override var isFlipped: Bool { true }

    init(model: CompanionModel) {
        self.model = model
        // Keep the same full layout even while the panel clips it to the strip.
        // Removing the details changes SwiftUI's text raster alignment on
        // macOS 14, shifting the counters by a pixel on the first open frame.
        host = NotchContentView(rootView: CompanionView(model: model, showsDetails: true, drawsBackground: false))
        measurement = NotchContentView(rootView: CompanionView(model: model, showsDetails: true, drawsBackground: false))
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        layer?.mask = outline
        // Measure SwiftUI inside a plain clipping view. It is deliberately not
        // the window's contentView, so its intrinsic size cannot resize the panel.
        host.sizingOptions = [.intrinsicContentSize]
        measurement.sizingOptions = [.intrinsicContentSize]
        host.autoresizingMask = []
        addSubview(host)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func measure(maximumHeight: CGFloat) -> CGFloat {
        // Measure an unbounded copy without removing/recreating the visible
        // scroll view. Refreshes must preserve its scroll position and focus.
        measurement.rootView = CompanionView(model: model, showsDetails: true, drawsBackground: false)
        measurement.layoutSubtreeIfNeeded()
        let naturalHeight = max(model.compactHeight, measurement.fittingSize.height)
        contentHeight = min(maximumHeight, naturalHeight)
        let limit = naturalHeight > maximumHeight ? contentHeight - model.compactHeight : nil
        if detailsHeight != limit {
            detailsHeight = limit
            host.rootView = CompanionView(model: model, showsDetails: true, drawsBackground: false, detailsHeight: limit)
        }
        host.layoutSubtreeIfNeeded()
        needsLayout = true
        return contentHeight
    }

    override func layout() {
        super.layout()
        host.frame = CGRect(x: 0, y: 0, width: bounds.width, height: contentHeight)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        outline.frame = bounds
        outline.path = NotchOutline(hasCamera: model.hasCamera, expansion: expansion).path(in: bounds).cgPath
        CATransaction.commit()
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
    }
    override func mouseEntered(with event: NSEvent) { super.mouseEntered(with: event); hover?() }
    override func mouseExited(with event: NSEvent) { super.mouseExited(with: event); hover?() }
}

final class NotchPresentation {
    let surface: NotchSurfaceView
    private let panel: NSPanel
    private let model: CompanionModel
    private let reduceMotion: () -> Bool
    private var motion = NotchMotion()
    private var screen: CGRect = .zero
    private var width: CGFloat = 0
    private var metrics = NotchMetrics()
    private var maximumHeight: CGFloat = 0
    private var expandedHeight: CGFloat = 0
    private var timer: Timer?
    private var lastFrame: TimeInterval = 0
    var isAnimating: Bool { motion.isMoving }

    init(panel: NSPanel, model: CompanionModel,
         reduceMotion: @escaping () -> Bool = { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }) {
        self.panel = panel
        self.model = model
        self.reduceMotion = reduceMotion
        surface = NotchSurfaceView(model: model)
        panel.contentView = surface
    }

    deinit { timer?.invalidate() }

    func update(screen: CGRect, availableHeight: CGFloat? = nil, animated: Bool = true) {
        let limit = max(model.compactHeight, min(screen.height, availableHeight ?? screen.height) - 8)
        let geometryChanged = self.screen != screen || metrics != model.metrics || maximumHeight != limit
        self.screen = screen
        metrics = model.metrics
        maximumHeight = limit
        width = model.compactWidth
        expandedHeight = surface.measure(maximumHeight: maximumHeight)
        let target = model.expanded ? expandedHeight : model.compactHeight
        motion.retarget(target, animated: animated && !geometryChanged && !reduceMotion())
        render()
        guard motion.isMoving else { stop(); return }
        guard timer == nil else { return }
        lastFrame = CACurrentMediaTime()
        let timer = Timer(timeInterval: 1.0 / 120, repeats: true) { [weak self] _ in self?.tick() }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    private func tick() {
        let now = CACurrentMediaTime()
        if reduceMotion() {
            motion.finish()
        } else {
            motion.advance(by: now - lastFrame, minimum: model.compactHeight)
        }
        lastFrame = now
        render()
        if !motion.isMoving { stop() }
    }

    private func render() {
        // AppKit coordinates grow upward: moving the bottom while preserving
        // maxY keeps the counters and menu-bar attachment completely still.
        // Fractional backing pixels can shift the text raster even with a
        // fixed top edge. Keep every intermediate window on the pixel grid.
        let scale = max(1, panel.backingScaleFactor)
        let height = (min(maximumHeight, max(model.compactHeight, motion.height)) * scale).rounded(.down) / scale
        let frame = CGRect(x: screen.midX + model.centerOffset - width / 2,
                           y: screen.maxY - height, width: width, height: height)
        surface.expansion = min(1, max(0, (height - model.compactHeight) / max(1, expandedHeight - model.compactHeight)))
        if panel.frame != frame { panel.setFrame(frame, display: false) }
        surface.needsLayout = true
        surface.layoutSubtreeIfNeeded()
        panel.displayIfNeeded()
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }
}
