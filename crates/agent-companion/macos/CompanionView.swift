// SPDX-License-Identifier: GPL-3.0-only
import SwiftUI

struct CompanionView: View {
    @ObservedObject var model: CompanionModel
    var showsDetails: Bool? = nil
    var drawsBackground = true
    var detailsHeight: CGFloat? = nil
    @State private var hoveredTask: String?

    // Snap layout dimensions to points so fractional density scaling cannot
    // change the viewport's rounding when an empty/populated tab is selected.
    private func scaled(_ value: CGFloat) -> CGFloat { (value * model.metrics.detailScale).rounded() }
    private func fontSize(_ value: CGFloat) -> CGFloat { max(9, value * model.metrics.detailScale) }
    private func compactScaled(_ value: CGFloat) -> CGFloat { value * min(1, model.compactWidth / 179) }
    private var taskViewportHeight: CGFloat {
        let active = model.snapshot.tasks.filter { $0.isActive }.count
        let finished = model.snapshot.tasks.count - active
        func height(_ count: Int) -> CGFloat { count == 0 ? 132 : min(CGFloat(count) * 58, 232) }
        // Both tabs share one viewport, so switching filters cannot resize the
        // panel or move the footer. Empty states need room for their guidance.
        return scaled(max(height(active), height(finished)))
    }

    var body: some View {
        VStack(spacing: 0) {
            compact
            if showsDetails ?? model.expanded {
                expanded
                    .allowsHitTesting(model.expanded)
                    .accessibilityHidden(!model.expanded)
            }
        }
        .frame(width: model.compactWidth)
        .background {
            if drawsBackground {
                NotchOutline(hasCamera: model.hasCamera, expansion: model.expanded ? 1 : 0).fill(.black)
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .foregroundStyle(.white)
        .preferredColorScheme(.dark)
        .onExitCommand { model.collapse?() }
    }

    private var compact: some View {
        Button { model.expand?() } label: {
            Group {
                if model.hasCamera {
                    cameraSummary
                } else {
                    ordinarySummary
                }
            }
            .font(.system(size: compactScaled(12)))
            .frame(width: model.compactWidth, height: model.compactHeight)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("notch-summary")
        .accessibilityLabel(model.summaryDescription)
        .accessibilityHint("Open task list")
        .help(model.summaryDescription + " · \(model.snapshot.completedCount) finished in the last 15 minutes")
        .contextMenu {
            Button("Show tasks") { model.showTasks(); model.expand?() }
            Button("Subscription usage") { model.showUsage() }
            Button("Settings…") { model.openSettings() }
            Divider()
            Button("Quit Agent Companion") { model.quit?() }
        }
    }

    private var ordinarySummary: some View {
        HStack(spacing: 0) {
            quotaSummary(scale: compactScaled(1.2))
            Spacer(minLength: compactScaled(8))
            workingSummary(scale: compactScaled(1.2))
        }
        .frame(height: model.metrics.statsHeight)
        .padding(.horizontal, compactScaled(10))
    }

    private var cameraSummary: some View {
        let scale = model.metrics.cameraContentScale
        return HStack(spacing: 0) {
            quotaSummary(scale: scale)
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 9 * scale).padding(.trailing, 3 * scale)
            .frame(width: model.cameraLeftWidth)

            // These are physical camera pixels, not a place to draw content.
            Color.clear.frame(width: model.metrics.cameraWidth)

            workingSummary(scale: scale)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.leading, 3 * scale).padding(.trailing, 9 * scale)
            .frame(width: model.cameraRightWidth)
        }
        .frame(height: model.metrics.cameraHeight)
    }

    private func quotaSummary(scale: CGFloat) -> some View {
        Text(model.weeklyText)
            .font(.system(size: 10 * scale, weight: .semibold)).monospacedDigit()
            .foregroundStyle(model.quotaTint).lineLimit(1).fixedSize()
    }

    private func workingSummary(scale: CGFloat) -> some View {
        HStack(spacing: 4 * scale) {
            HStack(spacing: 2 * scale) {
                Image(systemName: "circle.inset.filled")
                    .font(.system(size: 6 * scale, weight: .semibold)).frame(width: 6 * scale)
                Text(model.countText(model.workingCount))
                    .font(.system(size: 10 * scale, weight: .semibold)).monospacedDigit()
                    .lineLimit(1).fixedSize()
            }
            .foregroundStyle(model.workingCount > 0 ? CompanionModel.accent : .white.opacity(0.55))
            if model.needsInput {
                Image(systemName: "questionmark")
                    .font(.system(size: 10 * scale, weight: .bold)).frame(width: 6 * scale)
                    .foregroundStyle(.orange)
            }
        }
    }

    private var expanded: some View {
        VStack(alignment: .leading, spacing: scaled(13)) {
            pageNavigation
            if detailsHeight != nil {
                ScrollView {
                    expandedContent
                }
                .scrollIndicators(.visible)
                .frame(maxHeight: .infinity)
            } else {
                expandedContent
            }
            Divider().overlay(.white.opacity(0.06))
            footer
        }
        .padding(.horizontal, scaled(16)).padding(.top, scaled(12)).padding(.bottom, scaled(16))
        .frame(maxWidth: .infinity)
        .frame(height: detailsHeight)
    }

    private var expandedContent: some View {
        Group {
            if model.showingUsage {
                SubscriptionUsageView(usage: model.subscriptionUsage, loading: model.usageLoading,
                                      scale: model.metrics.detailScale, refresh: { model.refreshUsage(force: true) })
            } else {
                taskContent
            }
        }
    }

    private var pageNavigation: some View {
        HStack(spacing: scaled(8)) {
            HStack(spacing: scaled(2)) {
                pageTab("Tasks", usage: false)
                pageTab("Usage", usage: true)
            }
            .padding(scaled(3))
            .background(.white.opacity(0.06), in: RoundedRectangle(cornerRadius: scaled(9)))
            Button { model.collapse?() } label: {
                Image(systemName: "chevron.up").font(.system(size: fontSize(11), weight: .medium))
                    .frame(width: scaled(26), height: scaled(26))
            }
            .buttonStyle(.plain).foregroundStyle(.secondary).help("Collapse (Esc)")
            .accessibilityLabel("Collapse panel")
        }
    }

    private func pageTab(_ title: String, usage: Bool) -> some View {
        let selected = model.showingUsage == usage
        return Button {
            guard !selected else { return }
            if usage { model.showUsage() } else { model.showTasks() }
        } label: {
            Text(title).font(.system(size: fontSize(12), weight: .medium))
                .foregroundStyle(selected ? CompanionModel.accent : .white.opacity(0.6))
                .frame(maxWidth: .infinity, minHeight: scaled(26))
                .contentShape(Rectangle())
                .background(.white.opacity(selected ? 0.10 : 0), in: RoundedRectangle(cornerRadius: scaled(6)))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(usage ? "notch-usage-tab" : "notch-tasks-tab")
        .accessibilityLabel(usage ? "Subscription usage" : "Task list")
        .accessibilityAddTraits(selected ? .isSelected : [])
        .help(usage ? "Subscription usage (⌘3)" : "Task list (⌘0)")
    }

    private var taskContent: some View {
        VStack(alignment: .leading, spacing: scaled(13)) {
            Button { model.showUsage() } label: { weekly }
                .buttonStyle(.plain).help("View subscription limits and token activity")
                .accessibilityLabel("Weekly quota, \(model.weeklyText). View subscription usage")
            HStack(spacing: scaled(8)) {
                tab("Active", count: model.snapshot.activeCount, completed: false)
                tab("Finished", count: model.snapshot.completedCount, completed: true)
                Spacer(minLength: 0)
            }
            if let error = model.snapshot.error { notice(error, symbol: "exclamationmark.triangle") }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 0) {
                        Color.clear.frame(height: 0).id("task-list-top")
                        if model.snapshot.loading {
                            empty("Reading Codex…", detail: "Checking local sessions and usage.", symbol: "ellipsis")
                                .frame(height: taskViewportHeight)
                        } else if model.visibleTasks.isEmpty {
                            empty(model.showingCompleted ? "No recently finished tasks" : "No active tasks",
                                  detail: model.showingCompleted ? "Finished, stopped and failed tasks stay here for 15 minutes." : "Start a task in Codex.app or Codex CLI. It will appear here automatically.",
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
                .padding(scaled(10)).background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: scaled(10)))
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: scaled(10)) {
            HStack {
                Button { model.openSettings() } label: { Label("Settings", systemImage: "gearshape").fixedSize() }
                    .help("Customize the Codex CLI status bar…")
                Spacer(minLength: 0)
                Button { model.quit?() } label: { Image(systemName: "power") }
                    .help("Quit Agent Companion").accessibilityLabel("Quit Agent Companion")
            }
            Button { model.openCodex() } label: { Label("Open Codex", systemImage: "arrow.up.forward.app").fixedSize() }
        }
        .buttonStyle(.plain).font(.system(size: fontSize(11))).foregroundStyle(.secondary)
    }

    private var weekly: some View {
        VStack(alignment: .leading, spacing: scaled(7)) {
            HStack(spacing: scaled(8)) {
                Text("Weekly quota").font(.system(size: fontSize(11))).foregroundStyle(.secondary)
                Spacer()
                Text(model.weeklyText + (model.weeklyRemainingPercent == nil ? "" : " left"))
                    .font(.system(size: fontSize(12), weight: .medium)).monospacedDigit()
            }
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.10))
                    if let remaining = model.weeklyRemainingPercent {
                        Capsule().fill(remaining <= 10 ? Color.orange : CompanionModel.accent)
                            .frame(width: geometry.size.width * CGFloat(remaining) / 100)
                    }
                }
            }.frame(height: scaled(3)).accessibilityHidden(true)
            if let usage = model.weeklyUsage, !usage.expired, let resets = usage.resetsAt {
                Text("Resets \(Date(timeIntervalSince1970: resets).formatted(date: .abbreviated, time: .shortened))")
                    .font(.system(size: fontSize(10))).foregroundStyle(.secondary)
            } else {
                Text(model.weeklyUsage?.expired == true ? "Waiting for a new Codex usage reading after reset." : "Usage appears after Codex records a rate limit reading.")
                    .font(.system(size: fontSize(10))).foregroundStyle(.secondary)
            }
        }
        .padding(scaled(12)).background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: scaled(12)))
    }

    private func tab(_ title: String, count: Int, completed: Bool) -> some View {
        Button { model.showingCompleted = completed } label: {
            HStack(spacing: scaled(5)) {
                Text(title)
                Text(model.countText(count)).monospacedDigit().foregroundStyle(model.showingCompleted == completed ? CompanionModel.accent : .white.opacity(0.55))
            }
            .font(.system(size: fontSize(11), weight: .medium)).fixedSize()
            .padding(.horizontal, scaled(8)).padding(.vertical, scaled(7))
            .background(.white.opacity(model.showingCompleted == completed ? 0.11 : 0.035), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title), \(count) tasks")
        .accessibilityAddTraits(model.showingCompleted == completed ? .isSelected : [])
        .keyboardShortcut(completed ? "2" : "1", modifiers: .command)
    }

    private func taskRow(_ task: CodexTask) -> some View {
        Button { model.jump(to: task) } label: {
            HStack(spacing: scaled(10)) {
                Image(systemName: task.symbol).foregroundStyle(task.tint).font(.system(size: fontSize(13))).frame(width: scaled(18))
                VStack(alignment: .leading, spacing: scaled(5)) {
                    Text(task.title).font(.system(size: fontSize(12), weight: .medium)).lineLimit(1)
                    HStack(spacing: scaled(5)) {
                        if model.compactWidth >= 260 {
                            Text(task.project).lineLimit(1)
                            Text("·")
                        }
                        Text(task.status).lineLimit(1).fixedSize()
                        Spacer(minLength: scaled(4))
                        Text(age(task.updatedAt)).monospacedDigit().fixedSize()
                    }.font(.system(size: fontSize(10))).foregroundStyle(.secondary)
                }
                if model.jumpingID == task.id { ProgressView().controlSize(.mini) }
                else { Image(systemName: "arrow.up.right").font(.system(size: fontSize(10))).foregroundStyle(.secondary) }
            }
            .padding(.horizontal, scaled(10)).frame(height: scaled(54))
            .background(.white.opacity(hoveredTask == task.id ? 0.10 : 0.045), in: RoundedRectangle(cornerRadius: scaled(10)))
            .contentShape(RoundedRectangle(cornerRadius: scaled(10)))
        }
        .buttonStyle(.plain)
        .disabled(model.jumpingID != nil)
        .onHover { inside in
            if inside { hoveredTask = task.id }
            else if hoveredTask == task.id { hoveredTask = nil }
        }
        .help("\(task.title)\n\(task.cwd ?? task.project)\nOpen this \(task.client == "desktop" ? "Codex conversation" : "session")")
        .accessibilityLabel("\(task.title), \(task.status), \(task.project). Open session")
        .contextMenu {
            Button("Open session") { model.jump(to: task) }
            if task.resumeCommand != nil { Button("Copy resume command") { model.copyResume(task) } }
        }
    }

    @ViewBuilder private func recoveryButtons(_ task: CodexTask) -> some View {
        Button("Try again") { model.jump(to: task) }
        if task.resumeCommand != nil {
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
            Image(systemName: symbol).font(.system(size: fontSize(22))).foregroundStyle(.white.opacity(0.35))
            Text(title).font(.system(size: fontSize(12), weight: .medium))
            Text(detail).font(.system(size: fontSize(11))).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity).frame(height: scaled(132)).padding(.horizontal, scaled(25))
    }

    private func notice(_ text: String, symbol: String) -> some View {
        Label(text, systemImage: symbol).font(.system(size: fontSize(11))).foregroundStyle(.orange)
            .fixedSize(horizontal: false, vertical: true)
    }
}
