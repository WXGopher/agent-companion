// SPDX-License-Identifier: GPL-3.0-only
import AppKit
import SwiftUI

@_silgen_name("agent_companion_snapshot_json")
private func readSnapshot() -> UnsafeMutablePointer<CChar>?
@_silgen_name("agent_companion_release_json")
private func releaseSnapshot(_ pointer: UnsafeMutablePointer<CChar>?)

struct CodexTask: Decodable, Identifiable {
    let id: String
    let title: String
    let project: String
    let cwd: String?
    let client: String
    let state: String
    let updatedAt: TimeInterval
    let transcriptPath: String?

    var isActive: Bool { state == "running" || state == "waiting" }
    var symbol: String {
        switch state {
        case "running": return "circle.inset.filled"
        case "waiting": return "pause.circle.fill"
        case "failed": return "exclamationmark.circle"
        case "stopped": return "stop.circle"
        default: return "checkmark.circle"
        }
    }
    var status: String {
        switch state {
        case "running": return "Running"
        case "waiting": return "Needs input"
        case "failed": return "Failed"
        case "stopped": return "Stopped"
        default: return "Completed"
        }
    }
    var tint: Color {
        if state == "waiting" || state == "failed" { return .orange }
        return isActive ? CompanionModel.accent : .white.opacity(0.55)
    }
    var resumeCommand: String? {
        guard UUID(uuidString: id) != nil else { return nil }
        return "codex resume \(id)"
    }
}

struct WeeklyUsage: Decodable {
    let usedPercent: Int
    let resetsAt: TimeInterval?
    let expired: Bool
}

struct CodexSnapshot: Decodable {
    var activeCount = 0
    var completedCount = 0
    var tasks: [CodexTask] = []
    var weekly: WeeklyUsage?
    var error: String?
    var codexHome = ""
    var updatedAt: TimeInterval = 0
    var loading = true
}

final class CompanionModel: ObservableObject {
    static let accent = Color(red: 0.45, green: 0.90, blue: 0.74)
    @Published var snapshot = CodexSnapshot()
    @Published var expanded = false {
        didSet { if expanded != oldValue { presentationRevision &+= 1 } }
    }
    @Published var showingCompleted = false
    @Published var cameraWidth: CGFloat = 0
    @Published var compactHeight: CGFloat = 28
    @Published var message: String?
    @Published var failedTask: CodexTask?
    @Published var jumpingID: String?
    var expand: (() -> Void)?
    var collapse: (() -> Void)?
    var quit: (() -> Void)?
    private var timer: Timer?
    private var settingsProcess: Process?
    private var presentationRevision: UInt64 = 0
    private let openTask: (CodexTask, String, @escaping (String?) -> Void) -> Void

    init(openTask: @escaping (CodexTask, String, @escaping (String?) -> Void) -> Void = TerminalJump.open) {
        self.openTask = openTask
    }

    var hasCamera: Bool { cameraWidth > 0 }
    // Keep both sides equally wide so the clear gap stays over the camera.
    // Grow only when larger counts need the space, not when the list opens.
    var sideWidth: CGFloat {
        let font = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .semibold)
        let labels = [countText(snapshot.activeCount), countText(snapshot.completedCount)]
        let textWidth = labels.reduce(CGFloat.zero) { width, label in
            width + (label as NSString).size(withAttributes: [.font: font]).width
        }
        return max(48, ceil(textWidth + 29))
    }
    var compactWidth: CGFloat { hasCamera ? cameraWidth + 2 * (sideWidth + 6) : 216 }
    func countText(_ count: Int) -> String { count > 99 ? "99+" : "\(count)" }
    var weeklyText: String {
        guard let usage = snapshot.weekly, !usage.expired else { return "—" }
        return "\(usage.usedPercent)%"
    }
    var visibleTasks: [CodexTask] {
        snapshot.tasks.filter { showingCompleted ? !$0.isActive : $0.isActive }
    }

    func start() {
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.refresh() }
    }
    func stop() { timer?.invalidate(); timer = nil }

    func refresh() {
        guard let pointer = readSnapshot() else { return }
        defer { releaseSnapshot(pointer) }
        do {
            snapshot = try JSONDecoder().decode(CodexSnapshot.self, from: Data(String(cString: pointer).utf8))
        } catch {
            snapshot.error = "Could not read the local Codex dashboard. Try reopening Agent Companion."
            snapshot.loading = false
        }
    }

    func openSettings() {
        if let process = settingsProcess, process.isRunning {
            NSRunningApplication(processIdentifier: process.processIdentifier)?.activate(options: [.activateAllWindows])
            collapse?()
            return
        }
        guard let executable = Bundle.main.executableURL else { return }
        let process = Process()
        process.executableURL = executable
        process.arguments = ["codex-tui"]
        do {
            try process.run()
            settingsProcess = process
            collapse?()
        } catch { message = "Could not open settings: \(error.localizedDescription)" }
    }

    func openCodex() {
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.openai.codex") else {
            message = "Codex.app is not installed. Start Codex CLI in your terminal to see its tasks here."
            return
        }
        let revision = presentationRevision
        NSWorkspace.shared.openApplication(at: url, configuration: .init()) { [weak self] _, error in
            DispatchQueue.main.async {
                guard let self, self.presentationRevision == revision else { return }
                if let error { self.message = "Could not open Codex: \(error.localizedDescription)" }
                else { self.collapse?() }
            }
        }
    }

    func jump(to task: CodexTask) {
        guard jumpingID == nil else { return }
        jumpingID = task.id
        message = nil
        failedTask = nil
        let revision = presentationRevision
        openTask(task, snapshot.codexHome) { [weak self] error in
            guard let self else { return }
            jumpingID = nil
            // A slow terminal/Automation response belongs to the presentation
            // that started it, not a panel the user has dismissed or reopened.
            guard presentationRevision == revision else { return }
            if let error {
                message = error
                failedTask = task
            } else { collapse?() }
        }
    }

    func copyResume(_ task: CodexTask) {
        guard let command = task.resumeCommand else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(command, forType: .string)
        message = "Copied resume command. Paste it into your terminal when ready."
        failedTask = nil
    }
}
