// SPDX-License-Identifier: GPL-3.0-only
import Foundation

final class FixtureAppUpdateBridge: AppUpdateBridging {
    static let releaseURL = "https://github.com/WXGopher/agent-companion/releases/tag/v0.3.22"
    static let available = AppUpdateSnapshot(latestVersion: "0.3.22", releaseUrl: releaseURL)
    var value: AppUpdateSnapshot? = AppUpdateSnapshot()
    var panelOpens = 0
    var snapshotReads = 0
    var onPanelOpen: (() -> Void)?

    func panelOpened() { panelOpens += 1; onPanelOpen?() }
    func snapshot() -> AppUpdateSnapshot? { snapshotReads += 1; return value }
}
