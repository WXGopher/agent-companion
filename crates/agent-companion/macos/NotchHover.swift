// SPDX-License-Identifier: GPL-3.0-only
import Foundation

/// Hover gets a little more room without enlarging the visible/clickable panel.
/// Closed bounds include the screen's very top edge, unlike CGRect.contains.
enum NotchHoverRegion {
    static func contains(_ point: CGPoint, panel: CGRect, screen: CGRect, cameraHeight: CGFloat = 0) -> Bool {
        guard !panel.isEmpty, !screen.isEmpty else { return false }
        // In the menu bar, stop exactly at our narrow side indicators instead
        // of extending the hover margin onto neighbouring menu icons.
        if cameraHeight > 0 && point.y >= screen.maxY - cameraHeight,
           point.x < panel.minX || point.x > panel.maxX { return false }
        let area = panel.insetBy(dx: -12, dy: -8).intersection(screen)
        guard !area.isEmpty else { return false }
        return point.x >= area.minX && point.x <= area.maxX
            && point.y >= area.minY && point.y <= area.maxY
    }
}

/// Tracking areas can be rebuilt while SwiftUI resizes. Reconcile their events
/// and mouse movement against the current pointer position, not event direction.
final class NotchHover {
    private let containsPointer: () -> Bool
    private let expand: () -> Void
    private let collapse: () -> Void
    private var inside: Bool?
    private var pinned = false
    private var dismissed = false
    private var stopped = false
    private var pending: DispatchWorkItem?
    private var pollingTimer: Timer?
    private var revision: UInt64 = 0

    init(containsPointer: @escaping () -> Bool, expand: @escaping () -> Void, collapse: @escaping () -> Void) {
        self.containsPointer = containsPointer
        self.expand = expand
        self.collapse = collapse
    }

    deinit {
        pollingTimer?.invalidate()
        pending?.cancel()
    }

    func start() {
        guard pollingTimer == nil else { return }
        stopped = false
        inside = nil
        pinned = false
        dismissed = false
        update()
        // Menu-bar tracking and a pointer clamped to the screen edge may not
        // deliver mouseMoved/entered events. This also covers the hover margin
        // outside our window without placing an invisible window over menu icons.
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in self?.update() }
        timer.tolerance = 0.02
        RunLoop.main.add(timer, forMode: .common)
        pollingTimer = timer
    }

    func update() {
        guard !stopped else { return }
        let current = containsPointer()
        // Movement within the strip and stale enter/exit events must not keep
        // resetting the dwell delay or cancel a pending expansion.
        guard inside != current else { return }
        inside = current
        cancelPending()
        if !current { dismissed = false }
        guard !pinned, !current || !dismissed else { return }
        let scheduledRevision = revision
        let work = DispatchWorkItem { [weak self] in
            guard let self, !stopped, !pinned, revision == scheduledRevision else { return }
            guard containsPointer() == current else { update(); return }
            pending = nil
            if current { if !dismissed { expand() } }
            else { collapse() }
        }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (current ? 0.18 : 0.35), execute: work)
    }

    func pin() {
        pinned = true
        dismissed = false
        cancelPending()
    }

    /// Call after the panel has collapsed, so its new hit area is used.
    func dismiss() {
        cancelPending()
        pinned = false
        let current = containsPointer()
        inside = current
        dismissed = current
    }

    func stop() {
        stopped = true
        pollingTimer?.invalidate()
        pollingTimer = nil
        cancelPending()
    }

    private func cancelPending() {
        revision &+= 1
        pending?.cancel()
        pending = nil
    }
}
