// SPDX-License-Identifier: GPL-3.0-only
import SwiftUI

enum CompanionPopupLayout {
    static let width: CGFloat = 356
    static let height: CGFloat = 560
    static let headerHeight: CGFloat = 28
    static let contentScale: CGFloat = 0.9
}

struct CompanionPopupView: View {

    @ObservedObject var model: CompanionModel
    @ObservedObject var themePreferences: ThemePreferences = .shared
    var version = CompanionAppInfo.version
    @State private var hoveredTask: String?
    @State private var themeSaveFailed = false
    private var palette: CompanionPalette { themePreferences.theme.palette }

    // Snap layout dimensions to points so fractional density scaling cannot
    // change the viewport's rounding when an empty/populated tab is selected.
    private func scaled(_ value: CGFloat) -> CGFloat { (value * CompanionPopupLayout.contentScale).rounded() }
    private func fontSize(_ value: CGFloat) -> CGFloat { max(9, value * CompanionPopupLayout.contentScale) }
    private var taskViewportHeight: CGFloat {
        let active = model.enabledTasks.filter { $0.isActive }.count
        let finished = model.enabledTasks.count - active
        func height(_ count: Int) -> CGFloat { count == 0 ? 132 : min(CGFloat(count) * 58, 232) }
        // Both tabs share one viewport, so switching filters cannot resize the
        // panel or move the footer. Empty states need room for their guidance.
        return scaled(max(height(active), height(finished)))
    }

    var body: some View {
        VStack(spacing: 0) {
            applicationHeader(version: version)
            content
        }
        .frame(width: CompanionPopupLayout.width, height: CompanionPopupLayout.height, alignment: .top)
        .background(palette.background)
        .foregroundStyle(palette.primaryText)
        .tint(palette.accent)
        .environment(\.companionTheme, themePreferences.theme)
        .preferredColorScheme(themePreferences.theme.colorScheme)
        .onExitCommand { model.dismiss?() }
        .alert("Could not save theme", isPresented: $themeSaveFailed) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("Try changing the theme again.")
        }
    }

    private func applicationHeader(version: String) -> some View {
        let count = model.workingCount
        let status = "\(count) running \(count == 1 ? "task" : "tasks")"
        return HStack(spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(CompanionAppInfo.title)
                    .font(.system(size: 12, weight: .semibold)).fixedSize()
                    .accessibilityIdentifier("popup-app-title")
                Text("v" + version)
                    .font(.system(size: 10)).foregroundStyle(palette.secondaryText)
                    .lineLimit(1).minimumScaleFactor(0.8)
                    .accessibilityLabel("Version " + version)
                    .accessibilityIdentifier("popup-app-version")
                    .help("Version " + version)
            }
            Spacer(minLength: 0)
            HStack(spacing: 5) {
                TaskActivityMark(activity: model.taskActivity, animating: model.animatesTaskActivity)
                    .frame(width: 6)
                Text("\(count) running")
                    .font(.system(size: 11, weight: .medium)).monospacedDigit().fixedSize()
            }
            .foregroundStyle(count > 0 ? palette.primaryText : palette.mutedText)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(status)
            .accessibilityValue(model.taskActivity.description)
            .accessibilityIdentifier("popup-running-count")
            .help(status + " · " + model.taskActivity.description)
            if model.needsInput {
                Image(systemName: "questionmark")
                    .font(.system(size: 10, weight: .bold)).frame(width: 6)
                    .foregroundStyle(TaskActivity.waiting.color(in: themePreferences.theme))
                    .accessibilityLabel("Tasks need approval or input")
                    .accessibilityIdentifier("popup-needs-input")
                    .help("A task needs approval or input")
            }
        }
        .padding(.horizontal, scaled(16))
        .frame(width: CompanionPopupLayout.width, height: CompanionPopupLayout.headerHeight)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("popup-header")
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: scaled(13)) {
            pageNavigation
            pageContent
            Divider().overlay(palette.separator)
            footer
        }
        .padding(.horizontal, scaled(16)).padding(.top, scaled(12)).padding(.bottom, scaled(16))
        .frame(maxWidth: .infinity)
        .frame(height: CompanionPopupLayout.height - CompanionPopupLayout.headerHeight)
    }

    private var pageContent: some View {
        // Page selection is presentation state, not view identity or geometry.
        // Keep both trees and size to their shared maximum. Replacing a page
        // with `if` destroys its NSScrollView and can flash the footer.
        ZStack(alignment: .top) {
            pageViewport { taskContent }
                .opacity(model.showingUsage ? 0 : 1)
                .allowsHitTesting(!model.showingUsage)
                .accessibilityElement(children: .contain)
                .accessibilityHidden(model.showingUsage)
            pageViewport {
                SubscriptionUsageView(usage: model.subscriptionUsage, loading: model.usageLoading,
                                      scale: CompanionPopupLayout.contentScale, refresh: { model.refreshUsage(force: true) },
                                      instances: model.instances, selectedInstanceID: model.selectedInstanceID,
                                      selectInstance: model.selectInstance)
            }
                .opacity(model.showingUsage ? 1 : 0)
                .allowsHitTesting(model.showingUsage)
                .accessibilityElement(children: .contain)
                .accessibilityHidden(!model.showingUsage)
        }
        // Do not disable a hidden page: AppKit would remove its legacy
        // scrollers, reflow its text and clamp its saved scroll offset.
        // Page changes must never animate layout or flash the fixed footer.
        .transaction { $0.animation = nil; $0.disablesAnimations = true }
    }

    @ViewBuilder private func pageViewport<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        // Each page keeps its own overflow position while tabs change.
        ScrollView { content() }
            .scrollIndicators(.visible)
            .frame(maxHeight: .infinity)
    }

    private var pageNavigation: some View {
        HStack(spacing: scaled(8)) {
            HStack(spacing: scaled(2)) {
                pageTab("Tasks", usage: false)
                pageTab("Usage", usage: true)
            }
            .padding(scaled(3))
            .background(palette.controlTrack, in: RoundedRectangle(cornerRadius: scaled(9)))
            Button { model.dismiss?() } label: {
                Image(systemName: "xmark").font(.system(size: fontSize(11), weight: .medium))
                    .frame(width: scaled(26), height: scaled(26))
            }
            .buttonStyle(.plain).foregroundStyle(palette.secondaryText).help("Close (Esc)")
            .accessibilityLabel("Close popup")
        }
    }

    private func pageTab(_ title: String, usage: Bool) -> some View {
        let selected = model.showingUsage == usage
        return Button {
            guard !selected else { return }
            if usage { model.showUsage() } else { model.showTasks() }
        } label: {
            Text(title).font(.system(size: fontSize(12), weight: .medium))
                .foregroundStyle(selected ? palette.accent : palette.secondaryText)
                .frame(maxWidth: .infinity, minHeight: scaled(26))
                .contentShape(Rectangle())
                .background(selected ? palette.selectedControl : .clear, in: RoundedRectangle(cornerRadius: scaled(6)))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(usage ? "popup-usage-tab" : "popup-tasks-tab")
        .accessibilityLabel(usage ? "Subscription usage" : "Task list")
        .accessibilityAddTraits(selected ? .isSelected : [])
        .help(usage ? "Subscription usage (⌘3)" : "Task list (⌘0)")
    }

    private var taskContent: some View {
        VStack(alignment: .leading, spacing: scaled(13)) {
            ForEach(model.instances) { instance in
                Button { model.showUsage(for: instance.id) } label: { weekly(instance) }
                    .buttonStyle(.plain).help("View \(instance.label) subscription limits and token activity")
                    .accessibilityIdentifier("weekly-quota-\(instance.id)")
                    .accessibilityLabel("\(instance.label) weekly quota, \(model.weeklyText(for: instance)). View subscription usage")
            }
            HStack(spacing: scaled(8)) {
                tab("Active", count: model.snapshot.activeCount, completed: false)
                tab("Finished", count: model.snapshot.completedCount, completed: true)
                Spacer(minLength: 0)
            }
            if let error = model.dashboardError { notice(error, symbol: "exclamationmark.triangle") }
            ForEach(model.instances) { instance in
                if let error = instance.error { notice("\(instance.label): \(error)", symbol: "exclamationmark.triangle") }
            }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        Color.clear.frame(height: 0).id("task-list-top")
                        if model.snapshot.loading {
                            empty("Reading Codex…", detail: "Checking local sessions and usage.", symbol: "ellipsis")
                                .frame(height: taskViewportHeight)
                        } else if model.visibleTasks.isEmpty {
                            empty(model.showingCompleted ? "No recently finished tasks" : "No active tasks",
                                  detail: model.showingCompleted ? "Finished, stopped and failed tasks stay here for 15 minutes." : "Start a task in \(model.instances.map(\.label).joined(separator: " or ")) or Codex CLI. It will appear here automatically.",
                                  symbol: model.showingCompleted ? "checkmark.circle" : "terminal")
                                .frame(height: taskViewportHeight)
                        } else {
                            LazyVStack(spacing: scaled(4)) {
                                ForEach(model.visibleTasks) { task in taskRow(task) }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, minHeight: taskViewportHeight, alignment: .top)
                }
                .frame(height: taskViewportHeight)
                .scrollIndicators(.visible)
                .onChange(of: model.showingCompleted) { _, _ in
                    // A long list's previous offset must not leave the newly
                    // selected list offscreen for its first frame.
                    proxy.scrollTo("task-list-top", anchor: .top)
                }
            }
            if let message = model.message {
                VStack(alignment: .leading, spacing: scaled(7)) {
                    HStack(alignment: .top, spacing: scaled(8)) {
                        Text(message).font(.system(size: fontSize(11))).fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: scaled(4))
                        Button { model.message = nil; model.failedTask = nil } label: { Image(systemName: "xmark") }
                            .buttonStyle(.plain).accessibilityLabel("Dismiss message")
                    }
                    if let task = model.failedTask {
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: scaled(8)) { recoveryButtons(task) }
                            VStack(alignment: .leading, spacing: scaled(6)) { recoveryButtons(task) }
                        }
                        .buttonStyle(.bordered).controlSize(.small).font(.system(size: fontSize(11)))
                    }
                }
                .padding(scaled(10)).background(palette.notice, in: RoundedRectangle(cornerRadius: scaled(10)))
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: scaled(10)) {
            HStack {
                Button { model.openSettings() } label: { Label("Settings", systemImage: "gearshape").fixedSize() }
                    .help("Customize the Codex CLI status bar…")
                Spacer(minLength: 0)
                Button {
                    themeSaveFailed = !themePreferences.toggle()
                } label: {
                    Image(systemName: themePreferences.theme.toggleSymbol)
                        .font(.system(size: fontSize(12)))
                        .frame(width: scaled(26), height: scaled(26))
                        .contentShape(Rectangle())
                }
                .help(themePreferences.theme.toggleLabel)
                .accessibilityLabel(themePreferences.theme.toggleLabel)
                .accessibilityValue(themePreferences.theme == .dark ? "Dark theme" : "Light theme")
                .accessibilityIdentifier("companion-theme-toggle")
                Button { model.quit?() } label: { Image(systemName: "power") }
                    .help("Quit Agent Companion").accessibilityLabel("Quit Agent Companion")
            }
            Button { model.openCodex() } label: { Label("Open Codex", systemImage: "arrow.up.forward.app").fixedSize() }
        }
        .buttonStyle(.plain).font(.system(size: fontSize(11))).foregroundStyle(palette.secondaryText)
    }

    private func weekly(_ instance: CodexInstance) -> some View {
        let usage = model.weeklyUsage(for: instance)
        let remaining = model.weeklyRemainingPercent(for: instance)
        return VStack(alignment: .leading, spacing: scaled(7)) {
            HStack(spacing: scaled(8)) {
                Text("\(instance.label) weekly quota").font(.system(size: fontSize(11))).foregroundStyle(palette.secondaryText)
                Spacer()
                Text(model.weeklyText(for: instance) + (remaining == nil ? "" : " left"))
                    .font(.system(size: fontSize(12), weight: .medium)).monospacedDigit()
            }
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(palette.quotaTrack)
                    if let remaining {
                        Capsule().fill(remaining <= 10 ? palette.warning : palette.accent)
                            .frame(width: geometry.size.width * CGFloat(remaining) / 100)
                    }
                }
            }.frame(height: scaled(3)).accessibilityHidden(true)
            if let usage, !usage.expired, let resets = usage.resetsAt {
                Text("Resets \(Date(timeIntervalSince1970: resets).formatted(date: .abbreviated, time: .shortened))")
                    .font(.system(size: fontSize(10))).foregroundStyle(palette.secondaryText)
            } else {
                Text(usage?.expired == true ? "Waiting for a new \(instance.label) usage reading after reset." : "Usage appears after \(instance.label) records a rate limit reading.")
                    .font(.system(size: fontSize(10))).foregroundStyle(palette.secondaryText)
            }
        }
        .padding(scaled(12)).background(palette.surface, in: RoundedRectangle(cornerRadius: scaled(12)))
    }

    private func tab(_ title: String, count: Int, completed: Bool) -> some View {
        Button { model.showingCompleted = completed } label: {
            HStack(spacing: scaled(5)) {
                Text(title)
                Text(model.countText(count)).monospacedDigit().foregroundStyle(model.showingCompleted == completed ? palette.accent : palette.mutedText)
            }
            .font(.system(size: fontSize(11), weight: .medium)).fixedSize()
            .padding(.horizontal, scaled(8)).padding(.vertical, scaled(7))
            .background(model.showingCompleted == completed ? palette.selectedControl : palette.inactiveControl, in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title), \(count) tasks")
        .accessibilityAddTraits(model.showingCompleted == completed ? .isSelected : [])
        .keyboardShortcut(completed ? "2" : "1", modifiers: .command)
    }

    private func taskRow(_ task: CodexTask) -> some View {
        Button { model.jump(to: task) } label: {
            HStack(spacing: scaled(10)) {
                TaskActivityMark(activity: task.activity, symbol: task.symbol, size: fontSize(13),
                                 animating: model.animatesTaskActivity && model.isPresented && !model.showingUsage)
                    .frame(width: scaled(18))
                VStack(alignment: .leading, spacing: scaled(5)) {
                    Text(task.title).font(.system(size: fontSize(12), weight: .medium)).lineLimit(1)
                    HStack(spacing: scaled(5)) {
                        if CompanionPopupLayout.width >= 260 {
                            Text(task.project).lineLimit(1)
                            Text("·")
                        }
                        Text(task.sourceLabel).foregroundStyle(palette.accent).lineLimit(1).fixedSize()
                        Text(task.status).lineLimit(1).fixedSize()
                        Spacer(minLength: scaled(4))
                        Text(age(task.updatedAt)).monospacedDigit().fixedSize()
                    }.font(.system(size: fontSize(10))).foregroundStyle(palette.secondaryText)
                }
                if model.jumpingID == task.id { ProgressView().controlSize(.mini) }
                else { Image(systemName: "arrow.up.right").font(.system(size: fontSize(10))).foregroundStyle(palette.secondaryText) }
            }
            .padding(.horizontal, scaled(10)).frame(height: scaled(54))
            .background(hoveredTask == task.id ? palette.hover : palette.row, in: RoundedRectangle(cornerRadius: scaled(10)))
            .contentShape(RoundedRectangle(cornerRadius: scaled(10)))
        }
        .buttonStyle(.plain)
        .disabled(model.jumpingID != nil)
        .onHover { inside in
            if inside { hoveredTask = task.id }
            else if hoveredTask == task.id { hoveredTask = nil }
        }
        .help("\(task.title)\n\(task.cwd ?? task.project)\nOpen this \(task.client == "desktop" ? "Codex conversation" : "session")")
        .accessibilityLabel("\(task.sourceLabel), \(task.title), \(task.status), \(task.project). Open session")
        .contextMenu {
            Button("Open session") { model.jump(to: task) }
            if model.resumeCommand(for: task) != nil { Button("Copy resume command") { model.copyResume(task) } }
        }
    }

    @ViewBuilder private func recoveryButtons(_ task: CodexTask) -> some View {
        Button("Try again") { model.jump(to: task) }
        if model.resumeCommand(for: task) != nil {
            Button("Copy command") { model.copyResume(task) }
                .accessibilityLabel("Copy resume command")
                .help("Copy the command to resume this Codex session")
        }
    }

    private func age(_ timestamp: TimeInterval) -> String {
        let seconds = max(0, Int(Date().timeIntervalSince1970 - timestamp))
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        return "\(seconds / 3600)h"
    }

    private func empty(_ title: String, detail: String, symbol: String) -> some View {
        VStack(spacing: scaled(9)) {
            Image(systemName: symbol).font(.system(size: fontSize(22))).foregroundStyle(palette.mutedIcon)
            Text(title).font(.system(size: fontSize(12), weight: .medium))
            Text(detail).font(.system(size: fontSize(11))).foregroundStyle(palette.secondaryText).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity).frame(height: scaled(132)).padding(.horizontal, scaled(25))
    }

    private func notice(_ text: String, symbol: String) -> some View {
        Label(text, systemImage: symbol).font(.system(size: fontSize(11))).foregroundStyle(palette.warning)
            .fixedSize(horizontal: false, vertical: true)
    }
}
