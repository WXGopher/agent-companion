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
            if !isPresented { historyDemand = nil }
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
    @Published private(set) var appUpdate = AppUpdateSnapshot()
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
    private let clock: () -> Date
    private let updateBridge: AppUpdateBridging
    private let openReleaseURL: (URL) -> Void
    var usageCoordinator: SubscriptionUsageCoordinator
    private struct HistoryDemand {
        var source: SubscriptionSource?
        var pending = true
    }
    private var historyDemand: HistoryDemand?

    init(openTask: @escaping (CodexTask, CodexInstance, @escaping (String?) -> Void) -> Void = TerminalJump.open,
         usageBridge: SubscriptionUsageBridging = RustSubscriptionUsageBridge(),
         updateBridge: AppUpdateBridging = RustAppUpdateBridge(),
         openReleaseURL: @escaping (URL) -> Void = { _ = NSWorkspace.shared.open($0) },
         clock: @escaping () -> Date = Date.init) {
        self.openTask = openTask
        self.clock = clock
        self.updateBridge = updateBridge
        self.openReleaseURL = openReleaseURL
        usageCoordinator = SubscriptionUsageCoordinator(bridge: usageBridge)
        usageCoordinator.onChange = { [weak self] in self?.receiveUsageSnapshot() }
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
        if showingUsage { historyDemand?.pending = true }
        receiveUsageSnapshot()
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
        cachedWeeklyUsage(for: instance)?.usage
    }
    func cachedWeeklyUsage(for instance: CodexInstance) -> (usage: WeeklyUsage, readAt: Date)? {
        usageCoordinator.cachedWeeklyUsage(for: instance.usageSource)
    }
    var weeklyRemainingPercent: Int? { weeklyRemainingPercent(for: selectedInstance) }
    func weeklyRemainingPercent(for instance: CodexInstance) -> Int? {
        MenuBarUsage.remaining(weeklyUsage(for: instance), at: clock())
    }
    var weeklyText: String { weeklyText(for: selectedInstance) }
    func weeklyText(for instance: CodexInstance) -> String {
        guard let remaining = weeklyRemainingPercent(for: instance) else { return "—" }
        let failed = usageCoordinator.state(for: instance.usageSource)?.limits.error != nil
        return "\(remaining)%\(failed ? "*" : "")"
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
    }

    func showUsage() {
        let entering = !showingUsage
        present?()
        showingUsage = true
        if entering || historyDemand == nil { historyDemand = HistoryDemand() }
        receiveUsageSnapshot()
    }

    func showUsage(for instanceID: String) {
        guard instances.contains(where: { $0.id == instanceID }) else { return }
        let changed = selectedInstanceID != instanceID
        let alreadyShowing = showingUsage
        selectedInstanceID = instanceID
        if alreadyShowing && changed { historyDemand?.pending = true }
        showUsage()
    }

    func showTasks() {
        showingUsage = false
        historyDemand = nil
    }

    func receiveUsageSnapshot() {
        let state = usageCoordinator.state(for: selectedInstance.usageSource)
        subscriptionUsage = state?.usage ?? SubscriptionUsage()
        usageLoading = state?.limits.loading ?? false
        fulfillHistoryDemand(state)
    }

    private func fulfillHistoryDemand(_ state: SubscriptionInstanceSnapshot?) {
        guard isPresented && showingUsage, var demand = historyDemand else { return }
        guard let state else {
            // Keep the page-entry/source-change intent until the shared service
            // has registered that exact source. This does not check cache age.
            historyDemand?.pending = true
            return
        }
        guard demand.pending || demand.source != state.source else { return }
        demand.pending = false
        demand.source = state.source
        historyDemand = demand
        // Store the fulfilled source first: the returned snapshot may notify
        // observers synchronously, including this same model.
        usageCoordinator.loadHistory(instanceID: state.source.instanceID)
    }

    func refreshUsage() {
        usageCoordinator.refreshQuota(instanceID: selectedInstance.id)
        receiveUsageSnapshot()
    }

    func refreshUsageSnapshot() {
        usageCoordinator.refreshSnapshot()
        receiveUsageSnapshot()
    }

    func panelOpenedForUpdates() {
        updateBridge.panelOpened()
        refreshUpdateSnapshot()
    }

    func refreshUpdateSnapshot() {
        guard isPresented, let value = updateBridge.snapshot(), value != appUpdate else { return }
        appUpdate = value
    }

    func viewRelease() {
        guard let release = appUpdate.release else { return }
        openReleaseURL(release.url)
    }

    func refresh() {
        if let pointer = readSnapshot() {
            defer { releaseSnapshot(pointer) }
            do {
                snapshot = try JSONDecoder().decode(CodexSnapshot.self, from: Data(String(cString: pointer).utf8))
                dashboardError = nil
            } catch {
                dashboardError = "Could not read the local Codex dashboard. Try reopening Agent Companion."
                snapshot.loading = false
            }
        }
        if !instances.contains(where: { $0.id == selectedInstanceID }) {
            selectedInstanceID = instances[0].id
        }
        // Polling never refreshes quota or ages a history cache. It can fulfill
        // a pending page-entry/source-change intent once registration arrives.
        refreshUsageSnapshot()
        // Poll only the already published release state while the panel is open.
        // Eligibility and network requests belong to the explicit opening event.
        refreshUpdateSnapshot()
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
