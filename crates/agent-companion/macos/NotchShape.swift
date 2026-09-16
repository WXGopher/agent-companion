// SPDX-License-Identifier: GPL-3.0-only
// Adapted from Open Island, by Octane0411 and contributors.
// Source: Sources/OpenIslandApp/NotchShape.swift at
// https://github.com/Octane0411/open-vibe-island/commit/b50f87aa7d58af1478837d48909eb68baa37f9b9
// See THIRD_PARTY_NOTICES.md for provenance and licensing.

import SwiftUI

struct NotchOutline: Shape {
    var hasCamera: Bool
    var expansion: CGFloat

    func path(in rect: CGRect) -> Path {
        if hasCamera {
            return NotchShape(topCornerRadius: 6, bottomCornerRadius: 10 + 14 * expansion).path(in: rect)
        }
        return RoundedRectangle(cornerRadius: 14 + 8 * expansion).path(in: rect)
    }
}

struct NotchShape: Shape {
    var topCornerRadius: CGFloat
    var bottomCornerRadius: CGFloat

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(topCornerRadius, bottomCornerRadius) }
        set {
            topCornerRadius = newValue.first
            bottomCornerRadius = newValue.second
        }
    }

    func path(in rect: CGRect) -> Path {
        let topR = min(topCornerRadius, rect.width / 4, rect.height / 4)
        let botR = min(bottomCornerRadius, rect.width / 4, rect.height / 2)

        var path = Path()

        // Start at top-left, after the inward curve
        path.move(to: CGPoint(x: rect.minX, y: rect.minY))

        // Top-left inward curve (concave, mimics notch edge)
        path.addQuadCurve(
            to: CGPoint(x: rect.minX + topR, y: rect.minY + topR),
            control: CGPoint(x: rect.minX + topR, y: rect.minY)
        )

        // Left edge down to bottom-left corner
        path.addLine(to: CGPoint(x: rect.minX + topR, y: rect.maxY - botR))

        // Bottom-left rounded corner
        path.addQuadCurve(
            to: CGPoint(x: rect.minX + topR + botR, y: rect.maxY),
            control: CGPoint(x: rect.minX + topR, y: rect.maxY)
        )

        // Bottom edge
        path.addLine(to: CGPoint(x: rect.maxX - topR - botR, y: rect.maxY))

        // Bottom-right rounded corner
        path.addQuadCurve(
            to: CGPoint(x: rect.maxX - topR, y: rect.maxY - botR),
            control: CGPoint(x: rect.maxX - topR, y: rect.maxY)
        )

        // Right edge up to top-right inward curve
        path.addLine(to: CGPoint(x: rect.maxX - topR, y: rect.minY + topR))

        // Top-right inward curve (concave)
        path.addQuadCurve(
            to: CGPoint(x: rect.maxX, y: rect.minY),
            control: CGPoint(x: rect.maxX - topR, y: rect.minY)
        )

        // Top edge back to start
        path.addLine(to: CGPoint(x: rect.minX, y: rect.minY))

        path.closeSubpath()
        return path
    }
}
