// SPDX-License-Identifier: GPL-3.0-only
import Foundation

enum PrimaryCodexAppTests {
    static func run() throws {
        let fm = FileManager.default
        let temporary = fm.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try fm.createDirectory(at: temporary, withIntermediateDirectories: true)
        let root = temporary.resolvingSymlinksInPath()
        defer { try? fm.removeItem(at: root) }
        let system = root.appendingPathComponent("Applications")
        let home = root.appendingPathComponent("user")
        let user = home.appendingPathComponent("Applications")
        let candidates = PrimaryCodexApp.candidates(systemApplications: system, userApplications: user)
        func create(_ app: URL, identity: String = "com.openai.codex", binary: Bool = false) throws {
            try fm.createDirectory(at: app.appendingPathComponent("Contents/MacOS"), withIntermediateDirectories: true)
            try fm.createDirectory(at: app.appendingPathComponent("Contents/Resources"), withIntermediateDirectories: true)
            let plist = try PropertyListSerialization.data(fromPropertyList: ["CFBundleIdentifier": identity],
                                                          format: binary ? .binary : .xml, options: 0)
            try plist.write(to: app.appendingPathComponent("Contents/Info.plist"))
            for path in ["Contents/MacOS/ChatGPT", "Contents/Resources/codex"] {
                let file = app.appendingPathComponent(path)
                try Data("synthetic executable; never run".utf8).write(to: file)
                try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: file.path)
            }
        }
        func find() -> URL? { PrimaryCodexApp.find(systemApplications: system, userApplications: user) }
        for binary in [false, true] {
            for app in candidates {
                try create(app, binary: binary)
                precondition(find()?.path == app.path, "Every fixed location must support XML and binary plist identities")
                try fm.removeItem(at: app)
            }
        }
        // A genuine ChatGPT product at this filename must never pass as Codex.
        try create(candidates[2], identity: "different.application")
        precondition(find() == nil)
        try create(candidates[3], binary: true)
        precondition(find()?.path == candidates[3].path, "An invalid earlier identity must not hide a later valid installation")
        try create(candidates[1])
        precondition(find()?.path == candidates[1].path, "Existing Codex.app priority must remain unchanged")
        try create(candidates[0])
        precondition(find()?.path == candidates[0].path)
        try fm.removeItem(at: candidates[0].appendingPathComponent("Contents/Resources/codex"))
        try fm.removeItem(at: candidates[0].appendingPathComponent("Contents/MacOS/ChatGPT"))
        precondition(find()?.path == candidates[1].path, "An incomplete candidate must be skipped")
        try fm.setAttributes([.posixPermissions: 0o644], ofItemAtPath: candidates[1].appendingPathComponent("Contents/MacOS/ChatGPT").path)
        precondition(find()?.path == candidates[3].path, "A non-executable desktop cannot be a launch target")
        for app in candidates { try fm.removeItem(at: app) }

        let hidden = system.appendingPathComponent(".Dodex/Dodex.app")
        try create(hidden)
        precondition(find() == nil, "Dodex must not be discovered by its shared vendor identity")
        try fm.createSymbolicLink(at: candidates[0], withDestinationURL: hidden)
        precondition(find() == nil, "An application alias must not turn Dodex into primary")
        try fm.removeItem(at: candidates[0])
        try create(candidates[2])
        let cli = candidates[2].appendingPathComponent("Contents/Resources/codex")
        try fm.removeItem(at: cli)
        try fm.createSymbolicLink(at: cli, withDestinationURL: hidden.appendingPathComponent("Contents/Resources/codex"))
        precondition(find()?.path == candidates[2].path, "A redirected bundled CLI must not be selected")
        let desktop = candidates[2].appendingPathComponent("Contents/MacOS/ChatGPT")
        try fm.removeItem(at: desktop)
        try fm.createSymbolicLink(at: desktop, withDestinationURL: hidden.appendingPathComponent("Contents/MacOS/ChatGPT"))
        precondition(find() == nil, "A redirected desktop executable must not be selected")
        print("Primary app discovery: fixed locations, XML/binary identities, priority, incomplete apps, isolated CLI and redirect rejection passed")
    }
}
