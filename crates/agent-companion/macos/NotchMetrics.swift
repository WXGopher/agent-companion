// SPDX-License-Identifier: GPL-3.0-only
import AppKit

/// AppKit screen geometry is in points. Reserve the actual camera cutout;
/// readable counters belong beside it, inside the menu-bar band.
struct NotchMetrics: Equatable {
    let width: CGFloat
    let cameraWidth: CGFloat
    let cameraHeight: CGFloat
    let centerOffset: CGFloat

    var hasCamera: Bool { cameraHeight > 0 }
    var contentScale: CGFloat { min(1, width / 216) }
    // The expanded surface is denser than the menu-bar indicators. Keep its
    // typography/spacing independent of the physical camera and hover region.
    var detailScale: CGFloat { 0.9 * contentScale }
    var cameraContentScale: CGFloat { min(1, cameraHeight / 32) }
    var cameraSideWidth: CGFloat { hasCamera ? (width - cameraWidth) / 2 : 0 }
    var statsHeight: CGFloat { hasCamera ? cameraHeight : (28 * contentScale).rounded() }
    var compactHeight: CGFloat { statsHeight }

    init(screen: CGRect = CGRect(x: 0, y: 0, width: 1920, height: 1080),
         safeTop: CGFloat = 0, topLeft: CGRect? = nil, topRight: CGRect? = nil) {
        if safeTop > 0, let left = topLeft, let right = topRight,
           left.maxX < right.minX, left.maxX >= screen.minX, right.minX <= screen.maxX {
            // Round the camera reservation outward so no glyph can fall under
            // the hardware after AppKit aligns the window to whole points.
            let minX = floor(left.maxX)
            let maxX = ceil(right.minX)
            cameraWidth = maxX - minX
            cameraHeight = ceil(safeTop)
            // The model adds only the width needed by the current numbers.
            // Keep the closed panel within the menu bar vertically.
            let side = ceil(24 * min(1, cameraHeight / 32))
            width = cameraWidth + 2 * side
            centerOffset = (minX + maxX) / 2 - screen.midX
        } else {
            // Keep the familiar size on large monitors, with a smaller strip
            // on compact displays. Changing Retina scale alone changes nothing.
            width = min(screen.width, min(216, max(180, (screen.width * 216 / 1920).rounded())))
            cameraWidth = 0
            cameraHeight = 0
            centerOffset = 0
        }
    }

    init(screen: NSScreen) {
        self.init(screen: screen.frame, safeTop: screen.safeAreaInsets.top,
                  topLeft: screen.auxiliaryTopLeftArea, topRight: screen.auxiliaryTopRightArea)
    }

    init(panelWidth: CGFloat) {
        width = panelWidth
        cameraWidth = 0
        cameraHeight = 0
        centerOffset = 0
    }
}
