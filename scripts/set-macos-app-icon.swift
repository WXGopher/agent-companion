// SPDX-License-Identifier: GPL-3.0-only
// Apply a Finder custom icon without changing the app's signed Contents.
import AppKit
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 2 else {
    fail("Usage: swift scripts/set-macos-app-icon.swift IMAGE_OR_--clear APP_PATH")
}
let app = URL(fileURLWithPath: args[1]).resolvingSymlinksInPath()
guard app.pathExtension == "app",
      FileManager.default.fileExists(atPath: app.appendingPathComponent("Contents/Info.plist").path)
else { fail("Expected an existing macOS app bundle") }

let icon: NSImage?
if args[0] == "--clear" {
    icon = nil
} else {
    guard let image = NSImage(contentsOfFile: args[0]), image.isValid else {
        fail("Cannot read the icon image")
    }
    icon = image
}
guard NSWorkspace.shared.setIcon(icon, forFile: app.path, options: []) else {
    fail("macOS could not update the custom icon")
}
print("Updated Finder custom icon: \(app.path)")
