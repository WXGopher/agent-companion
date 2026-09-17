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
        didSet {
            if expanded != oldValue { presentationRevision &+= 1 }
            if !expanded { subscriptionMonitor.stop(); usageLoading = false }
        }
    }
    @Published var showingCompleted = false
    @Published var showingUsage = false
    @Published var subscriptionUsage = SubscriptionUsage()
    @Published var usageLoading = false
    @Published var metrics = NotchMetrics()
    @Published var message: String?
    @Published var failedTask: CodexTask?
    @Published var jumpingID: String?
    var expand: (() -> Void)?
    var collapse: (() -> Void)?
    var quit: (() -> Void)?
    private var timer: Timer?
    private var settingsProcess: Process?
    private var settingsApplication: NSRunningApplication?
    private var openingSettings = false
    private var presentationRevision: UInt64 = 0
    private let openTask: (CodexTask, String, @escaping (String?) -> Void) -> Void
    private let subscriptionMonitor: SubscriptionMonitor

    init(openTask: @escaping (CodexTask, String, @escaping (String?) -> Void) -> Void = TerminalJump.open,
         usageReader: SubscriptionReading = CodexSubscriptionReader()) {
        self.openTask = openTask
        subscriptionMonitor = SubscriptionMonitor(reader: usageReader)
        subscriptionMonitor.onChange = { [weak self] value, loading in
            if let value { self?.subscriptionUsage = value }
            self?.usageLoading = loading
        }
    }

    var hasCamera: Bool { metrics.hasCamera }
    var workingCount: Int { snapshot.tasks.filter { $0.state == "running" }.count }
    var needsInput: Bool { snapshot.tasks.contains { $0.state == "waiting" } }
    var quotaTint: Color {
        guard let remaining = weeklyRemainingPercent else { return .white.opacity(0.55) }
        return remaining <= 10 ? .orange : Self.accent
    }
    var summaryDescription: String {
        "Codex: \(workingCount) working\(needsInput ? ", approval or input needed" : ""). Weekly quota remaining: \(weeklyText)"
    }
    private func cameraWingWidth(text: String, accessories: CGFloat = 0) -> CGFloat {
        guard hasCamera else { return 0 }
        let scale = metrics.cameraContentScale
        let font = NSFont.monospacedDigitSystemFont(ofSize: 10 * scale, weight: .semibold)
        let textWidth = (text as NSString).size(withAttributes: [.font: font]).width
        // Leave room for the outer curve and the edge of the camera cutout.
        return ceil(max(metrics.cameraSideWidth, textWidth + (accessories + 12) * scale))
    }
    var cameraLeftWidth: CGFloat { cameraWingWidth(text: weeklyText) }
    var cameraRightWidth: CGFloat {
        // Reserve only the working count, its icon and an optional question
        // mark. Waiting sessions remain in Active but are not still working.
        cameraWingWidth(text: countText(workingCount), accessories: needsInput ? 18 : 8)
    }
    var compactWidth: CGFloat { hasCamera ? cameraLeftWidth + metrics.cameraWidth + cameraRightWidth : metrics.width }
    var centerOffset: CGFloat {
        // Unequal wings reclaim menu space without shifting the camera gap.
        metrics.centerOffset + (cameraRightWidth - cameraLeftWidth) / 2
    }
    var compactHeight: CGFloat { metrics.compactHeight }
    func countText(_ count: Int) -> String { count > 99 ? "99+" : "\(count)" }
    var weeklyUsage: WeeklyUsage? {
        // A fresh account read also updates the compact strip. Otherwise keep
        // the existing local-log source, which does not require a network read.
        if let readAt = subscriptionUsage.readAt, Date().timeIntervalSince(readAt) < 300,
           let limits = subscriptionUsage.limits,
           let bucket = limits.buckets.first(where: { $0.id == "codex" }),
           let window = [bucket.value.primary, bucket.value.secondary].compactMap({ $0 })
               .first(where: { $0.windowDurationMins == 10080 }) {
            return WeeklyUsage(usedPercent: window.usedPercent, resetsAt: window.resetsAt,
                               expired: window.remaining() == nil)
        }
        return snapshot.weekly
    }
    var weeklyRemainingPercent: Int? {
        guard let usage = weeklyUsage, !usage.expired else { return nil }
        return 100 - min(100, max(0, usage.usedPercent))
    }
    var weeklyText: String {
        guard let remaining = weeklyRemainingPercent else { return "—" }
        return "\(remaining)%"
    }
    var visibleTasks: [CodexTask] {
        snapshot.tasks.filter { showingCompleted ? !$0.isActive : $0.isActive }
    }

    func start() {
        guard timer == nil else { return }
        refresh()
        let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in self?.refresh() }
        // Scrolling and native menus use AppKit's event-tracking mode.
        // Keep task counts and quota live while those controls are in use.
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }
    func stop() {
        timer?.invalidate(); timer = nil
        subscriptionMonitor.stop()
        usageLoading = false
    }

    func showUsage() {
        expand?()
        showingUsage = true
        refreshUsage()
    }

    func showTasks() {
        showingUsage = false
        subscriptionMonitor.stop()
        usageLoading = false
    }

    func refreshUsage(force: Bool = false) {
        subscriptionMonitor.refresh(codexHome: snapshot.codexHome, force: force)
    }

    func refresh() {
        guard let pointer = readSnapshot() else { return }
        defer { releaseSnapshot(pointer) }
        do {
            snapshot = try JSONDecoder().decode(CodexSnapshot.self, from: Data(String(cString: pointer).utf8))
        } catch {
            snapshot.error = "Could not read the local Codex dashboard. Try reopening Agent Companion."
            snapshot.loading = false
        }
        if expanded && showingUsage { refreshUsage() }
    }

    func openSettings() {
        if let application = settingsApplication, !application.isTerminated {
            application.activate(options: [.activateAllWindows])
            collapse?()
            return
        }
        guard !openingSettings else { return }
        if let process = settingsProcess, process.isRunning {
            NSRunningApplication(processIdentifier: process.processIdentifier)?.activate(options: [.activateAllWindows])
            collapse?()
            return
        }
        guard let executable = Bundle.main.executableURL else { return }
        var environment = ProcessInfo.processInfo.environment
        environment[DockPreferences.editorParentKey] = String(ProcessInfo.processInfo.processIdentifier)
        if Bundle.main.bundleURL.pathExtension == "app" {
            // Register the accessory editor with Launch Services so its window
            // can be activated even though it has no Dock tile of its own.
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.createsNewApplicationInstance = true
            configuration.arguments = ["codex-tui"]
            configuration.environment = environment
            openingSettings = true
            let revision = presentationRevision
            NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) { [weak self] application, error in
                DispatchQueue.main.async {
                    guard let self else { return }
                    self.openingSettings = false
                    self.settingsApplication = application
                    guard self.presentationRevision == revision else { return }
                    if let error { self.message = "Could not open settings: \(error.localizedDescription)" }
                    else { self.collapse?() }
                }
            }
            return
        }
        let process = Process()
        process.executableURL = executable
        process.arguments = ["codex-tui"]
        process.environment = environment
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
