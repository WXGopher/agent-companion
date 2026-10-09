// SPDX-License-Identifier: GPL-3.0-only
import Foundation

/// Fixed primary installation locations. The shared vendor ID alone cannot
/// distinguish primary Codex from Dodex's separately managed signed runtime.
enum PrimaryCodexApp {
    static func candidates(systemApplications: URL, userApplications: URL) -> [URL] {
        [systemApplications.appendingPathComponent("Codex.app"),
         userApplications.appendingPathComponent("Codex.app"),
         systemApplications.appendingPathComponent("ChatGPT.app"),
         userApplications.appendingPathComponent("ChatGPT.app")]
    }

    static func regularFile(_ url: URL, executable: Bool = false) -> Bool {
        let file = url.standardizedFileURL
        guard file.path == file.resolvingSymlinksInPath().path,
              let attributes = try? FileManager.default.attributesOfItem(atPath: file.path),
              attributes[.type] as? FileAttributeType == .typeRegular else { return false }
        return !executable || FileManager.default.isExecutableFile(atPath: file.path)
    }

    static func isCodex(_ app: URL) -> Bool {
        let plist = app.appendingPathComponent("Contents/Info.plist")
        guard regularFile(plist),
              regularFile(app.appendingPathComponent("Contents/MacOS/ChatGPT"), executable: true),
              let size = try? FileManager.default.attributesOfItem(atPath: plist.path)[.size] as? NSNumber,
              size.intValue <= 1024 * 1024,
              let data = try? Data(contentsOf: plist), data.count <= 1024 * 1024,
              let value = try? PropertyListSerialization.propertyList(from: data, options: [], format: nil),
              let dictionary = value as? [String: Any] else { return false }
        return dictionary["CFBundleIdentifier"] as? String == "com.openai.codex"
    }

    static func find(systemApplications: URL = URL(fileURLWithPath: "/Applications"),
                     userApplications: URL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications")) -> URL? {
        candidates(systemApplications: systemApplications, userApplications: userApplications).first(where: isCodex)
    }

    static func executable(in app: URL) -> URL? {
        let executable = app.appendingPathComponent("Contents/Resources/codex")
        return regularFile(executable, executable: true) ? executable : nil
    }


}

/// Foundation parses both binary and XML plists for native and Rust callers;
/// discovery never opens an application or invokes a subprocess.
@_cdecl("agent_companion_find_primary_codex_app")
func findPrimaryCodexApp(_ system: UnsafePointer<CChar>, _ user: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
    autoreleasepool {
        guard let app = PrimaryCodexApp.find(systemApplications: URL(fileURLWithPath: String(cString: system)),
                                            userApplications: URL(fileURLWithPath: String(cString: user))) else { return nil }
        var result: [String: String] = ["app": app.path]
        result["executable"] = PrimaryCodexApp.executable(in: app)?.path
        guard let data = try? JSONSerialization.data(withJSONObject: result),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return strdup(json)
    }
}

@_cdecl("agent_companion_release_primary_app_path")
func releasePrimaryAppPath(_ path: UnsafeMutablePointer<CChar>?) { free(path) }
