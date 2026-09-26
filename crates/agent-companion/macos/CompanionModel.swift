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
    var sessionId: String? = nil
    var instanceId: String? = nil
    var instanceLabel: String? = nil

    var conversationID: String { sessionId ?? id }
    var sourceID: String { instanceId ?? "codex" }
    var sourceLabel: String { instanceLabel ?? "Codex" }

    var isActive: Bool { state == "running" || state == "waiting" }
    var activity: TaskActivity { TaskActivity(state: state) }
    var symbol: String {
        switch state {
        case "running": return "circle.inset.filled"
        case "waiting": return "pause.circle.fill"
        case "failed": return "exclamationmark.circle"
        case "stopped": return "stop.circle"
        case "paused": return "pause.circle"
        case "completed": return "checkmark.circle"
        default: return "questionmark.circle"
        }
    }
    var status: String {
        switch state {
        case "running": return "Running"
        case "waiting": return "Needs input"
        case "failed": return "Failed"
        case "stopped": return "Stopped"
        case "paused": return "Paused"
        case "completed": return "Completed"
        default: return "Unknown"
        }
    }
    func tint(in palette: CompanionPalette) -> Color {
        activity.color(in: palette.theme)
    }
}

struct WeeklyUsage: Decodable {
    let usedPercent: Int
    let resetsAt: TimeInterval?
    let expired: Bool
}

struct CodexInstance: Decodable, Identifiable {
    var instanceId: String
    var label: String
    var codexHome: String
    var appPath: String? = nil
    var executablePath: String? = nil
    var databasePath: String? = nil
    var weekly: WeeklyUsage? = nil
    var error: String? = nil
    var id: String { instanceId }
    var usageSource: SubscriptionSource {
        SubscriptionSource(instanceID: instanceId, codexHome: codexHome,
                           executablePath: executablePath, databasePath: databasePath)
    }
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
    var instances: [CodexInstance]? = nil
}

final class CompanionModel: ObservableObject {
    @Published var snapshot = CodexSnapshot()
    @Published var isPresented = false {
        didSet {
            if isPresented != oldValue { presentationRevision &+= 1 }
            if !isPresented { stopUsage() }
        }
    }
    @Published var showingCompleted = false
    @Published var showingUsage = false
    @Published var selectedInstanceID = "codex"
    @Published var subscriptionUsage = SubscriptionUsage()
    @Published var usageLoading = false
    @Published var message: String?
    @Published var failedTask: CodexTask?
    @Published var jumpingID: String?
    @Published var dashboardError: String?
    @Published var animatesTaskActivity = false
    var present: (() -> Void)?
    var dismiss: (() -> Void)?
    var quit: (() -> Void)?
    var settingsAction: (() -> Void)?
    private var timer: Timer?
    private var settingsProcess: Process?
    private var settingsApplication: NSRunningApplication?
    private var openingSettings = false
    private var presentationRevision: UInt64 = 0
    private let openTask: (CodexTask, CodexInstance, @escaping (String?) -> Void) -> Void
    private let subscriptionMonitor: SubscriptionMonitor
    private let clock: () -> Date
    private var usageSource: SubscriptionSource?
    weak var usageCoordinator: SubscriptionUsageCoordinator?

    init(openTask: @escaping (CodexTask, CodexInstance, @escaping (String?) -> Void) -> Void = TerminalJump.open,
         usageReader: SubscriptionReading = CodexSubscriptionReader(), clock: @escaping () -> Date = Date.init) {
        self.openTask = openTask
        self.clock = clock
        subscriptionMonitor = SubscriptionMonitor(reader: usageReader, clock: clock)
        subscriptionMonitor.onChange = { [weak self] value, loading in
            if let value { self?.subscriptionUsage = value }
            self?.usageLoading = loading
        }
    }

    var instances: [CodexInstance] {
        if let instances = snapshot.instances, !instances.isEmpty { return instances }
        return [CodexInstance(instanceId: "codex", label: "Codex", codexHome: snapshot.codexHome,
                              weekly: snapshot.weekly, error: snapshot.error)]
    }
    var selectedInstance: CodexInstance {
        instances.first { $0.id == selectedInstanceID } ?? instances[0]
    }
    func selectInstance(_ id: String) {
        guard instances.contains(where: { $0.id == id }), selectedInstanceID != id else { return }
        selectedInstanceID = id
        subscriptionUsage = SubscriptionUsage()
        usageLoading = false
        refreshUsage()
    }
    var enabledTasks: [CodexTask] {
        let enabled = Set(instances.map(\.id))
        return snapshot.tasks.filter { enabled.contains($0.sourceID) }
    }
    var taskActivity: TaskActivity { TaskActivity.aggregate(enabledTasks) }
    var workingCount: Int { enabledTasks.filter { $0.activity == .running }.count }
    var needsInput: Bool { enabledTasks.contains { $0.activity == .waiting } }
    func countText(_ count: Int) -> String { count > 99 ? "99+" : "\(count)" }
    var weeklyUsage: WeeklyUsage? { weeklyUsage(for: selectedInstance) }
    func weeklyUsage(for instance: CodexInstance) -> WeeklyUsage? {
        cachedWeeklyUsage(for: instance)?.usage ?? instance.weekly
    }
    func cachedWeeklyUsage(for instance: CodexInstance) -> (usage: WeeklyUsage, readAt: Date)? {
        if let usageCoordinator {
            guard let cached = usageCoordinator.cachedWeeklyUsage(for: instance.usageSource),
                  clock().timeIntervalSince(cached.readAt) >= 0,
                  clock().timeIntervalSince(cached.readAt) < SubscriptionMonitor.refreshInterval,
                  MenuBarUsage.remaining(cached.usage, at: clock()) != nil else { return nil }
            return cached
        }
        // Each card uses only its own account cache or local snapshot.
        let hasCurrentReading = usageSource == instance.usageSource || (usageSource == nil && subscriptionUsage.readAt != nil)
        let usage = instance.id == selectedInstance.id && hasCurrentReading
            ? subscriptionUsage : subscriptionMonitor.cachedUsage(for: instance.usageSource)
        if let readAt = usage?.readAt, clock().timeIntervalSince(readAt) >= 0,
           clock().timeIntervalSince(readAt) < SubscriptionMonitor.refreshInterval,
           let limits = usage?.limits,
           let bucket = limits.buckets.first(where: { $0.id == "codex" }),
           let window = [bucket.value.primary, bucket.value.secondary].compactMap({ $0 })
               .first(where: { $0.windowDurationMins == 10080 }) {
            return (WeeklyUsage(usedPercent: window.usedPercent, resetsAt: window.resetsAt,
                                expired: window.remaining(at: clock()) == nil), readAt)
        }
        return nil
    }
    var weeklyRemainingPercent: Int? { weeklyRemainingPercent(for: selectedInstance) }
    func weeklyRemainingPercent(for instance: CodexInstance) -> Int? {
        guard let usage = weeklyUsage(for: instance), !usage.expired else { return nil }
        return 100 - min(100, max(0, usage.usedPercent))
    }
    var weeklyText: String { weeklyText(for: selectedInstance) }
    func weeklyText(for instance: CodexInstance) -> String {
        guard let remaining = weeklyRemainingPercent(for: instance) else { return "—" }
        return "\(remaining)%"
    }
    var visibleTasks: [CodexTask] {
        enabledTasks.filter { showingCompleted ? !$0.isActive : $0.isActive }
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
        stopUsage()
    }

    func showUsage() {
        present?()
        showingUsage = true
        refreshUsage()
    }

    func showUsage(for instanceID: String) {
        guard instances.contains(where: { $0.id == instanceID }) else { return }
        selectedInstanceID = instanceID
        showUsage()
    }

    func showTasks() {
        showingUsage = false
        stopUsage()
    }

    private func stopUsage() {
        if let usageCoordinator { usageCoordinator.release(owner: self) }
        else { subscriptionMonitor.stop() }
        usageLoading = false
    }

    func receiveUsage(source: SubscriptionSource, value: SubscriptionUsage?, loading: Bool) {
        guard source == usageSource else { return }
        if let value { subscriptionUsage = value }
        usageLoading = isPresented && showingUsage && loading
    }

    func refreshUsage(force: Bool = false) {
        let source = selectedInstance.usageSource
        if usageSource != source {
            usageSource = source
            subscriptionUsage = SubscriptionUsage()
            usageLoading = false
        }
        if let usageCoordinator { usageCoordinator.refresh(source: source, owner: self, force: force) }
        else { subscriptionMonitor.refresh(source: source, force: force) }
    }

    func refresh() {
        guard let pointer = readSnapshot() else { return }
        defer { releaseSnapshot(pointer) }
        do {
            snapshot = try JSONDecoder().decode(CodexSnapshot.self, from: Data(String(cString: pointer).utf8))
            dashboardError = nil
        } catch {
            dashboardError = "Could not read the local Codex dashboard. Try reopening Agent Companion."
            snapshot.loading = false
        }
        subscriptionMonitor.retainSources(Set(instances.map(\.usageSource)))
        if !instances.contains(where: { $0.id == selectedInstanceID }) {
            selectedInstanceID = instances[0].id
            subscriptionUsage = SubscriptionUsage()
        }
        if let usageSource, usageSource != selectedInstance.usageSource {
            self.usageSource = nil
            subscriptionUsage = SubscriptionUsage()
            usageLoading = false
        }
        if isPresented && showingUsage { refreshUsage() }
    }

    func openSettings() {
        if let settingsAction { settingsAction(); return }
        if let application = settingsApplication, !application.isTerminated {
            application.activate(options: [.activateAllWindows])
            dismiss?()
            return
        }
        guard !openingSettings else { return }
        if let process = settingsProcess, process.isRunning {
            NSRunningApplication(processIdentifier: process.processIdentifier)?.activate(options: [.activateAllWindows])
            dismiss?()
            return
        }
        guard let executable = Bundle.main.executableURL else { return }
        let environment = ProcessInfo.processInfo.environment
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
                    else { self.dismiss?() }
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
            dismiss?()
        } catch { message = "Could not open settings: \(error.localizedDescription)" }
    }

    func openCodex() {
        guard let path = instances.first(where: { $0.id == "codex" })?.appPath,
              FileManager.default.fileExists(atPath: path) else {
            message = "Codex.app is not installed. Start Codex CLI in your terminal to see its tasks here."
            return
        }
        let url = URL(fileURLWithPath: path)
        let revision = presentationRevision
        NSWorkspace.shared.openApplication(at: url, configuration: .init()) { [weak self] _, error in
            DispatchQueue.main.async {
                guard let self, self.presentationRevision == revision else { return }
                if let error { self.message = "Could not open Codex: \(error.localizedDescription)" }
                else { self.dismiss?() }
            }
        }
    }

    func jump(to task: CodexTask) {
        guard jumpingID == nil else { return }
        guard let instance = instances.first(where: { $0.id == task.sourceID }) else {
            message = "This task's instance is no longer enabled. Check its deployment in Settings."
            return
        }
        jumpingID = task.id
        message = nil
        failedTask = nil
        let revision = presentationRevision
        openTask(task, instance) { [weak self] error in
            guard let self else { return }
            jumpingID = nil
            // A slow terminal/Automation response belongs to the presentation
            // that started it, not a panel the user has dismissed or reopened.
            guard presentationRevision == revision else { return }
            if let error {
                message = error
                failedTask = task
            } else { dismiss?() }
        }
    }

    func copyResume(_ task: CodexTask) {
        guard let command = resumeCommand(for: task) else {
            message = "The exact Codex runtime for this task could not be located. Check its deployment in Settings."
            return
        }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(command, forType: .string)
        message = "Copied resume command. Paste it into your terminal when ready."
        failedTask = nil
    }

    func resumeCommand(for task: CodexTask) -> String? {
        guard let instance = instances.first(where: { $0.id == task.sourceID }) else { return nil }
        return TerminalJump.resumeCommand(task, instance: instance)
    }
}
