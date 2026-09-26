// SPDX-License-Identifier: GPL-3.0-only
import Foundation

@_silgen_name("agent_companion_version")
private func companionVersion() -> UnsafePointer<CChar>

enum CompanionAppInfo {
    static let title = "Agent Companion"
    // Rust owns this static string for the process lifetime. Read the binary's
    // Cargo version, including when running without a packaged Info.plist.
    static let version = String(cString: companionVersion())
}
