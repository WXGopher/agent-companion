// SPDX-License-Identifier: GPL-3.0-only
// Terminal/iTerm selection adapted from Open Island's TerminalJumpService.
// Scripts use argv instead of interpolating session data into AppleScript.
// See THIRD_PARTY_NOTICES.md for attribution and the pinned upstream revision.
import AppKit
import Carbon

enum TerminalJump {
    private static let queue = DispatchQueue(label: "agent-companion.jump", qos: .userInitiated)

    static func open(_ task: CodexTask, instance: CodexInstance, completion: @escaping (String?) -> Void) {
        guard task.sourceID == instance.id, UUID(uuidString: task.conversationID) != nil else {
            completion("This session has no valid Codex conversation ID.")
            return
        }
        // Paginated desktop history can omit originator metadata. A local
        // Codex conversation link still selects the exact stored thread.
        if task.client != "cli" {
            openConversation(task.conversationID, instance: instance, completion: completion)
            return
        }
        queue.async {
            let target = terminalTarget(task, home: instance.codexHome)
            guard let target else {
                DispatchQueue.main.async {
                    completion("The original terminal could not be located. It may have closed. You can copy a command to resume this session.")
                }
                return
            }
            let script: String?
            switch target.bundle {
            case "com.apple.Terminal": script = terminalScript
            case "com.googlecode.iterm2": script = iTermScript
            default: script = nil
            }
            if let script {
                let result = run("/usr/bin/osascript", ["-e", script, target.tty], timeout: 12)
                DispatchQueue.main.async {
                    if result.status == 0 && result.text.trimmingCharacters(in: .whitespacesAndNewlines) == "matched" {
                        completion(nil)
                    } else {
                        completion("Could not select the original terminal tab. Allow Agent Companion in System Settings → Privacy & Security → Automation, or copy the resume command.")
                    }
                }
            } else {
                DispatchQueue.main.async {
                    // An app activation alone is not an exact session jump.
                    // Keep the panel open and say what happened instead of claiming success.
                    if let app = NSRunningApplication(processIdentifier: target.appPID) {
                        app.activate(options: [.activateAllWindows])
                    }
                    completion("Opened the host app. Exact tab selection is available for Terminal and iTerm2; use the resume command if needed.")
                }
            }
        }
    }

    private static func openConversation(_ id: String, instance: CodexInstance, completion: @escaping (String?) -> Void) {
        let applications = NSWorkspace.shared.runningApplications.filter { application in
            guard let path = instance.appPath, let url = application.bundleURL else { return false }
            return sameApplication(url.path, path)
        }
        guard applications.count == 1, let app = applications.first else {
            completion("The running \(instance.label) app could not be located. Open \(instance.label) from Applications, then try again.")
            return
        }
        // Both runtimes retain the official bundle identifier and signature.
        // Address the URL event to the exact PID; Launch Services can otherwise
        // pick a different running app with the same bundle identifier.
        let event = NSAppleEventDescriptor(eventClass: AEEventClass(kInternetEventClass),
            eventID: AEEventID(kAEGetURL), targetDescriptor: NSAppleEventDescriptor(processIdentifier: app.processIdentifier),
            returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
        event.setParam(NSAppleEventDescriptor(string: "codex://threads/\(id)"), forKeyword: AEKeyword(keyDirectObject))
        do {
            _ = try event.sendEvent(options: [.noReply], timeout: 5)
            app.activate(options: [.activateAllWindows])
            completion(nil)
        } catch {
            completion("Could not select the conversation in \(instance.label). Check Automation permission or copy its resume command.")
        }
    }

    static func sameApplication(_ lhs: String, _ rhs: String) -> Bool {
        URL(fileURLWithPath: lhs).resolvingSymlinksInPath().standardizedFileURL.path
            == URL(fileURLWithPath: rhs).resolvingSymlinksInPath().standardizedFileURL.path
    }

    static func resumeCommand(_ task: CodexTask, instance: CodexInstance) -> String? {
        let runtime = instance.executablePath ?? (instance.id == "codex" ? CodexSubscriptionReader.findExecutable()?.path : nil)
        guard task.sourceID == instance.id, UUID(uuidString: task.conversationID) != nil,
              instance.codexHome.hasPrefix("/"),
              let executable = runtime, executable.hasPrefix("/"),
              let database = instance.databasePath, database.hasPrefix("/"),
              FileManager.default.isExecutableFile(atPath: executable) else { return nil }
        func quote(_ value: String) -> String { "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        // Start a clean environment at execution time, including when pasted
        // into a different terminal with inherited auth or instance overrides.
        let environment = "/usr/bin/env -i HOME=\"$HOME\" PATH=\"$PATH\" TERM=\"${TERM:-xterm-256color}\""
        let arguments = InstanceEnvironment.configurationArguments(instance.usageSource) + ["resume", task.conversationID]
        return environment + " CODEX_HOME=" + quote(instance.codexHome) + " CODEX_SQLITE_HOME=" + quote(database)
            + " " + quote(executable) + " " + arguments.map(quote).joined(separator: " ")
    }

    struct TerminalTarget {
        let tty: String
        let bundle: String
        let appPID: pid_t
    }

    struct ProcessRow {
        let parent: Int32
        let tty: String
        let executable: String
    }

    struct HostApp {
        let pid: Int32
        let bundle: String
    }

    /// Only inspect process IDs, ancestry, tty and executable names. Never read
    /// process environments, credentials, terminal contents or shell history.
    private static func terminalTarget(_ task: CodexTask, home: String) -> TerminalTarget? {
        guard home.hasPrefix("/") else { return nil }
        let root = URL(fileURLWithPath: home).resolvingSymlinksInPath().standardizedFileURL.path
        var paths = [URL(fileURLWithPath: home).appendingPathComponent("thread-writer-locks/\(task.conversationID).lock").path]
        if let transcript = task.transcriptPath {
            let resolved = URL(fileURLWithPath: transcript).resolvingSymlinksInPath().standardizedFileURL.path
            if resolved.hasPrefix(root + "/") { paths.append(resolved) }
        }
        var writers = Set<Int32>()
        for path in paths where FileManager.default.fileExists(atPath: path) {
            guard URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL.path.hasPrefix(root + "/") else { continue }
            let result = run("/usr/sbin/lsof", ["-t", "--", path], timeout: 3)
            for line in result.text.split(separator: "\n") {
                if let pid = Int32(line) { writers.insert(pid) }
            }
        }
        guard !writers.isEmpty else { return nil }
        let listing = run("/bin/ps", ["-axo", "pid=,ppid=,tty=,comm="], timeout: 3)
        guard listing.status == 0 else { return nil }
        var processes: [Int32: ProcessRow] = [:]
        for line in listing.text.split(separator: "\n") {
            let fields = line.split(maxSplits: 3, whereSeparator: { $0.isWhitespace })
            guard fields.count == 4, let pid = Int32(fields[0]), let parent = Int32(fields[1]) else { continue }
            processes[pid] = ProcessRow(parent: parent, tty: String(fields[2]), executable: String(fields[3]))
        }
        let iTerm = NSRunningApplication.runningApplications(withBundleIdentifier: "com.googlecode.iterm2").first
        return processTarget(writers: writers, processes: processes, detachedITerm: iTerm.map {
            HostApp(pid: $0.processIdentifier, bundle: "com.googlecode.iterm2")
        }) { pid in
            guard let app = NSRunningApplication(processIdentifier: pid), let bundle = app.bundleIdentifier,
                  bundle != Bundle.main.bundleIdentifier, bundle != "com.openai.codex" else { return nil }
            return HostApp(pid: pid, bundle: bundle)
        }
    }

    /// Newer iTerm versions keep sessions in a detached iTermServer process,
    /// so their parent chain ends at launchd instead of the GUI application.
    /// The live writer's tty still selects the exact session in the script.
    static func processTarget(writers: Set<Int32>, processes: [Int32: ProcessRow],
                              detachedITerm: HostApp?, application: (Int32) -> HostApp?) -> TerminalTarget? {
        for writer in writers.sorted() {
            guard let origin = processes[writer], URL(fileURLWithPath: origin.executable).lastPathComponent.lowercased().contains("codex") else { continue }
            var pid = writer
            var tty = origin.tty
            for _ in 0..<24 {
                guard let process = processes[pid] else { break }
                if tty == "??" { tty = process.tty }
                let detachedServer = process.executable.contains("/iTerm2/")
                    && URL(fileURLWithPath: process.executable).lastPathComponent.hasPrefix("iTermServer-")
                if let app = application(pid) ?? (detachedServer ? detachedITerm : nil) {
                    guard tty != "??" && !tty.isEmpty else { break }
                    return TerminalTarget(tty: tty.hasPrefix("/dev/") ? tty : "/dev/\(tty)", bundle: app.bundle, appPID: app.pid)
                }
                if process.parent <= 1 || process.parent == pid { break }
                pid = process.parent
            }
        }
        return nil
    }

    private static func run(_ executable: String, _ arguments: [String], timeout: TimeInterval) -> (status: Int32, text: String) {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: executable)
        task.arguments = arguments
        let output = Pipe()
        task.standardOutput = output
        task.standardError = output
        do { try task.run() } catch { return (-1, error.localizedDescription) }
        let deadline = DispatchWorkItem { if task.isRunning { task.terminate() } }
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: deadline)
        let data = output.fileHandleForReading.readDataToEndOfFile()
        task.waitUntilExit()
        deadline.cancel()
        return (task.terminationStatus, String(decoding: data, as: UTF8.self))
    }

    private static let terminalScript = """
    on run argv
        set targetTTY to item 1 of argv
        tell application "Terminal"
            if not running then return "not-found"
            repeat with aWindow in windows
                repeat with aTab in tabs of aWindow
                    if (tty of aTab as text) is targetTTY then
                        set selected of aTab to true
                        set frontmost of aWindow to true
                        activate
                        return "matched"
                    end if
                end repeat
            end repeat
        end tell
        return "not-found"
    end run
    """

    private static let iTermScript = """
    on run argv
        set targetTTY to item 1 of argv
        tell application "iTerm"
            if not running then return "not-found"
            repeat with aWindow in windows
                repeat with aTab in tabs of aWindow
                    repeat with aSession in sessions of aTab
                        if (tty of aSession as text) is targetTTY then
                            select aWindow
                            tell aWindow to select aTab
                            select aSession
                            activate
                            return "matched"
                        end if
                    end repeat
                end repeat
            end repeat
        end tell
        return "not-found"
    end run
    """
}
