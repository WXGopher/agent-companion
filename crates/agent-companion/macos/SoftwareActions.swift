// SPDX-License-Identifier: GPL-3.0-only
import AppKit

enum SoftwareAction: String, CaseIterable {
    case align
    case updateAll = "update-all"

    var title: String {
        switch self {
        case .align: return "对齐 Codex/Dodex 版本"
        case .updateAll: return "全部更新到最新"
        }
    }

    static let notification = Notification.Name("com.wxgopher.agent-companion.softwareUpdate")
    static let readyNotification = Notification.Name("com.wxgopher.agent-companion.softwareUpdateReady")

    func send(to processIdentifier: Int32) {
        DistributedNotificationCenter.default().postNotificationName(
            Self.notification, object: String(processIdentifier), userInfo: ["action": rawValue],
            deliverImmediately: true)
    }
}

@_silgen_name("agent_companion_request_software_update")
private func requestSoftwareUpdate(_ action: UnsafePointer<CChar>)

/// The editor owns this listener for its process lifetime. Registration is
/// idempotent; another editor's PID and unsupported actions never reach Rust.
private enum SoftwareActionListener {
    static var observer: NSObjectProtocol?

    static func start() {
        precondition(Thread.isMainThread)
        guard observer == nil else { return }
        observer = DistributedNotificationCenter.default().addObserver(
            forName: SoftwareAction.notification, object: String(ProcessInfo.processInfo.processIdentifier),
            queue: .main
        ) { notification in
            precondition(Thread.isMainThread)
            guard let value = notification.userInfo?["action"] as? String,
                  let action = SoftwareAction(rawValue: value) else { return }
            action.rawValue.withCString { requestSoftwareUpdate($0) }
        }
        // Process.run() and Launch Services can finish before this observer is
        // installed. The menu waits for this handshake before sending actions.
        DistributedNotificationCenter.default().postNotificationName(
            SoftwareAction.readyNotification, object: String(ProcessInfo.processInfo.processIdentifier),
            userInfo: nil, deliverImmediately: true)
    }
}

@_cdecl("agent_companion_listen_software_updates")
public func listenForAgentCompanionSoftwareUpdates() {
    SoftwareActionListener.start()
}

/// Settings and software actions share the editor launched by this menu app.
/// Keeping the process/application captured here also keeps its activation
/// handle alive without discovering or disturbing externally launched editors.
final class CompanionEditorLauncher {
    struct Editor {
        let processIdentifier: Int32
        let isRunning: () -> Bool
        let activate: () -> Void
    }

    typealias Completion = (String?) -> Void
    typealias Launch = ([String], @escaping (Result<Editor, Error>) -> Void) -> Void

    private let launch: Launch
    private let send: (SoftwareAction, Int32) -> Void
    private let readyCenter: NotificationCenter
    private let readinessTimeout: TimeInterval
    private var readyObserver: NSObjectProtocol?
    private var readyTimer: Timer?
    private var readyPIDs: Set<Int32> = []
    private var editor: Editor?
    private var editorReady = false
    private var opening = false
    private var pendingRequest: (SoftwareAction, Completion)?
    private var launchCompletions: [Completion] = []

    init(launch: @escaping Launch = CompanionEditorLauncher.launchEditor,
         send: @escaping (SoftwareAction, Int32) -> Void = { $0.send(to: $1) },
         readyCenter: NotificationCenter = DistributedNotificationCenter.default(),
         readinessTimeout: TimeInterval = 15) {
        self.launch = launch
        self.send = send
        self.readyCenter = readyCenter
        self.readinessTimeout = readinessTimeout
        readyObserver = readyCenter.addObserver(forName: SoftwareAction.readyNotification, object: nil, queue: .main) { [weak self] notification in
            guard let self, let value = notification.object as? String, let pid = Int32(value) else { return }
            precondition(Thread.isMainThread)
            if self.opening { self.readyPIDs.insert(pid) }
            if self.editor?.processIdentifier == pid {
                self.editorReady = true
                self.deliverPendingRequest()
            }
        }
    }

    deinit {
        readyTimer?.invalidate()
        if let readyObserver { readyCenter.removeObserver(readyObserver) }
    }

    func open(action: SoftwareAction? = nil, completion: @escaping Completion) {
        precondition(Thread.isMainThread)
        if let editor, editor.isRunning() {
            editor.activate()
            if let action, !editorReady {
                enqueue(action, completion: completion)
                waitForReadiness()
            } else {
                if let action { send(action, editor.processIdentifier) }
                completion(nil)
            }
            return
        }
        if opening {
            if let action { enqueue(action, completion: completion) }
            else { launchCompletions.append(completion) }
            return
        }
        failPendingRequest("The editor closed before it could receive the action. Try again.")
        editor = nil
        editorReady = false
        readyPIDs.removeAll()
        launchCompletions.append(completion)
        opening = true
        var arguments = ["codex-tui"]
        if let action { arguments += ["--software-action", action.rawValue] }
        launch(arguments) { [weak self] result in
            precondition(Thread.isMainThread)
            guard let self else { return }
            self.opening = false
            let callbacks = self.launchCompletions
            self.launchCompletions.removeAll()
            switch result {
            case .success(let editor):
                self.editor = editor
                self.editorReady = self.readyPIDs.contains(editor.processIdentifier)
                self.readyPIDs.removeAll()
                editor.activate()
                for callback in callbacks { callback(nil) }
                self.deliverPendingRequest()
            case .failure(let error):
                self.editor = nil
                self.failPendingRequest(error.localizedDescription)
                for callback in callbacks { callback(error.localizedDescription) }
            }
        }
    }

    private func enqueue(_ action: SoftwareAction, completion: @escaping Completion) {
        guard pendingRequest == nil else {
            completion("Another software action is already waiting for the editor. Try again when it finishes.")
            return
        }
        pendingRequest = (action, completion)
    }

    private func deliverPendingRequest() {
        guard let (action, completion) = pendingRequest else { return }
        guard let editor, editor.isRunning() else {
            failPendingRequest("The editor closed before it could receive the action. Try again.")
            return
        }
        guard editorReady else { waitForReadiness(); return }
        readyTimer?.invalidate()
        readyTimer = nil
        pendingRequest = nil
        send(action, editor.processIdentifier)
        completion(nil)
    }

    private func waitForReadiness() {
        guard readyTimer == nil else { return }
        let timer = Timer(timeInterval: readinessTimeout, repeats: false) { [weak self] _ in
            self?.failPendingRequest("The editor did not become ready to receive the action. Try again.")
        }
        readyTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private func failPendingRequest(_ message: String) {
        readyTimer?.invalidate()
        readyTimer = nil
        let request = pendingRequest
        pendingRequest = nil
        request?.1(message)
    }

    private static func launchEditor(arguments: [String], completion: @escaping (Result<Editor, Error>) -> Void) {
        guard let executable = Bundle.main.executableURL else {
            completion(.failure(NSError(domain: "AgentCompanion.Editor", code: 1,
                userInfo: [NSLocalizedDescriptionKey: "The Agent Companion executable could not be located."])))
            return
        }
        let environment = ProcessInfo.processInfo.environment
        if Bundle.main.bundleURL.pathExtension == "app" {
            // Launch Services lets an accessory editor without a Dock tile
            // receive activation when the menu later reuses its process.
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.createsNewApplicationInstance = true
            configuration.arguments = arguments
            configuration.environment = environment
            NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) { application, error in
                DispatchQueue.main.async {
                    if let application {
                        completion(.success(Editor(processIdentifier: application.processIdentifier,
                            isRunning: { !application.isTerminated },
                            activate: { application.activate(options: [.activateAllWindows]) })))
                    } else {
                        completion(.failure(error ?? NSError(domain: "AgentCompanion.Editor", code: 2,
                            userInfo: [NSLocalizedDescriptionKey: "The editor did not finish opening."])))
                    }
                }
            }
            return
        }
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        process.environment = environment
        do {
            try process.run()
            completion(.success(Editor(processIdentifier: process.processIdentifier,
                isRunning: { process.isRunning }, activate: {
                    NSRunningApplication(processIdentifier: process.processIdentifier)?.activate(options: [.activateAllWindows])
                })))
        } catch { completion(.failure(error)) }
    }
}
