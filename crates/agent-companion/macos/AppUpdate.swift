// SPDX-License-Identifier: GPL-3.0-only
import Foundation

@_silgen_name("agent_companion_update_panel_open")
private func notifyUpdatePanelOpen()
@_silgen_name("agent_companion_update_snapshot_json")
private func readUpdateSnapshot() -> UnsafeMutablePointer<CChar>?
@_silgen_name("agent_companion_release_json")
private func releaseUpdateSnapshot(_ pointer: UnsafeMutablePointer<CChar>?)

struct AppUpdateSnapshot: Decodable, Equatable {
    var latestVersion: String?
    var releaseUrl: String?

    // Rust decides whether a version is newer. The native boundary only
    // restricts navigation to this app's published GitHub release pages.
    var release: (version: String, url: URL)? {
        guard let latestVersion, !latestVersion.isEmpty, let releaseUrl,
              let url = URL(string: releaseUrl), url.scheme == "https", url.host == "github.com",
              url.user == nil, url.password == nil, url.port == nil, url.query == nil, url.fragment == nil else { return nil }
        let prefix = "/wxgopher/agent-companion/releases/tag/"
        guard url.path.lowercased().hasPrefix(prefix), url.path.count > prefix.count else { return nil }
        return (latestVersion, url)
    }
}

protocol AppUpdateBridging: AnyObject {
    func panelOpened()
    func snapshot() -> AppUpdateSnapshot?
}

final class RustAppUpdateBridge: AppUpdateBridging {
    func panelOpened() { notifyUpdatePanelOpen() }

    func snapshot() -> AppUpdateSnapshot? {
        guard let pointer = readUpdateSnapshot() else { return nil }
        defer { releaseUpdateSnapshot(pointer) }
        return try? JSONDecoder().decode(AppUpdateSnapshot.self, from: Data(String(cString: pointer).utf8))
    }
}
