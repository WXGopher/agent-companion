// SPDX-License-Identifier: GPL-3.0-only
import Darwin
import Foundation

/// This helper owns only the child it created. Cancellation kills it promptly,
/// including on app shutdown, rather than leaving an app-server behind.
private final class UsageCancellation {
    private let lock = NSLock()
    private var cancelled = false
    private var process: Process?
    var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelled }

    func launch(_ child: Process) throws {
        lock.lock(); defer { lock.unlock() }
        guard !cancelled else { throw UsageReadError.cancelled }
        try child.run()
        process = child
    }
    func detach() { lock.lock(); process = nil; lock.unlock() }
    func cancel() {
        lock.lock(); defer { lock.unlock() }
        cancelled = true
        if let process, process.isRunning { kill(process.processIdentifier, SIGKILL) }
    }
}

private enum UsageReadError: Error {
    case cancelled, timeout, closed, malformed, oversized
}

private struct UsageRPCReply {
    let result: Data?
    let errorCode: Int?

    func decode<T: Decodable>(_ type: T.Type) -> T? {
        guard errorCode == nil, let result else { return nil }
        return try? JSONDecoder().decode(type, from: result)
    }
    func message(for section: String) -> String {
        if errorCode == -32601 || errorCode == -32600 {
            return "Update Codex CLI to read \(section). This version does not support the account usage interface."
        }
        return "Codex could not read \(section). Check your subscription login and connection, then refresh."
    }
}

/// A bounded JSON-lines client for three read-only account methods. Stderr and
/// unknown notifications are never logged; auth files are never opened here.
private final class UsageRPCSession {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private let cancellation: UsageCancellation
    private let deadline: TimeInterval
    private var buffer = Data()
    private var received = 0
    private var replies: [Int: UsageRPCReply] = [:]

    init(executable: URL, codexHome: String, cancellation: UsageCancellation, timeout: TimeInterval) throws {
        self.cancellation = cancellation
        deadline = ProcessInfo.processInfo.systemUptime + timeout
        process.executableURL = executable
        process.arguments = ["app-server", "--listen", "stdio://", "-c", "analytics.enabled=false"]
        var environment = ProcessInfo.processInfo.environment
        if !codexHome.isEmpty { environment["CODEX_HOME"] = codexHome }
        process.environment = environment
        // An editor's working directory must not select an unrelated project's
        // local Codex configuration. No shell or shell startup files are used.
        process.currentDirectoryURL = FileManager.default.homeDirectoryForCurrentUser
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try cancellation.launch(process)
        try? input.fileHandleForReading.close()
        try? output.fileHandleForWriting.close()
        // Avoid terminating the GUI with SIGPIPE if an old CLI exits early.
        _ = fcntl(input.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)
    }

    func close() {
        try? input.fileHandleForWriting.close()
        if process.isRunning { process.terminate() }
        let end = ProcessInfo.processInfo.systemUptime + 0.3
        while process.isRunning && ProcessInfo.processInfo.systemUptime < end { Thread.sleep(forTimeInterval: 0.01) }
        if process.isRunning { kill(process.processIdentifier, SIGKILL) }
        process.waitUntilExit()
        cancellation.detach()
        try? output.fileHandleForReading.close()
    }

    func send(_ message: [String: Any]) throws {
        guard !cancellation.isCancelled else { throw UsageReadError.cancelled }
        var data = try JSONSerialization.data(withJSONObject: message)
        data.append(10)
        try input.fileHandleForWriting.write(contentsOf: data)
    }

    func receive(_ id: Int) throws -> UsageRPCReply {
        while true {
            if cancellation.isCancelled { throw UsageReadError.cancelled }
            if let reply = replies.removeValue(forKey: id) { return reply }
            if ProcessInfo.processInfo.systemUptime >= deadline { throw UsageReadError.timeout }
            while let newline = buffer.firstIndex(of: 10) {
                let line = buffer.prefix(upTo: newline)
                buffer.removeSubrange(...newline)
                guard !line.isEmpty else { continue }
                guard let value = try JSONSerialization.jsonObject(with: line) as? [String: Any] else {
                    throw UsageReadError.malformed
                }
                guard let responseID = value["id"] as? Int, (1...4).contains(responseID) else { continue }
                let code = (value["error"] as? [String: Any])?["code"] as? Int
                let result = try value["result"].map { try JSONSerialization.data(withJSONObject: $0, options: .fragmentsAllowed) }
                replies[responseID] = UsageRPCReply(result: result, errorCode: code)
            }
            if replies[id] != nil { continue }
            var descriptor = pollfd(fd: output.fileHandleForReading.fileDescriptor, events: Int16(POLLIN), revents: 0)
            let ready = poll(&descriptor, 1, 100)
            if ready < 0 { if errno == EINTR { continue }; throw UsageReadError.closed }
            if ready == 0 { continue }
            var bytes = [UInt8](repeating: 0, count: 65536)
            let count = Darwin.read(descriptor.fd, &bytes, bytes.count)
            if count < 0 { if errno == EINTR { continue }; throw UsageReadError.closed }
            guard count > 0 else { throw UsageReadError.closed }
            received += count
            guard received <= 8 * 1024 * 1024 else { throw UsageReadError.oversized }
            buffer.append(contentsOf: bytes.prefix(count))
        }
    }
}

final class CodexSubscriptionReader: SubscriptionReading {
    private let executable: () -> URL?
    private let timeout: TimeInterval
    private var cancellation: UsageCancellation?

    init(executable: @escaping () -> URL? = CodexSubscriptionReader.findExecutable, timeout: TimeInterval = 20) {
        self.executable = executable
        self.timeout = timeout
    }

    static func findExecutable() -> URL? {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let paths = ProcessInfo.processInfo.environment["PATH", default: ""].split(separator: ":")
            .filter { $0.hasPrefix("/") }.map { String($0) + "/codex" }
        let candidates = paths + [home.appendingPathComponent(".local/bin/codex").path,
            "/opt/homebrew/bin/codex", "/usr/local/bin/codex",
            "/Applications/Codex.app/Contents/Resources/codex",
            home.appendingPathComponent("Applications/Codex.app/Contents/Resources/codex").path]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }.map { URL(fileURLWithPath: $0) }
    }

    func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void) {
        cancel()
        guard let executable = executable() else {
            completion(.failure("Install Codex CLI or Codex.app, then sign in with your ChatGPT subscription."))
            return
        }
        let cancellation = UsageCancellation()
        self.cancellation = cancellation
        let timeout = timeout
        DispatchQueue.global(qos: .utility).async {
            let result = Self.fetch(executable: executable, codexHome: codexHome, cancellation: cancellation, timeout: timeout)
            DispatchQueue.main.async {
                guard !cancellation.isCancelled else { return }
                completion(result)
            }
        }
    }

    func cancel() { cancellation?.cancel(); cancellation = nil }
    deinit { cancel() }

    private static func fetch(executable: URL, codexHome: String, cancellation: UsageCancellation, timeout: TimeInterval) -> SubscriptionUsage {
        struct AccountResponse: Decodable {
            struct Account: Decodable { let type: String }
            let account: Account?
        }
        do {
            let session = try UsageRPCSession(executable: executable, codexHome: codexHome, cancellation: cancellation, timeout: timeout)
            defer { session.close() }
            try session.send(["id": 1, "method": "initialize", "params": [
                "clientInfo": ["name": "agent_companion_usage", "version": "1"],
                "capabilities": ["experimentalApi": true]]])
            let initialized = try session.receive(1)
            guard initialized.errorCode == nil, initialized.result != nil else {
                return .failure("Update Codex CLI to read subscription usage.")
            }
            try session.send(["method": "initialized"])
            try session.send(["id": 2, "method": "account/read", "params": ["refreshToken": false]])
            guard let account = try session.receive(2).decode(AccountResponse.self) else {
                return .failure("Could not read the Codex login. Check Codex CLI and refresh.")
            }
            guard account.account?.type == "chatgpt" else {
                return .failure("Sign in to Codex with your ChatGPT subscription, then refresh. Subscription usage does not use an API key.")
            }
            // Read quotas first so they remain useful if the token endpoint is
            // unavailable on an older CLI or takes too long to respond.
            try session.send(["id": 3, "method": "account/rateLimits/read"])
            try session.send(["id": 4, "method": "account/usage/read"])
            var result = SubscriptionUsage(readAt: Date())
            do {
                let limits = try session.receive(3)
                result.limits = limits.decode(SubscriptionLimits.self)
                if result.limits == nil { result.limitsError = limits.message(for: "subscription limits") }
                let tokens = try session.receive(4)
                result.tokens = tokens.decode(SubscriptionTokens.self)
                if result.tokens == nil { result.tokenError = tokens.message(for: "token activity") }
            } catch {
                let message = "The usage read did not finish. Check your connection and refresh."
                if result.limits == nil { result.limitsError = message }
                result.tokenError = message
            }
            return result
        } catch UsageReadError.timeout {
            return .failure("Codex did not respond in time. Check your connection and refresh.")
        } catch {
            return .failure("Could not read subscription usage. Check Codex CLI and your subscription login, then refresh.")
        }
    }
}
