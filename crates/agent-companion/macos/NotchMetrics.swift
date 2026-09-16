// SPDX-License-Identifier: GPL-3.0-only
import AppKit

/// AppKit screen geometry is in points. The physical camera gap must never
/// be multiplied by the backing pixel scale or widened to fit task counts.
struct NotchMetrics: Equatable {
    let width: CGFloat
    let cameraHeight: CGFloat
    let centerOffset: CGFloat

    var hasCamera: Bool { cameraHeight > 0 }
    var contentScale: CGFloat { min(1, width / 216) }
    var statsHeight: CGFloat { hasCamera ? 24 : (28 * contentScale).rounded() }
    var compactHeight: CGFloat { cameraHeight + statsHeight }

    init(screen: CGRect = CGRect(x: 0, y: 0, width: 1920, height: 1080),
         safeTop: CGFloat = 0, topLeft: CGRect? = nil, topRight: CGRect? = nil) {
        if safeTop > 0, let left = topLeft, let right = topRight,
           ceil(left.maxX) < floor(right.minX), left.maxX >= screen.minX, right.minX <= screen.maxX {
            // NSWindow can align origins to whole points when moving between
            // screens. Round inward so that never overlaps either adjacent menu.
            let minX = ceil(left.maxX)
            let maxX = floor(right.minX)
            width = maxX - minX
            cameraHeight = ceil(safeTop)
            centerOffset = (minX + maxX) / 2 - screen.midX
        } else {
            // Keep the familiar size on large monitors, with a smaller strip
            // on compact displays. Changing Retina scale alone changes nothing.
            width = min(screen.width, min(216, max(180, (screen.width * 216 / 1920).rounded())))
            cameraHeight = 0
            centerOffset = 0
        }
    }

    init(screen: NSScreen) {
        self.init(screen: screen.frame, safeTop: screen.safeAreaInsets.top,
                  topLeft: screen.auxiliaryTopLeftArea, topRight: screen.auxiliaryTopRightArea)
    }
}
